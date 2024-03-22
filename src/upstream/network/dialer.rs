use std::{
    error::Error,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpStream, UdpSocket},
    task::JoinSet,
};

use crate::{adapter, option};

enum DialerWrapper {
    Basic(super::BasicDialer),

    #[cfg(feature = "upstream-dialer-socks5-support")]
    Socks5(super::Socks5Dialer),
}

pub(crate) struct Dialer {
    dialer: DialerWrapper,
    connect_timeout: Option<Duration>,
}

impl Dialer {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        options: option::DialerOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let basic_dialer = super::BasicDialer::new(manager, options.basic);

        cfg_if::cfg_if! {
            if #[cfg(feature = "upstream-dialer-socks5-support")] {
                if let Some(socks5_options) = options.socks5 {
                    let socks5_dialer = super::Socks5Dialer::new(basic_dialer, socks5_options)?;
                    Ok(Self {
                        dialer: DialerWrapper::Socks5(socks5_dialer),
                        connect_timeout: options.connect_timeout,
                    })
                } else {
                    Ok(Self {
                        dialer: DialerWrapper::Basic(basic_dialer),
                        connect_timeout: options.connect_timeout,
                    })
                }
            } else {
                Ok(Self {
                    dialer: DialerWrapper::Basic(basic_dialer),
                    connect_timeout: options.connect_timeout,
                })
            }
        }
    }
}

impl Dialer {
    async fn with_connect_timeout<T>(
        &self,
        f: impl Future<Output = io::Result<T>>,
    ) -> io::Result<T> {
        match self.connect_timeout {
            Some(timeout) => tokio::time::timeout(timeout, f)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))?,
            None => f.await,
        }
    }

    async fn new_tcp_stream_wrapper(
        &self,
        remote_addr: super::SocksAddr,
    ) -> io::Result<GenericTcpStream> {
        match &self.dialer {
            DialerWrapper::Basic(basic_dialer) => {
                let remote_addr = remote_addr.try_to_socket_addr().ok_or(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "domain address is unsupported in basic dialer",
                ))?;
                basic_dialer.new_tcp_stream(remote_addr).await
            }

            #[cfg(feature = "upstream-dialer-socks5-support")]
            DialerWrapper::Socks5(socks5_dialer) => socks5_dialer.new_tcp_stream(remote_addr).await,
        }
    }

    async fn new_udp_socket_wrapper(
        &self,
        addr: &super::SocksAddr,
    ) -> io::Result<(GenericUdpSocket, super::SocksAddr)> {
        match &self.dialer {
            DialerWrapper::Basic(basic_dialer) => {
                let addr = addr.try_to_socket_addr_ref().ok_or(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "domain address is unsupported in basic dialer",
                ))?;
                let udp_socket = basic_dialer.new_udp_socket(addr).await?;
                Ok((
                    GenericUdpSocket::Basic(udp_socket),
                    super::SocksAddr::from(addr.clone()),
                ))
            }

            #[cfg(feature = "upstream-dialer-socks5-support")]
            DialerWrapper::Socks5(socks5_dialer) => {
                let (udp_socket, remote_addr) = socks5_dialer.new_udp_socket(addr).await?;
                Ok((GenericUdpSocket::Socks5(udp_socket), remote_addr))
            }
        }
    }

    #[cfg(all(feature = "upstream-quic-support", feature = "upstream-tls-support"))]
    async fn new_quic_connection_wrapper(
        &self,
        remote_addr: super::SocksAddr,
        quic_client_config: quinn::ClientConfig,
        server_name: &str,
    ) -> io::Result<(quinn::Endpoint, quinn::Connection)> {
        match &self.dialer {
            DialerWrapper::Basic(basic_dialer) => {
                let remote_addr = remote_addr.try_to_socket_addr().ok_or(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "domain address is unsupported in basic dialer",
                ))?;
                basic_dialer
                    .new_quic_connection(remote_addr, quic_client_config, server_name)
                    .await
            }

            #[cfg(feature = "upstream-dialer-socks5-support")]
            DialerWrapper::Socks5(socks5_dialer) => {
                socks5_dialer
                    .new_quic_connection(remote_addr, quic_client_config, server_name)
                    .await
            }
        }
    }

    pub(crate) async fn start(&self) {
        match &self.dialer {
            DialerWrapper::Basic(basic_dialer) => basic_dialer.start().await,

            #[cfg(feature = "upstream-dialer-socks5-support")]
            DialerWrapper::Socks5(socks5_dialer) => socks5_dialer.start().await,
        }
    }

    pub(crate) async fn close(&self) {
        match &self.dialer {
            DialerWrapper::Basic(basic_dialer) => basic_dialer.close().await,

            #[cfg(feature = "upstream-dialer-socks5-support")]
            DialerWrapper::Socks5(socks5_dialer) => socks5_dialer.close().await,
        }
    }

    pub(crate) fn domain_support(&self) -> bool {
        match &self.dialer {
            DialerWrapper::Basic(_) => false,

            #[cfg(feature = "upstream-dialer-socks5-support")]
            DialerWrapper::Socks5(_) => true,
        }
    }

    pub(crate) async fn new_tcp_stream(
        &self,
        remote_addr: super::SocksAddr,
    ) -> io::Result<GenericTcpStream> {
        self.with_connect_timeout(self.new_tcp_stream_wrapper(remote_addr))
            .await
    }

    pub(crate) async fn new_udp_socket(
        &self,
        addr: &super::SocksAddr,
    ) -> io::Result<(GenericUdpSocket, super::SocksAddr)> {
        self.with_connect_timeout(self.new_udp_socket_wrapper(addr))
            .await
    }

    #[cfg(all(feature = "upstream-quic-support", feature = "upstream-tls-support"))]
    pub(crate) async fn new_quic_connection(
        &self,
        remote_addr: super::SocksAddr,
        quic_client_config: quinn::ClientConfig,
        server_name: &str,
    ) -> io::Result<(quinn::Endpoint, quinn::Connection)> {
        self.with_connect_timeout(self.new_quic_connection_wrapper(
            remote_addr,
            quic_client_config,
            server_name,
        ))
        .await
    }

    pub(crate) async fn parallel_new_tcp_stream(
        self: &Arc<Self>,
        remote_ip_addr: Vec<IpAddr>,
        port: u16,
    ) -> io::Result<GenericTcpStream> {
        let mut join_set = JoinSet::new();
        for ip in remote_ip_addr {
            let remote_addr = super::SocksAddr::SocketAddr(SocketAddr::new(ip, port));
            let s = self.clone();
            join_set.spawn(async move { s.new_tcp_stream(remote_addr).await });
        }
        let mut err: Option<io::Error> = None;
        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Ok(tcp_stream)) => {
                    join_set.abort_all();
                    return Ok(tcp_stream);
                }
                Ok(Err(e)) => err = Some(e),
                Err(e) => err = Some(io::Error::new(io::ErrorKind::Other, e)),
            }
        }
        join_set.abort_all();
        Err(err.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "create new tcp stream failed")
        }))
    }
}

// Tcp

pub(crate) enum GenericTcpStream {
    Tokio(TcpStream),
    Tfo(tokio_tfo::TfoStream),
}

impl From<TcpStream> for GenericTcpStream {
    fn from(value: TcpStream) -> Self {
        Self::Tokio(value)
    }
}

impl From<tokio_tfo::TfoStream> for GenericTcpStream {
    fn from(value: tokio_tfo::TfoStream) -> Self {
        Self::Tfo(value)
    }
}

impl AsyncWrite for GenericTcpStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Self::Tokio(tcp_stream) => std::pin::Pin::new(tcp_stream).poll_write(cx, buf),
            Self::Tfo(tfo_stream) => std::pin::Pin::new(tfo_stream).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Self::Tokio(tcp_stream) => std::pin::Pin::new(tcp_stream).poll_write_vectored(cx, bufs),
            Self::Tfo(tfo_stream) => std::pin::Pin::new(tfo_stream).poll_write_vectored(cx, bufs),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Tokio(tcp_stream) => std::pin::Pin::new(tcp_stream).poll_flush(cx),
            Self::Tfo(tfo_stream) => std::pin::Pin::new(tfo_stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Tokio(tcp_stream) => std::pin::Pin::new(tcp_stream).poll_shutdown(cx),
            Self::Tfo(tfo_stream) => std::pin::Pin::new(tfo_stream).poll_shutdown(cx),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tokio(tcp_stream) => tcp_stream.is_write_vectored(),
            Self::Tfo(tfo_stream) => tfo_stream.is_write_vectored(),
        }
    }
}

impl AsyncRead for GenericTcpStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tokio(tcp_stream) => std::pin::Pin::new(tcp_stream).poll_read(cx, buf),
            Self::Tfo(tfo_stream) => std::pin::Pin::new(tfo_stream).poll_read(cx, buf),
        }
    }
}

// Udp

pub(crate) enum GenericUdpSocket {
    Basic(UdpSocket),

    #[cfg(feature = "upstream-dialer-socks5-support")]
    Socks5(super::Socks5UdpSocket),
}

impl GenericUdpSocket {
    pub(crate) async fn recv_buf_from<B: bytes::BufMut>(
        &self,
        buf: &mut B,
    ) -> io::Result<(usize, super::SocksAddr)> {
        match self {
            Self::Basic(udp_socket) => {
                let (n, addr) = udp_socket.recv_buf_from(buf).await?;
                Ok((n, super::SocksAddr::SocketAddr(addr)))
            }

            #[cfg(feature = "upstream-dialer-socks5-support")]
            Self::Socks5(socks5_udp_socket) => socks5_udp_socket.recv_buf_from(buf).await,
        }
    }

    pub(crate) async fn send_to(&self, buf: &[u8], target: super::SocksAddr) -> io::Result<usize> {
        match self {
            Self::Basic(udp_socket) => {
                let target = target.try_to_socket_addr().ok_or(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "domain address is unsupported in basic dialer",
                ))?;
                udp_socket.send_to(buf, target).await
            }

            #[cfg(feature = "upstream-dialer-socks5-support")]
            Self::Socks5(socks5_udp_socket) => socks5_udp_socket.send_to(buf, target).await,
        }
    }
}

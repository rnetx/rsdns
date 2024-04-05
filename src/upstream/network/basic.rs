use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use tokio::net::{TcpSocket, UdpSocket};

use crate::{adapter, option};

pub(super) struct BasicDialer {
    manager: Arc<Box<dyn adapter::Manager>>,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    bind_interface: Option<String>,
    tcp_fast_open: bool,

    #[cfg(any(target_os = "linux", target_os = "android"))]
    so_mark: Option<u32>,

    #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
    quic_endpoint_config: quinn::EndpointConfig,
}

impl BasicDialer {
    pub(super) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        options: option::BasicDialerOptions,
    ) -> Self {
        Self {
            manager,
            bind_ipv4: options.bind_ipv4,
            bind_ipv6: options.bind_ipv6,
            bind_interface: options.bind_interface,
            tcp_fast_open: options.tcp_fast_open,

            #[cfg(any(target_os = "linux", target_os = "android"))]
            so_mark: options.so_mark,

            #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
            quic_endpoint_config: quinn::EndpointConfig::default(),
        }
    }

    pub(super) async fn start(&self) {
        if self.bind_interface.is_some() {
            if self
                .manager
                .get_state_map()
                .try_get::<super::InterfaceFinder>()
                .is_none()
            {
                let finder = super::InterfaceFinder::new();
                finder.start().await;
                self.manager.get_state_map().set(finder);
            }
        }
    }

    pub(super) async fn close(&self) {
        if self.bind_interface.is_some() {
            if let Some(finder) = self
                .manager
                .get_state_map()
                .try_get::<super::InterfaceFinder>()
            {
                finder.close().await;
            }
        }
    }

    pub(super) async fn new_tcp_stream(
        &self,
        remote_addr: SocketAddr,
    ) -> io::Result<super::GenericTcpStream> {
        let tcp_socket = match &remote_addr {
            SocketAddr::V4(_) => {
                let tcp_socket = TcpSocket::new_v4()?;
                if let Some(ip) = &self.bind_ipv4 {
                    tcp_socket.bind(SocketAddr::new(IpAddr::V4(ip.clone()), 0))?;
                } else {
                    tcp_socket.bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))?;
                }
                tcp_socket
            }
            SocketAddr::V6(_) => {
                let tcp_socket = TcpSocket::new_v6()?;
                if let Some(ip) = &self.bind_ipv6 {
                    tcp_socket.bind(SocketAddr::new(IpAddr::V6(ip.clone()), 0))?;
                } else {
                    tcp_socket.bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0))?;
                }
                tcp_socket
            }
        };

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if let Some(so_mark) = self.so_mark {
                super::set_so_mark(&tcp_socket, so_mark)?;
            }
        }

        if let Some(interface) = &self.bind_interface {
            let finder = self.manager.get_state_map().get::<super::InterfaceFinder>();
            super::set_interface(&tcp_socket, finder, interface, remote_addr.is_ipv6()).await?;
        }

        let tcp_stream = if !self.tcp_fast_open {
            tcp_socket
                .connect(remote_addr)
                .await
                .map(|s| super::GenericTcpStream::from(s))?
        } else {
            tokio_tfo::TfoStream::connect_with_socket(tcp_socket, remote_addr)
                .await
                .map(|s| super::GenericTcpStream::from(s))?
        };

        Ok(tcp_stream)
    }

    pub(super) async fn new_udp_socket(&self, addr: &SocketAddr) -> io::Result<UdpSocket> {
        let udp_socket = match addr {
            SocketAddr::V4(_) => match &self.bind_ipv4 {
                Some(ip) => UdpSocket::bind(SocketAddr::new(IpAddr::V4(ip.clone()), 0)),
                None => UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)),
            },
            SocketAddr::V6(_) => match &self.bind_ipv6 {
                Some(ip) => UdpSocket::bind(SocketAddr::new(IpAddr::V6(ip.clone()), 0)),
                None => UdpSocket::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)),
            },
        }
        .await?;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if let Some(so_mark) = self.so_mark {
                super::set_so_mark(&udp_socket, so_mark)?;
            }
        }

        if let Some(interface) = &self.bind_interface {
            let finder = self.manager.get_state_map().get::<super::InterfaceFinder>();
            super::set_interface(&udp_socket, finder, interface, addr.is_ipv6()).await?;
        }

        Ok(udp_socket)
    }

    #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
    pub(super) async fn new_quic_connection(
        &self,
        remote_addr: SocketAddr,
        quic_client_config: quinn::ClientConfig,
        server_name: &str,
    ) -> io::Result<(quinn::Endpoint, quinn::Connection)> {
        let udp_socket = self.new_udp_socket(&remote_addr).await?;
        let runtime = Arc::new(quinn::TokioRuntime);
        let std_udp_socket = udp_socket.into_std()?;
        let endpoint = quinn::Endpoint::new(
            self.quic_endpoint_config.clone(),
            None,
            std_udp_socket,
            runtime,
        )
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to create QUIC endpoint: {}", err),
            )
        })?;
        let quic_connecting = endpoint
            .connect_with(quic_client_config, remote_addr, server_name)
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("failed to connect to QUIC server: {}", err),
                )
            })?;
        let quic_connection = quic_connecting.await.map_err(|err| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to establish QUIC connection: {}", err),
            )
        })?;
        Ok((endpoint, quic_connection))
    }
}

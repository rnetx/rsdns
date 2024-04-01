use std::{
    io,
    net::SocketAddr,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UdpSocket,
};

use protocol::{Decoder, Encoder};

use crate::option;

pub(super) struct Socks5Dialer {
    basic_dialer: Arc<super::BasicDialer>,
    address: SocketAddr,
    user_pass: Option<(String, String)>,

    #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
    quic_endpoint_config: quinn::EndpointConfig,
}

impl Socks5Dialer {
    pub(super) fn new(
        basic_dialer: super::BasicDialer,
        options: option::Socks5DialerOptions,
    ) -> anyhow::Result<Self> {
        let user_pass = match (options.username, options.password) {
            (Some(username), Some(password)) => Some((username, password)),
            (Some(_), None) => return Err(anyhow::anyhow!("socks5: missing password")),
            (None, Some(_)) => return Err(anyhow::anyhow!("socks5: missing username")),
            _ => None,
        };
        Ok(Self {
            basic_dialer: Arc::new(basic_dialer),
            address: options.address,
            user_pass,

            #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
            quic_endpoint_config: {
                let mut config = quinn::EndpointConfig::default();
                config.max_udp_payload_size(1500 - 20 - 8 - 261).ok();
                config
            },
        })
    }

    pub(super) async fn start(&self) {
        self.basic_dialer.start().await;
    }

    pub(super) async fn close(&self) {
        self.basic_dialer.close().await;
    }

    async fn basic_handshake<S: AsyncRead + AsyncWrite + Unpin + Send>(
        &self,
        stream: &mut S,
    ) -> io::Result<()> {
        let need_auth = {
            let request = if self.user_pass.is_none() {
                protocol::FirstRequest::new_no_auth()
            } else {
                protocol::FirstRequest::new_username_password()
            };
            let mut buf = Vec::with_capacity(request.encode_length());
            request.encode_to(&mut buf);
            stream.write_all(&mut buf).await?;
            let mut buf = bytes::BytesMut::new();
            stream.read_buf(&mut buf).await?;
            let response = protocol::FirstResponse::decode_from(&mut buf)?;
            response.is_username_password()
        };
        if need_auth && self.user_pass.is_some() {
            let (username, password) = self.user_pass.clone().unwrap();
            let request = protocol::UsernamePasswordRequest::new(username, password);
            let mut buf = Vec::with_capacity(request.encode_length());
            request.encode_to(&mut buf);
            stream.write_all(&mut buf).await?;
            let mut buf = bytes::BytesMut::new();
            stream.read_buf(&mut buf).await?;
            let response = protocol::UsernamePasswordResponse::decode_from(&mut buf)?;
            if !response.is_success() {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "username password auth failed",
                ));
            }
        }
        Ok(())
    }

    async fn command_request<S: AsyncRead + AsyncWrite + Unpin + Send>(
        stream: &mut S,
        command: u8,
        address: super::SocksAddr,
    ) -> io::Result<super::SocksAddr> {
        let request = protocol::CommandRequest { command, address };
        let mut buf = Vec::with_capacity(request.encode_length());
        request.encode_to(&mut buf);
        stream.write_all(&mut buf).await?;
        let mut buf = bytes::BytesMut::new();
        stream.read_buf(&mut buf).await?;
        let response = protocol::CommandResponse::decode_from(&mut buf)?;
        if !response.is_success() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!(
                    "command request failed: {}",
                    protocol::reply_to_err_string(response.reply)
                ),
            ));
        }
        Ok(response.address)
    }

    pub(super) async fn new_tcp_stream(
        &self,
        remote_addr: super::SocksAddr,
    ) -> io::Result<super::GenericTcpStream> {
        let mut tcp_stream = self
            .basic_dialer
            .new_tcp_stream(self.address.clone())
            .await?;
        self.basic_handshake(&mut tcp_stream).await.map_err(|err| {
            io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("socks5: handshake failed: {}", err),
            )
        })?;
        // Command Request
        async move {
            let _ = Self::command_request(&mut tcp_stream, 0x01, remote_addr).await?;
            Ok::<_, io::Error>(tcp_stream)
        }
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::ConnectionRefused, format!("socks5: {}", err)))
    }

    pub(super) async fn new_udp_socket(
        &self,
        addr: &super::SocksAddr,
    ) -> io::Result<(Socks5UdpSocket, super::SocksAddr)> {
        let mut tcp_stream = self
            .basic_dialer
            .new_tcp_stream(self.address.clone())
            .await?;
        self.basic_handshake(&mut tcp_stream).await?;
        // Command Request
        async {
            let peer_addr = match Self::command_request(&mut tcp_stream, 0x03, addr.clone()).await?
            {
                super::SocksAddr::DomainAddr(_, _) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "udp peer domain address is unsupported",
                    ));
                }
                super::SocksAddr::SocketAddr(addr) => addr,
            };
            let udp_socket = self.basic_dialer.new_udp_socket(&peer_addr).await?;
            Ok((
                Socks5UdpSocket {
                    _tcp_stream: tcp_stream,
                    inner: udp_socket,
                    peer_addr,
                },
                addr.clone(),
            ))
        }
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::ConnectionRefused, format!("socks5: {}", err)))
    }

    #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
    pub(super) async fn new_quic_connection(
        &self,
        remote_addr: super::SocksAddr,
        quic_client_config: quinn::ClientConfig,
        server_name: &str,
    ) -> io::Result<(quinn::Endpoint, quinn::Connection)> {
        let (udp_socket, raddr) = self.new_udp_socket(&remote_addr).await?;
        let runtime: Arc<quic::Socks5QUICTokioRuntime> = Arc::new(quic::Socks5QUICTokioRuntime);
        let maybe_fake_socket_addr = match &remote_addr {
            super::SocksAddr::DomainAddr(_, port) => quic::fake_socket_addr(*port),
            super::SocksAddr::SocketAddr(addr) => addr.clone(),
        };
        let quic_udp_socket =
            quic::Socks5QUICUdpSocket::new(udp_socket, raddr, maybe_fake_socket_addr.clone());
        let endpoint = quinn::Endpoint::new_with_abstract_socket(
            self.quic_endpoint_config.clone(),
            None,
            quic_udp_socket,
            runtime,
        )
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("failed to create QUIC endpoint: {}", err),
            )
        })?;
        let quic_connecting = endpoint
            .connect_with(quic_client_config, maybe_fake_socket_addr, server_name)
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!("failed to connect to QUIC server: {}", err),
                )
            })?;
        let quic_connection = quic_connecting.await.map_err(|err| {
            io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("failed to establish QUIC connection: {}", err),
            )
        })?;
        Ok((endpoint, quic_connection))
    }
}

const DEFAULT_BUFFER_SIZE: usize = 65535;

pub(crate) struct Socks5UdpSocket {
    _tcp_stream: super::GenericTcpStream,
    inner: UdpSocket,
    peer_addr: SocketAddr,
}

impl Deref for Socks5UdpSocket {
    type Target = UdpSocket;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Socks5UdpSocket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl AsRef<UdpSocket> for Socks5UdpSocket {
    fn as_ref(&self) -> &UdpSocket {
        &self.inner
    }
}

impl Socks5UdpSocket {
    pub(super) async fn recv_buf_from<B: bytes::BufMut>(
        &self,
        buf: &mut B,
    ) -> io::Result<(usize, super::SocksAddr)> {
        let mut raw_buffer = Vec::with_capacity(DEFAULT_BUFFER_SIZE);
        self.inner.recv_buf_from(&mut raw_buffer).await?;
        let mut raw_buffer = bytes::Bytes::from(raw_buffer);
        let packet = protocol::UdpPacket::decode_from(&mut raw_buffer)?;
        let data_length = packet.data.len();
        buf.put(packet.data);
        Ok((data_length, packet.address))
    }

    pub(super) async fn send_to(&self, buf: &[u8], target: super::SocksAddr) -> io::Result<usize> {
        let data_length = buf.len();
        let packet = protocol::UdpPacketRef {
            address: target,
            data: buf,
        };
        let mut raw_buf = Vec::with_capacity(packet.encode_length());
        packet.encode_to(&mut raw_buf);
        self.inner.send_to(&raw_buf, &self.peer_addr).await?;
        Ok(data_length)
    }
}

#[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
mod quic {
    use std::{
        fmt,
        future::Future,
        io,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        ops::DerefMut,
        pin::Pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    use futures_util::FutureExt;

    pub(super) fn fake_socket_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)), port)
    }

    #[derive(Debug)]
    pub(super) struct Socks5QUICTokioRuntime;

    impl quinn::Runtime for Socks5QUICTokioRuntime {
        fn new_timer(&self, t: std::time::Instant) -> Pin<Box<dyn quinn::AsyncTimer>> {
            Box::pin(tokio::time::sleep_until(t.into()))
        }

        fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) {
            tokio::spawn(future);
        }

        fn wrap_udp_socket(
            &self,
            _sock: std::net::UdpSocket,
        ) -> io::Result<Box<dyn quinn::AsyncUdpSocket>> {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "socks5: wrap udp socket is unsupported",
            ))
        }
    }

    pub(super) struct Socks5QUICUdpSocket {
        inner: Arc<super::Socks5UdpSocket>,
        remote_addr: super::super::SocksAddr,
        maybe_fake_socket_addr: SocketAddr,
        send_futs: Mutex<Vec<Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'static>>>>,
        sent: AtomicUsize,
        recv_fut: Mutex<
            Option<
                Pin<Box<dyn Future<Output = io::Result<(usize, bytes::Bytes)>> + Send + 'static>>,
            >,
        >,
    }

    impl Socks5QUICUdpSocket {
        pub(super) fn new(
            inner: super::Socks5UdpSocket,
            remote_addr: super::super::SocksAddr,
            maybe_fake_socket_addr: SocketAddr,
        ) -> Self {
            Self {
                inner: Arc::new(inner),
                send_futs: Mutex::new(Vec::new()),
                sent: AtomicUsize::new(0),
                recv_fut: Mutex::new(None),
                remote_addr,
                maybe_fake_socket_addr,
            }
        }
    }

    impl fmt::Debug for Socks5QUICUdpSocket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Socks5QUICUdpSocket").finish()
        }
    }

    impl quinn::AsyncUdpSocket for Socks5QUICUdpSocket {
        fn poll_recv(
            &self,
            cx: &mut std::task::Context,
            bufs: &mut [io::IoSliceMut<'_>],
            meta: &mut [quinn::udp::RecvMeta],
        ) -> std::task::Poll<io::Result<usize>> {
            {
                let mut fut = self.recv_fut.lock().unwrap();
                let fut_opt = &mut *fut;
                if let Some(fut) = fut_opt {
                    let poll = fut.poll_unpin(cx);
                    if poll.is_ready() {
                        *fut_opt = None;
                    }
                    if let std::task::Poll::Ready(res) = poll {
                        if let Ok((length, buf)) = &res {
                            let b = bufs[0].deref_mut();
                            b[..buf.len()].copy_from_slice(&buf);
                            meta[0] = quinn::udp::RecvMeta {
                                len: *length,
                                stride: *length,
                                addr: self.maybe_fake_socket_addr.clone(),
                                ecn: None,
                                dst_ip: None,
                            };
                        }
                        return std::task::Poll::Ready(res.map(|_| 1));
                    }
                    return std::task::Poll::Pending;
                }

                let s = self.inner.clone();
                let fut = async move {
                    let mut buf = Vec::with_capacity(super::DEFAULT_BUFFER_SIZE);
                    s.recv_buf_from(&mut buf)
                        .await
                        .map(|(length, _)| (length, bytes::Bytes::from(buf)))
                };
                *fut_opt = Some(Box::pin(fut));
            }
            self.poll_recv(cx, bufs, meta)
        }

        fn poll_send(
            &self,
            state: &quinn::udp::UdpState,
            cx: &mut std::task::Context,
            transmits: &[quinn::udp::Transmit],
        ) -> std::task::Poll<Result<usize, io::Error>> {
            {
                let num_transmits = transmits.len().min(32);
                let mut vec = self.send_futs.lock().unwrap();
                let vec = &mut *vec;
                let empty = vec.is_empty();
                let mut has_error = false;
                vec.retain_mut(|fut| {
                    let poll = fut.poll_unpin(cx);
                    let is_pending = poll.is_pending();
                    if let std::task::Poll::Ready(Err(_)) = poll {
                        has_error = true;
                    }
                    is_pending
                });
                if !empty {
                    if has_error {
                        return std::task::Poll::Ready(Ok(num_transmits.min(1)));
                    }
                    if vec.is_empty() {
                        return std::task::Poll::Ready(Ok(self.sent.load(Ordering::Relaxed)));
                    }
                    return std::task::Poll::Pending;
                }

                let mut sent = 0;

                while sent < transmits.len() {
                    let s = self.inner.clone();
                    let remote_addr = self.remote_addr.clone();
                    let transmit = transmits[sent].clone();
                    let fut =
                        async move { s.send_to(&transmit.contents, remote_addr).await.map(|_| ()) };
                    //
                    vec.push(Box::pin(fut));
                    sent += 1;
                }
                self.sent.store(sent, Ordering::Relaxed);
            }
            self.poll_send(state, cx, transmits)
        }

        fn may_fragment(&self) -> bool {
            false
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            self.inner.local_addr()
        }
    }
}

mod protocol {
    use std::io;

    use bytes::Buf;

    const REPLY_SUCCEEDED: u8 = 0x00;
    const REPLY_GENERAL_FAILURE: u8 = 0x01;
    const REPLY_CONNECTION_NOT_ALLOWED: u8 = 0x02;
    const REPLY_NETWORK_UNREACHABLE: u8 = 0x03;
    const REPLY_HOST_UNREACHABLE: u8 = 0x04;
    const REPLY_CONNECTION_REFUSED: u8 = 0x05;
    const REPLY_TTL_EXPIRED: u8 = 0x06;
    const REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
    const REPLY_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;

    pub(super) fn reply_to_err_string(b: u8) -> String {
        match b {
            REPLY_SUCCEEDED => "succeeded".to_string(),
            REPLY_GENERAL_FAILURE => "general failure".to_string(),
            REPLY_CONNECTION_NOT_ALLOWED => "connection not allowed".to_string(),
            REPLY_NETWORK_UNREACHABLE => "network unreachable".to_string(),
            REPLY_HOST_UNREACHABLE => "host unreachable".to_string(),
            REPLY_CONNECTION_REFUSED => "connection refused".to_string(),
            REPLY_TTL_EXPIRED => "TTL expired".to_string(),
            REPLY_COMMAND_NOT_SUPPORTED => "command not supported".to_string(),
            REPLY_ADDRESS_TYPE_NOT_SUPPORTED => "address type not supported".to_string(),
            _ => format!("unknown reply: {}", b),
        }
    }

    pub(super) trait Encoder {
        fn encode_length(&self) -> usize;

        fn encode_to(&self, buf: impl bytes::BufMut);
    }

    pub(super) trait Decoder: Sized {
        fn decode_from(buf: impl bytes::Buf) -> io::Result<Self>;
    }

    pub(super) struct FirstRequest {
        methods: Vec<u8>,
    }

    impl FirstRequest {
        pub(super) fn new_no_auth() -> Self {
            Self {
                methods: vec![0x00],
            }
        }

        pub(super) fn new_username_password() -> Self {
            Self {
                methods: vec![0x02, 0x00],
            }
        }
    }

    impl Encoder for FirstRequest {
        fn encode_length(&self) -> usize {
            2 + self.methods.len()
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x05);
            buf.put_u8(self.methods.len() as u8);
            buf.put_slice(&self.methods);
        }
    }

    impl Decoder for FirstRequest {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            let version = buf.get_u8();
            if version != 0x05 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid version: {}", version),
                ));
            }

            let methods_len = buf.get_u8() as usize;
            let methods = buf.copy_to_bytes(methods_len).to_vec();

            Ok(Self { methods })
        }
    }

    pub(super) struct FirstResponse {
        method: u8,
    }

    impl FirstResponse {
        pub(super) fn is_username_password(&self) -> bool {
            self.method == 0x02
        }
    }

    impl Encoder for FirstResponse {
        fn encode_length(&self) -> usize {
            2
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x05);
            buf.put_u8(self.method);
        }
    }

    impl Decoder for FirstResponse {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            let version = buf.get_u8();
            if version != 0x05 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid version: {}", version),
                ));
            }

            let method = buf.get_u8();

            Ok(Self { method })
        }
    }

    pub(super) struct UsernamePasswordRequest {
        username: String,
        password: String,
    }

    impl UsernamePasswordRequest {
        pub(super) fn new(username: String, password: String) -> Self {
            Self { username, password }
        }
    }

    impl Encoder for UsernamePasswordRequest {
        fn encode_length(&self) -> usize {
            3 + self.username.len() + self.password.len()
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x01);
            buf.put_u8(self.username.len() as u8);
            buf.put_slice(self.username.as_bytes());
            buf.put_u8(self.password.len() as u8);
            buf.put_slice(self.password.as_bytes());
        }
    }

    impl Decoder for UsernamePasswordRequest {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            let version = buf.get_u8();
            if version != 0x01 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid version: {}", version),
                ));
            }

            let username_len = buf.get_u8() as usize;
            let username =
                String::from_utf8(buf.copy_to_bytes(username_len).to_vec()).map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid username: {}", err),
                    )
                })?;

            let password_len = buf.get_u8() as usize;
            let password =
                String::from_utf8(buf.copy_to_bytes(password_len).to_vec()).map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid password: {}", err),
                    )
                })?;

            Ok(Self { username, password })
        }
    }

    pub(super) struct UsernamePasswordResponse {
        status: u8,
    }

    impl UsernamePasswordResponse {
        pub(super) fn is_success(&self) -> bool {
            self.status == 0x00
        }
    }

    impl Encoder for UsernamePasswordResponse {
        fn encode_length(&self) -> usize {
            2
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x01);
            buf.put_u8(self.status);
        }
    }

    impl Decoder for UsernamePasswordResponse {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            let version = buf.get_u8();
            if version != 0x01 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid version: {}", version),
                ));
            }

            let status = buf.get_u8();

            Ok(Self { status })
        }
    }

    pub(super) struct CommandRequest {
        pub(super) command: u8,
        pub(super) address: super::super::SocksAddr,
    }

    impl Encoder for CommandRequest {
        fn encode_length(&self) -> usize {
            3 + self.address.encode_length()
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x05);
            buf.put_u8(self.command);
            buf.put_u8(0x00);
            self.address.encode_to(&mut buf);
        }
    }

    impl Decoder for CommandRequest {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            let version = buf.get_u8();
            if version != 0x05 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid version: {}", version),
                ));
            }

            let command = buf.get_u8();
            let _ = buf.get_u8();
            let address_buf = buf.copy_to_bytes(buf.remaining());
            let address = super::super::SocksAddr::decode_from(&address_buf).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid address: {}", err),
                )
            })?;

            Ok(Self { command, address })
        }
    }

    pub(super) struct CommandResponse {
        pub(super) reply: u8,
        pub(super) address: super::super::SocksAddr,
    }

    impl CommandResponse {
        pub(super) fn is_success(&self) -> bool {
            self.reply == 0x00
        }
    }

    impl Encoder for CommandResponse {
        fn encode_length(&self) -> usize {
            3 + self.address.encode_length()
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x05);
            buf.put_u8(self.reply);
            buf.put_u8(0x00);
            self.address.encode_to(&mut buf);
        }
    }

    impl Decoder for CommandResponse {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            let version = buf.get_u8();
            if version != 0x05 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid version: {}", version),
                ));
            }

            let reply = buf.get_u8();
            let _ = buf.get_u8();
            let address_buf = buf.copy_to_bytes(buf.remaining());
            let address = super::super::SocksAddr::decode_from(&address_buf).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid address: {}", err),
                )
            })?;

            Ok(Self { reply, address })
        }
    }

    // Udp Packet

    pub(super) struct UdpPacketRef<'a> {
        pub(super) address: super::super::SocksAddr,
        pub(super) data: &'a [u8],
    }

    impl<'a> Encoder for UdpPacketRef<'a> {
        fn encode_length(&self) -> usize {
            3 + self.address.encode_length() + self.data.len()
        }

        fn encode_to(&self, mut buf: impl bytes::BufMut) {
            buf.put_u8(0x00);
            buf.put_u8(0x00);
            buf.put_u8(0x00);
            self.address.encode_to(&mut buf);
            buf.put_slice(self.data);
        }
    }

    pub(super) struct UdpPacket {
        pub(super) address: super::super::SocksAddr,
        pub(super) data: bytes::Bytes,
    }

    impl Decoder for UdpPacket {
        fn decode_from(mut buf: impl bytes::Buf) -> io::Result<Self> {
            buf.advance(3);
            let mut buf = buf.copy_to_bytes(buf.remaining());
            let address = super::super::SocksAddr::decode_from(&buf).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid address: {}", err),
                )
            })?;
            buf.advance(address.encode_length());
            let data = buf.copy_to_bytes(buf.remaining());

            Ok(Self { address, data })
        }
    }
}

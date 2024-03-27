use std::{
    error::Error,
    fmt,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use base64::Engine;
use bytes::{Buf, BufMut};
use hickory_proto::op::Message;
use http_body_util::BodyExt;

use crate::{adapter, debug, error, info, log, option};

use super::network;

const HTTPS_UPSTREAM_TYPE: &str = "https";

const DEFAULT_PATH: &str = "/dns-query";

pub(crate) struct HTTPSUpstream {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    host: String,
    path: String,
    header: http::HeaderMap,
    use_post: bool,
    client: GenericHTTPSClient,
}

impl HTTPSUpstream {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Arc<Box<dyn log::Logger>>,
        tag: String,
        options: option::HTTPSUpstreamOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let address = network::SocksAddr::parse_with_default_port(&options.address, 443)?;
        let dialer = network::Dialer::new(manager.clone(), options.dialer.unwrap_or_default())?;
        let (tls_client_config, server_name) = super::new_tls_config(options.tls)?;
        let server_name = match server_name {
            Some(s) => s,
            None => match &address {
                network::SocksAddr::DomainAddr(domain, _) => domain.clone(),
                network::SocksAddr::SocketAddr(addr) => addr.ip().to_string(),
            },
        };
        let host = options.host.unwrap_or(server_name.clone());
        let server_name = match rustls::ServerName::try_from(server_name.as_str()) {
            Ok(v) => v,
            Err(e) => return Err(format!("invalid server-name: {}", e).into()),
        };
        let bootstrap = if address.is_domain_addr() {
            if let Some(bootstrap_options) = options.bootstrap {
                Some(super::Bootstrap::new(manager.clone(), bootstrap_options))
            } else {
                None
            }
        } else {
            None
        };
        if address.is_domain_addr() && bootstrap.is_none() && !dialer.domain_support() {
            return Err("domain address not supported, because dialer is unsupported, and bootstrap is not set".into());
        }
        let path = options.path.unwrap_or(DEFAULT_PATH.to_owned());
        let host = {
            let port = address.port();
            if port == 443 {
                if let Ok(ip) = host.parse::<IpAddr>() {
                    if ip.is_ipv4() {
                        ip.to_string()
                    } else {
                        format!("[{}]", ip)
                    }
                } else {
                    host.clone()
                }
            } else {
                if let Ok(ip) = host.parse::<IpAddr>() {
                    SocketAddr::new(ip, port).to_string()
                } else {
                    format!("{}:{}", &host, port)
                }
            }
        };
        let mut header = http::HeaderMap::new();
        if let Some(map) = options.header {
            for (k, v) in map {
                let key = http::HeaderName::try_from(&k)
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("invalid header key: {}, {}", &k, err).into()
                    })?;
                let vaule = http::HeaderValue::try_from(&v)
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("invalid header value: {}, {}", &v, err).into()
                    })?;
                header.insert(key, vaule);
            }
        }
        header.insert(
            http::header::CONTENT_TYPE,
            "application/dns-message".parse().unwrap(),
        );
        header.insert(
            http::header::ACCEPT,
            "application/dns-message".parse().unwrap(),
        );
        let client = if !options.use_http3 {
            GenericHTTPSClient::from(HTTPSClient::new(
                logger.clone(),
                address,
                tls_client_config,
                server_name,
                options
                    .idle_timeout
                    .unwrap_or_else(|| super::DEFAULT_IDLE_TIMEOUT),
                bootstrap,
                dialer,
            ))
        } else {
            let client: Option<GenericHTTPSClient> = {
                cfg_if::cfg_if! {
                    if #[cfg(all(feature = "upstream-https-support", feature = "upstream-quic-support", feature = "upstream-tls-support"))] {
                        Some(GenericHTTPSClient::from(http3::HTTP3Client::new(
                            logger.clone(),
                            address,
                            tls_client_config,
                            server_name,
                            options
                                .idle_timeout
                                .unwrap_or_else(|| super::DEFAULT_IDLE_TIMEOUT),
                            bootstrap,
                            dialer,
                        )))
                    } else {
                        None
                    }
                }
            };
            if client.is_none() {
                return Err("http3 is not supported".into());
            }
            client.unwrap()
        };
        Ok(Self {
            manager,
            logger,
            tag,
            host,
            path,
            header,
            use_post: options.use_post,
            client,
        })
    }

    async fn exchange_wrapper(
        &self,
        request: &Message,
    ) -> Result<Message, Box<dyn Error + Send + Sync>> {
        let request_bytes = request
            .to_vec()
            .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("serialize request failed: {}", err).into()
            })?;
        let response = if self.use_post {
            let request_bytes = bytes::Bytes::from(request_bytes);
            let uri = http::Uri::builder()
                .scheme("https")
                .authority(self.host.as_bytes())
                .path_and_query(&self.path)
                .build()
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("invalid uri: {}", err).into()
                })?;
            let mut request_builder = http::Request::builder().uri(uri).method(http::Method::POST);
            for (k, v) in self.header.iter() {
                request_builder = request_builder.header(k, v.clone());
            }
            let request = request_builder.body(request_bytes)?;
            self.client.post(request).await?
        } else {
            let mut request_buf = String::new();
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode_string(&request_bytes, &mut request_buf);
            let uri = http::Uri::builder()
                .scheme("https")
                .authority(self.host.as_bytes())
                .path_and_query({
                    let path = &self.path;
                    if path.contains('?') {
                        format!("{}&dns={}", path, request_buf)
                    } else {
                        format!("{}?dns={}", path, request_buf)
                    }
                })
                .build()
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("invalid uri: {}", err).into()
                })?;
            let mut request_builder = http::Request::builder().uri(uri).method(http::Method::GET);
            for (k, v) in self.header.iter() {
                request_builder = request_builder.header(k, v.clone());
            }
            let request = request_builder.body(())?;
            self.client.get(request).await?
        };
        if response.status() != http::StatusCode::OK {
            return Err(format!("response status: {}", response.status()).into());
        }
        let result = Message::from_vec(&response.body())
            .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("deserialize response failed: {}", err).into()
            });
        result
    }
}

#[async_trait::async_trait]
impl adapter::Common for HTTPSUpstream {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.client.start().await?;
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.client.close().await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::Upstream for HTTPSUpstream {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        HTTPS_UPSTREAM_TYPE
    }

    fn dependencies(&self) -> Option<Vec<String>> {
        self.client.dependency().map(|v| vec![v])
    }

    fn logger(&self) -> &Arc<Box<dyn log::Logger>> {
        &self.logger
    }

    async fn exchange(
        &self,
        log_tracker: Option<&log::Tracker>,
        request: &mut Message,
    ) -> Result<Message, Box<dyn Error + Send + Sync>> {
        let query_info = super::show_query(&request);
        info!(
            self.logger,
            { option_tracker = log_tracker },
            "exchange {}",
            &query_info
        );
        let old_id = request.id();
        request.set_id(0);
        let mut res = self.exchange_wrapper(request).await;
        match &mut res {
            Ok(v) => {
                info!(
                    self.logger,
                    { option_tracker = log_tracker },
                    "exchange {} success",
                    &query_info
                );
                request.set_id(old_id);
                v.set_id(old_id);
            }
            Err(e) => {
                error!(
                    self.logger,
                    { option_tracker = log_tracker },
                    "exchange {} failed: {}",
                    &query_info,
                    e.to_string()
                );
                request.set_id(old_id);
            }
        }

        #[cfg(feature = "api")]
        {
            if let Some(m) = self
                .manager
                .get_state_map()
                .try_get::<super::UpstreamStatisticDataMap>()
            {
                m.add_record(self.tag(), self.r#type(), res.is_ok());
            }
        }

        res
    }
}

// Resolver //

enum GenericHTTPSClient {
    HTTPS(HTTPSClient),

    #[cfg(all(
        feature = "upstream-https-support",
        feature = "upstream-quic-support",
        feature = "upstream-tls-support"
    ))]
    HTTP3(http3::HTTP3Client),
}

impl From<HTTPSClient> for GenericHTTPSClient {
    fn from(value: HTTPSClient) -> Self {
        Self::HTTPS(value)
    }
}

#[cfg(all(
    feature = "upstream-https-support",
    feature = "upstream-quic-support",
    feature = "upstream-tls-support"
))]
impl From<http3::HTTP3Client> for GenericHTTPSClient {
    fn from(value: http3::HTTP3Client) -> Self {
        Self::HTTP3(value)
    }
}

impl GenericHTTPSClient {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self {
            Self::HTTPS(client) => client.start().await,

            #[cfg(all(
                feature = "upstream-https-support",
                feature = "upstream-quic-support",
                feature = "upstream-tls-support"
            ))]
            Self::HTTP3(client) => client.start().await,
        }
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self {
            Self::HTTPS(client) => client.close().await,

            #[cfg(all(
                feature = "upstream-https-support",
                feature = "upstream-quic-support",
                feature = "upstream-tls-support"
            ))]
            Self::HTTP3(client) => client.close().await,
        }
    }

    fn dependency(&self) -> Option<String> {
        match self {
            Self::HTTPS(client) => client.dependency(),

            #[cfg(all(
                feature = "upstream-https-support",
                feature = "upstream-quic-support",
                feature = "upstream-tls-support"
            ))]
            Self::HTTP3(client) => client.dependency(),
        }
    }

    async fn post(
        &self,
        req: http::Request<bytes::Bytes>,
    ) -> Result<http::Response<bytes::Bytes>, Box<dyn Error + Send + Sync>> {
        match self {
            Self::HTTPS(client) => {
                let (parts, body) = req.into_parts();
                let response = client
                    .request(http::Request::from_parts(
                        parts,
                        http::Response::new(http_body_util::Full::new(body)),
                    ))
                    .await?;
                let (parts, incoming) = response.into_parts();
                let body = incoming.collect().await?.to_bytes();
                Ok(http::Response::from_parts(parts, body))
            }

            #[cfg(all(
                feature = "upstream-https-support",
                feature = "upstream-quic-support",
                feature = "upstream-tls-support"
            ))]
            Self::HTTP3(client) => {
                let (parts, req_body) = req.into_parts();
                let req = http::Request::from_parts(parts, ());
                let mut http3_connection = client.get_http3_connection().await?;
                let mut req_stream = http3_connection.send_request(req).await?;
                req_stream.send_data(req_body).await?;
                req_stream.finish().await?;
                let response = req_stream.recv_response().await?;
                let (parts, _) = response.into_parts();
                let mut response_body = bytes::BytesMut::new();
                while let Some(data) = req_stream.recv_data().await? {
                    response_body.put(data);
                }
                let response_body = response_body.copy_to_bytes(response_body.len());
                Ok(http::Response::from_parts(parts, response_body))
            }
        }
    }

    async fn get(
        &self,
        req: http::Request<()>,
    ) -> Result<http::Response<bytes::Bytes>, Box<dyn Error + Send + Sync>> {
        match self {
            Self::HTTPS(client) => {
                let (parts, _) = req.into_parts();
                let response = client
                    .request(http::Request::from_parts(
                        parts,
                        http::Response::new(http_body_util::Full::default()),
                    ))
                    .await?;
                let (parts, incoming) = response.into_parts();
                let body = incoming.collect().await?.to_bytes();
                Ok(http::Response::from_parts(parts, body))
            }

            #[cfg(all(
                feature = "upstream-https-support",
                feature = "upstream-quic-support",
                feature = "upstream-tls-support"
            ))]
            Self::HTTP3(client) => {
                let mut http3_connection = client.get_http3_connection().await?;
                let mut req_stream = http3_connection.send_request(req).await?;
                req_stream.finish().await?;
                let response = req_stream.recv_response().await?;
                let (parts, _) = response.into_parts();
                let mut response_body = bytes::BytesMut::new();
                while let Some(data) = req_stream.recv_data().await? {
                    response_body.put(data);
                }
                let response_body = response_body.copy_to_bytes(response_body.len());
                Ok(http::Response::from_parts(parts, response_body))
            }
        }
    }
}

// HTTPS

#[derive(Clone)]
struct HTTPSConnector {
    logger: Arc<Box<dyn log::Logger>>,
    address: network::SocksAddr,
    tls_connector: tokio_rustls::TlsConnector,
    server_name: rustls::ServerName,
    bootstrap: Arc<Option<super::Bootstrap>>,
    dialer: Arc<network::Dialer>,
}

impl fmt::Debug for HTTPSConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HTTPSConnector").finish()
    }
}

impl HTTPSConnector {
    fn new(
        logger: Arc<Box<dyn log::Logger>>,
        address: network::SocksAddr,
        mut tls_client_config: tokio_rustls::rustls::ClientConfig,
        server_name: rustls::ServerName,
        bootstrap: Option<super::Bootstrap>,
        dialer: network::Dialer,
    ) -> Self {
        tls_client_config.alpn_protocols = vec![b"dns".to_vec(), b"h2".to_vec()];
        //let quic_client_config = quinn::ClientConfig::new(Arc::new(tls_client_config));
        Self {
            logger,
            address,
            tls_connector: tokio_rustls::TlsConnector::from(Arc::new(tls_client_config)),
            server_name,
            bootstrap: Arc::new(bootstrap),
            dialer: Arc::new(dialer),
        }
    }

    async fn new_tcp_stream(&self) -> io::Result<network::GenericTcpStream> {
        super::Bootstrap::dial_with_bootstrap(
            &self.logger,
            self.address.clone(),
            &self.bootstrap,
            self.dialer.clone(),
            |address, dialer| async move { dialer.new_tcp_stream(address).await },
            |ips, port, dialer| async move { dialer.parallel_new_tcp_stream(ips, port).await },
        )
        .await
    }

    async fn new_tls_stream(
        &self,
    ) -> io::Result<tokio_rustls::client::TlsStream<network::GenericTcpStream>> {
        debug!(self.logger, "connect to {}", self.address);
        let tcp_stream = self.new_tcp_stream().await?;
        self.tls_connector
            .connect(self.server_name.clone(), tcp_stream)
            .await
            .map(|s| {
                debug!(self.logger, "new tcp stream");
                s
            })
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("handshake tls stream failed: {}", err),
                )
            })
    }
}

impl tower_service::Service<http::Uri> for HTTPSConnector {
    type Response = HTTPSConnection<
        hyper_util::rt::TokioIo<
            super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
        >,
    >;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: http::Uri) -> Self::Future {
        let s = self.clone();
        Box::pin(async move {
            let tls_stream = s.new_tls_stream().await?;
            debug!(s.logger, "new https stream");
            Ok(HTTPSConnection::from(hyper_util::rt::TokioIo::new(
                super::LogStream::new(s.logger.clone(), "close https stream", tls_stream),
            )))
        })
    }
}

struct HTTPSConnection<T>(T);

impl
    From<
        hyper_util::rt::TokioIo<
            super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
        >,
    >
    for HTTPSConnection<
        hyper_util::rt::TokioIo<
            super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
        >,
    >
{
    fn from(
        value: hyper_util::rt::TokioIo<
            super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
        >,
    ) -> Self {
        Self(value)
    }
}

impl<T: hyper::rt::Read + Unpin> hyper::rt::Read for HTTPSConnection<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<T: hyper::rt::Write + Unpin> hyper::rt::Write for HTTPSConnection<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }
}

impl<T> hyper_util::client::legacy::connect::Connection for HTTPSConnection<T> {
    fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
        hyper_util::client::legacy::connect::Connected::new()
    }
}

struct HTTPSClient {
    bootstrap: Arc<Option<super::Bootstrap>>,
    dialer: Arc<network::Dialer>,
    inner: hyper_util::client::legacy::Client<
        HTTPSConnector,
        http::Response<http_body_util::Full<bytes::Bytes>>,
    >,
}

impl HTTPSClient {
    fn new(
        logger: Arc<Box<dyn log::Logger>>,
        address: network::SocksAddr,
        tls_client_config: tokio_rustls::rustls::ClientConfig,
        server_name: rustls::ServerName,
        idle_timeout: Duration,
        bootstrap: Option<super::Bootstrap>,
        dialer: network::Dialer,
    ) -> Self {
        let connector = HTTPSConnector::new(
            logger,
            address,
            tls_client_config,
            server_name,
            bootstrap,
            dialer,
        );
        let mut client_builder =
            hyper_util::client::legacy::Builder::new(hyper_util::rt::TokioExecutor::default());
        client_builder.pool_idle_timeout(idle_timeout);
        client_builder.http2_only(true);
        let bootstrap = connector.bootstrap.clone();
        let dialer = connector.dialer.clone();
        let client = client_builder.build(connector);
        Self {
            bootstrap,
            dialer,
            inner: client,
        }
    }

    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.dialer.start().await;
        if let Some(bootstrap) = self.bootstrap.as_ref() {
            bootstrap.start().await?;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.dialer.close().await;
        Ok(())
    }

    fn dependency(&self) -> Option<String> {
        if let Some(bootstrap) = self.bootstrap.as_ref() {
            return Some(bootstrap.upstream_tag().to_string());
        }
        None
    }
}

impl Deref for HTTPSClient {
    type Target = hyper_util::client::legacy::Client<
        HTTPSConnector,
        http::Response<http_body_util::Full<bytes::Bytes>>,
    >;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for HTTPSClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

// HTTP3

#[cfg(all(
    feature = "upstream-https-support",
    feature = "upstream-quic-support",
    feature = "upstream-tls-support"
))]
mod http3 {
    use std::{
        error::Error,
        future,
        ops::{Deref, DerefMut},
        sync::{
            atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use tokio::{
        sync::{Mutex, RwLock},
        task::JoinHandle,
    };
    use tokio_util::sync::CancellationToken;

    pub(super) struct HTTP3Client {
        logger: Arc<Box<dyn super::log::Logger>>,
        address: super::network::SocksAddr,
        quic_client_config: quinn::ClientConfig,
        server_name: rustls::ServerName,
        idle_timeout: Duration,
        bootstrap: Option<super::super::Bootstrap>,
        dialer: Arc<super::network::Dialer>,
        //
        last_use: Arc<AtomicI64>,
        connection: Arc<RwLock<Option<HTTP3Connection>>>,
        handler: Mutex<Option<JoinHandle<()>>>,
    }

    impl HTTP3Client {
        pub(super) fn new(
            logger: Arc<Box<dyn super::log::Logger>>,
            address: super::network::SocksAddr,
            mut tls_client_config: tokio_rustls::rustls::ClientConfig,
            server_name: rustls::ServerName,
            idle_timeout: Duration,
            bootstrap: Option<super::super::Bootstrap>,
            dialer: super::network::Dialer,
        ) -> Self {
            tls_client_config.alpn_protocols = vec![b"dns".to_vec(), b"h3".to_vec()];
            let quic_client_config = quinn::ClientConfig::new(Arc::new(tls_client_config));
            Self {
                logger,
                address,
                quic_client_config,
                server_name,
                idle_timeout,
                bootstrap,
                dialer: Arc::new(dialer),
                last_use: Arc::new(AtomicI64::new(0)),
                connection: Arc::new(RwLock::new(None)),
                handler: Mutex::new(None),
            }
        }

        pub(super) async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.dialer.start().await;
            if let Some(bootstrap) = self.bootstrap.as_ref() {
                bootstrap.start().await?;
            }
            let connection = self.connection.clone();
            let last_use = self.last_use.clone();
            let idle_timeout = self.idle_timeout;
            let handler =
                tokio::spawn(async move { Self::handle(connection, last_use, idle_timeout).await });
            self.handler.lock().await.replace(handler);
            Ok(())
        }

        pub(super) async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
            if let Some(handler) = self.handler.lock().await.take() {
                handler.abort();
            }
            if let Some(connection) = self.connection.write().await.take() {
                connection.set_close_tag();
            }
            self.dialer.close().await;
            Ok(())
        }

        pub(super) fn dependency(&self) -> Option<String> {
            if let Some(bootstrap) = self.bootstrap.as_ref() {
                return Some(bootstrap.upstream_tag().to_string());
            }
            None
        }

        async fn handle(
            connection: Arc<RwLock<Option<HTTP3Connection>>>,
            last_use: Arc<AtomicI64>,
            idle_timeout: Duration,
        ) {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        let now = chrono::Local::now().timestamp();
                        let last = last_use.load(Ordering::Relaxed);
                        let mut connection = connection.write().await;
                        if last == 0 || now - last > idle_timeout.as_secs() as i64 {
                            connection.take();
                            continue;
                        }
                        if let Some(c) = connection.as_mut() {
                            if c.is_closed() {
                                connection.take();
                            }
                        }
                    }
                }
            }
        }

        fn flush_last_use(&self) {
            self.last_use
                .store(chrono::Local::now().timestamp(), Ordering::Relaxed);
        }

        fn get_server_name_str(&self) -> String {
            match &self.server_name {
                rustls::ServerName::DnsName(domain) => domain.as_ref().to_string(),
                rustls::ServerName::IpAddress(ip) => ip.to_string(),
                _ => unreachable!(),
            }
        }

        pub(super) async fn get_http3_connection(
            &self,
        ) -> Result<HTTP3Connection, Box<dyn Error + Send + Sync>> {
            let mut connection = self.connection.read().await.clone();
            if let Some(connection) = connection.take() {
                if !connection.is_closed() {
                    self.flush_last_use();
                    return Ok(connection);
                }
            }
            let (quic_endpoint, quic_connection) = super::super::Bootstrap::dial_with_bootstrap(
                &self.logger,
                self.address.clone(),
                &self.bootstrap,
                (
                    self.dialer.clone(),
                    self.quic_client_config.clone(),
                    self.get_server_name_str(),
                ),
                |address, (dialer, quic_client_config, server_name)| async move {
                    dialer
                        .new_quic_connection(address, quic_client_config, &server_name)
                        .await
                },
                |ips, port, (dialer, quic_client_config, server_name)| async move {
                    dialer
                        .parallel_new_quic_connection(ips, port, quic_client_config, &server_name)
                        .await
                },
            )
            .await?;
            super::debug!(self.logger, "new http3 connection");
            let http3_connection = HTTP3Connection::connect(
                self.logger.clone(),
                quic_endpoint,
                h3_quinn::Connection::new(quic_connection),
            )
            .await?;
            self.connection
                .write()
                .await
                .replace(http3_connection.clone());
            self.flush_last_use();
            Ok(http3_connection)
        }
    }

    pub(super) struct HTTP3Connection {
        token: CancellationToken,
        close_tag: Arc<AtomicBool>,
        n: Arc<AtomicUsize>,
        send_request: h3::client::SendRequest<h3_quinn::OpenStreams, bytes::Bytes>,
    }

    impl HTTP3Connection {
        async fn connect(
            logger: Arc<Box<dyn super::log::Logger>>,
            endpoint: quinn::Endpoint,
            connection: h3_quinn::Connection,
        ) -> Result<Self, h3::Error> {
            let (mut connection, send_request) = h3::client::new(connection).await?;
            let token = CancellationToken::new();
            let token_driver = token.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = future::poll_fn(|cx| connection.poll_close(cx)) => {
                        token_driver.cancel();
                    }
                    _ = endpoint.wait_idle() => {
                        token_driver.cancel();
                        super::debug!(logger, "close http3 connection");
                        return;
                    }
                    _ = token_driver.cancelled() => {
                        connection.shutdown(0).await.ok();
                    }
                }
                endpoint.close(0u8.into(), b"");
                endpoint.wait_idle().await;
                super::debug!(logger, "close http3 connection");
            });
            Ok(Self {
                token,
                n: Arc::new(AtomicUsize::new(1)),
                close_tag: Arc::new(AtomicBool::new(false)),
                send_request,
            })
        }

        pub(super) fn set_close_tag(&self) {
            self.close_tag.store(true, Ordering::Relaxed);
        }

        pub(super) fn is_closed(&self) -> bool {
            self.token.is_cancelled() || self.close_tag.load(Ordering::Relaxed)
        }
    }

    impl Clone for HTTP3Connection {
        fn clone(&self) -> Self {
            self.n.fetch_add(1, Ordering::Relaxed);
            Self {
                token: self.token.clone(),
                n: self.n.clone(),
                close_tag: self.close_tag.clone(),
                send_request: self.send_request.clone(),
            }
        }
    }

    impl Drop for HTTP3Connection {
        fn drop(&mut self) {
            if self.n.fetch_sub(1, Ordering::Relaxed) == 1 {
                self.close_tag.store(true, Ordering::Relaxed);
            }
            if self.close_tag.load(Ordering::Relaxed) {
                self.token.cancel();
            }
        }
    }

    impl Deref for HTTP3Connection {
        type Target = h3::client::SendRequest<h3_quinn::OpenStreams, bytes::Bytes>;

        fn deref(&self) -> &Self::Target {
            &self.send_request
        }
    }

    impl DerefMut for HTTP3Connection {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.send_request
        }
    }

    unsafe impl Send for HTTP3Connection {}
    unsafe impl Sync for HTTP3Connection {}
}

use std::{
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use hickory_proto::op::Message;
use tokio::{net::TcpListener, sync::Mutex};

use crate::{adapter, common, error, fatal, info, log, option};

const HTTP_LISTENER_TYPE: &str = "http";

enum HTTPListenerType {
    HTTP,

    #[cfg(all(feature = "listener-https-support", feature = "listener-tls-support"))]
    HTTPS(Arc<tokio_rustls::rustls::ServerConfig>),

    #[cfg(all(
        feature = "listener-https-support",
        feature = "listener-quic-support",
        feature = "listener-tls-support"
    ))]
    HTTP3(Arc<tokio_rustls::rustls::ServerConfig>),
}

pub(crate) struct HTTPListener {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    listen: SocketAddr,
    workflow_tag: String,
    query_timeout: Option<Duration>,
    listener_type: HTTPListenerType,
    canceller: Mutex<Option<common::Canceller>>,
}

impl HTTPListener {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::HTTPListenerOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let listen = super::parse_with_default_port(&options.listen, 53).map_err::<Box<
            dyn Error + Send + Sync,
        >, _>(|err| {
            format!("invalid listen: {}", err).into()
        })?;
        if options.generic.workflow.is_empty() {
            return Err("missing workflow".into());
        }
        let mut _listener_type = None;
        cfg_if::cfg_if! {
            if #[cfg(all(feature = "listener-https-support", feature = "listener-quic-support", feature = "listener-tls-support"))] {
                if options.use_http3 {
                    match options.tls {
                        Some(tls_options) => {
                            let mut tls_config = super::new_tls_config(tls_options)?;
                            tls_config.alpn_protocols = vec![b"dns".into()];
                            _listener_type = Some(HTTPListenerType::HTTP3(Arc::new(tls_config)));
                        }
                        None => {
                            return Err("missing tls options".into());
                        }
                    }
                } else {
                    match options.tls {
                        Some(tls_options) => {
                            let mut tls_config = super::new_tls_config(tls_options)?;
                            tls_config.alpn_protocols = vec![b"dns".into()];
                            _listener_type = Some(HTTPListenerType::HTTPS(Arc::new(tls_config)));
                        }
                        None => {
                            _listener_type = Some(HTTPListenerType::HTTP);
                        }
                    }
                }
            } else if #[cfg(all(feature = "listener-https-support", feature = "listener-tls-support"))] {
                if options.use_http3 {
                    return Err("http3 not supported".into());
                }
                match options.tls {
                    Some(tls_options) => {
                        let mut tls_config = super::new_tls_config(tls_options)?;
                        tls_config.alpn_protocols = vec![b"dns".into()];
                        _listener_type = Some(HTTPListenerType::HTTPS(Arc::new(tls_config)));
                    }
                    None => {
                        _listener_type = Some(HTTPListenerType::HTTP);
                    }
                }
            } else {
                if options.use_http3 {
                    return Err("http3 not supported".into());
                }
                if options.tls.is_some() {
                    return Err("tls not supported".into());
                }
                _listener_type = Some(HTTPListenerType::HTTP);
            }
        }
        Ok(Self {
            manager,
            logger: Arc::new(logger),
            tag,
            listen,
            workflow_tag: options.generic.workflow,
            query_timeout: options.generic.query_timeout,
            listener_type: _listener_type.unwrap(),
            canceller: Mutex::new(None),
        })
    }

    // HTTP
    async fn http_handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        service: Arc<HTTPService>,
        listener_tag: String,
        tcp_listener: TcpListener,
        canceller_guard: common::CancellerGuard,
    ) {
        let conn_builder = Arc::new(hyper_util::server::conn::auto::Builder::new(
            hyper_util::rt::TokioExecutor::default(),
        ));
        let (mut http_canceller, http_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = tcp_listener.accept() => {
                match res {
                  Ok((stream, peer_addr)) => {
                    // TODO: Check Valid
                    let service = service.clone();
                    let conn_builder = conn_builder.clone();
                    let http_canceller_guard = http_canceller_guard.clone();
                    tokio::spawn(async move {
                        let stream = hyper_util::rt::TokioIo::new(stream);
                        let fut = conn_builder.serve_connection(stream, HTTPRequestState::new(service, peer_addr));
                        tokio::select! {
                            _ = fut => {}
                            _ = http_canceller_guard.cancelled() => {}
                        }
                    });
                  }
                  Err(e) => {
                    if !canceller_guard.is_cancelled() {
                        fatal!(service.logger(), "failed to accept HTTP connection: {}", e);
                        manager.fail_to_close(format!("listener [{}]: failed to accept HTTP connection: {}", listener_tag, e)).await;
                    }
                    break;
                  }
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
        drop(http_canceller_guard);
        http_canceller.cancel_and_wait().await;
    }

    // HTTPS
    #[cfg(all(feature = "listener-https-support", feature = "listener-tls-support"))]
    async fn https_handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        service: Arc<HTTPService>,
        listener_tag: String,
        tcp_listener: TcpListener,
        tls_acceptor: Arc<tokio_rustls::TlsAcceptor>,
        canceller_guard: common::CancellerGuard,
    ) {
        let conn_builder = Arc::new(hyper_util::server::conn::auto::Builder::new(
            hyper_util::rt::TokioExecutor::default(),
        ));
        let (mut http_canceller, http_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = tcp_listener.accept() => {
                match res {
                  Ok((stream, peer_addr)) => {
                    // TODO: Check Valid
                    let service = service.clone();
                    let conn_builder = conn_builder.clone();
                    let tls_acceptor = tls_acceptor.clone();
                    let http_canceller_guard = http_canceller_guard.clone();
                    tokio::spawn(async move {
                        let stream = tokio::select! {
                            res = tls_acceptor.accept(stream) => {
                                match res {
                                    Ok(v) => v,
                                    Err(e) => {
                                        error!(
                                            service.logger(),
                                            "invalid tls stream: {}, peer_addr: {}", e, peer_addr,
                                        );
                                        return;
                                    }
                                }
                            }
                            _ = http_canceller_guard.cancelled() => {
                                return;
                            }
                        };
                        let stream = hyper_util::rt::TokioIo::new(stream);
                        let fut = conn_builder.serve_connection(stream, HTTPRequestState::new(service, peer_addr));
                        tokio::select! {
                            _ = fut => {}
                            _ = http_canceller_guard.cancelled() => {}
                        }
                    });
                  }
                  Err(e) => {
                    if !canceller_guard.is_cancelled() {
                        fatal!(service.logger(), "failed to accept HTTPS connection: {}", e);
                        manager.fail_to_close(format!("listener [{}]: failed to accept HTTPS connection: {}", listener_tag, e)).await;
                    }
                    break;
                  }
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
        drop(http_canceller_guard);
        http_canceller.cancel_and_wait().await;
    }

    // HTTP3
    #[cfg(all(
        feature = "listener-https-support",
        feature = "listener-quic-support",
        feature = "listener-tls-support"
    ))]
    async fn http3_handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        service: Arc<HTTPService>,
        listener_tag: String,
        endpoint: quinn::Endpoint,
        canceller_guard: common::CancellerGuard,
    ) {
        let (mut http_canceller, http_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = endpoint.accept() => {
                match res {
                    Some(c) => {
                        let service = service.clone();
                        let http_canceller_guard = http_canceller_guard.clone();
                        tokio::spawn(async move {
                            let conn = tokio::select! {
                                res = c => {
                                    match res {
                                        Ok(c) => c,
                                        Err(_) => {
                                            return;
                                        }
                                    }
                                }
                                _ = http_canceller_guard.cancelled() => {
                                    return;
                                }
                            };
                            Self::http3_conn_handle(service, conn, http_canceller_guard).await;
                        });
                    }
                    None => {
                        if !canceller_guard.is_cancelled() {
                            fatal!(service.logger(), "failed to accept HTTP3 connection");
                            manager.fail_to_close(format!("listener [{}]: failed to accept HTTP3 connection", listener_tag)).await;
                        }
                        break;
                    }
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
        drop(http_canceller_guard);
        http_canceller.cancel_and_wait().await;
        endpoint.close(0u32.into(), b"");
    }

    // HTTP3: connection handle
    #[cfg(all(
        feature = "listener-https-support",
        feature = "listener-quic-support",
        feature = "listener-tls-support"
    ))]
    async fn http3_conn_handle(
        service: Arc<HTTPService>,
        connection: quinn::Connection,
        canceller_guard: common::CancellerGuard,
    ) {
        let peer_addr = connection.remote_address();
        let mut h3_connection: h3::server::Connection<_, bytes::Bytes> = tokio::select! {
            res = h3::server::Connection::new(h3_quinn::Connection::new(connection.clone())) => {
                match res {
                    Ok(v) => v,
                    Err(e) => {
                        error!(service.logger(), "failed to handshake HTTP3 connection: {}, peer_addr: {}", e, peer_addr);
                        connection.close(0u32.into(), b"");
                        return;
                    }
                }
            }
            _ = canceller_guard.cancelled() => {
                connection.close(0u32.into(), b"");
                return;
            }
        };
        let (mut conn_canceller, conn_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
                res = h3_connection.accept() => {
                    match res {
                        Ok(Some((req, req_stream))) => {
                            let service = service.clone();
                            let peer_addr = peer_addr.clone();
                            let conn_canceller_guard = conn_canceller_guard.clone();
                            tokio::spawn(async move {
                                Self::http3_request_handle(service, peer_addr, req, req_stream, conn_canceller_guard).await;
                            });
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!(service.logger(), "failed to accept HTTP3 stream: {}, peer_addr: {}", e, peer_addr);
                            break;
                        }
                    }
                }
                _ = canceller_guard.cancelled() => {
                    break;
                }
            }
        }
        drop(conn_canceller_guard);
        conn_canceller.cancel_and_wait().await;
        connection.close(0u32.into(), b"");
    }

    // HTTP3: stream handle
    #[cfg(all(
        feature = "listener-https-support",
        feature = "listener-quic-support",
        feature = "listener-tls-support"
    ))]
    async fn http3_request_handle(
        service: Arc<HTTPService>,
        peer_addr: SocketAddr,
        req: http::Request<()>,
        mut req_stream: h3::server::RequestStream<h3_quinn::BidiStream<bytes::Bytes>, bytes::Bytes>,
        conn_canceller_guard: common::CancellerGuard,
    ) {
        tokio::select! {
            _ = service.http3_call(peer_addr, req, &mut req_stream) => {}
            _ = conn_canceller_guard.cancelled() => {}
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for HTTPListener {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let workflow = self
            .manager
            .get_workflow(&self.workflow_tag)
            .await
            .ok_or(format!("workflow [{}] not found", self.workflow_tag))?;
        let (canceller, canceller_guard) = common::new_canceller();
        let manager = self.manager.clone();
        let listener_tag = self.tag.clone();
        let service = Arc::new(HTTPService::new(
            workflow,
            self.logger.clone(),
            self.tag.clone(),
            self.query_timeout,
        ));
        match &self.listener_type {
            HTTPListenerType::HTTP => {
                let tcp_listener = TcpListener::bind(&self.listen).await?;
                info!(self.logger, "HTTP listener listen on {}", self.listen);
                tokio::spawn(Self::http_handle(
                    manager,
                    service,
                    listener_tag,
                    tcp_listener,
                    canceller_guard,
                ));
            }

            #[cfg(all(feature = "listener-https-support", feature = "listener-tls-support"))]
            HTTPListenerType::HTTPS(tls_config) => {
                let tcp_listener = TcpListener::bind(&self.listen).await?;
                let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_config.clone());
                info!(self.logger, "HTTPS listener listen on {}", self.listen);
                tokio::spawn(Self::https_handle(
                    manager,
                    service,
                    listener_tag,
                    tcp_listener,
                    Arc::new(tls_acceptor),
                    canceller_guard,
                ));
            }

            #[cfg(all(
                feature = "listener-https-support",
                feature = "listener-quic-support",
                feature = "listener-tls-support"
            ))]
            HTTPListenerType::HTTP3(tls_config) => {
                let udp_socket = std::net::UdpSocket::bind(&self.listen)?;
                let quic_server_config = quinn_proto::ServerConfig::with_crypto(tls_config.clone());
                let quic_endpoint = quinn::Endpoint::new(
                    quinn::EndpointConfig::default(),
                    Some(quic_server_config),
                    udp_socket,
                    Arc::new(quinn::TokioRuntime),
                )?;
                info!(self.logger, "HTTP3 endpoint listen on {}", self.listen);
                tokio::spawn(Self::http3_handle(
                    manager,
                    service,
                    listener_tag,
                    quic_endpoint,
                    canceller_guard,
                ));
            }
        }
        self.canceller.lock().await.replace(canceller);
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(mut canceller) = self.canceller.lock().await.take() {
            canceller.cancel_and_wait().await;
        }
        Ok(())
    }
}

impl adapter::Listener for HTTPListener {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &str {
        HTTP_LISTENER_TYPE
    }
}

struct HTTPService {
    workflow: Arc<Box<dyn adapter::Workflow>>,
    logger: Arc<Box<dyn log::Logger>>,
    listener_tag: String,
    query_timeout: Option<Duration>,
}

impl HTTPService {
    fn new(
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        listener_tag: String,
        query_timeout: Option<Duration>,
    ) -> Self {
        Self {
            workflow,
            logger,
            listener_tag,
            query_timeout,
        }
    }

    fn logger(&self) -> &Box<dyn log::Logger> {
        &self.logger
    }

    fn failed_response_with_body(
        status_code: http::StatusCode,
    ) -> http::Response<http_body_util::Full<bytes::Bytes>> {
        let mut response = http::Response::new(http_body_util::Full::new(bytes::Bytes::new()));
        *response.status_mut() = status_code;
        response
    }

    async fn failed_response_with_req_stream(
        status_code: http::StatusCode,
        req_stream: &mut h3::server::RequestStream<
            h3_quinn::BidiStream<bytes::Bytes>,
            bytes::Bytes,
        >,
    ) {
        let mut response = http::Response::new(());
        *response.status_mut() = status_code;
        if let Ok(_) = req_stream.send_response(response).await {
            req_stream.finish().await.ok();
        }
    }

    async fn http_call(
        &self,
        peer_addr: SocketAddr,
        req: http::Request<hyper::body::Incoming>,
    ) -> http::Response<http_body_util::Full<bytes::Bytes>> {
        // GET Real IP
        let mut peer_addr = peer_addr.ip();
        if let Some(v) = req.headers().get("X-Forwarded-For") {
            v.to_str()
                .map(|v| v.split(',').next().unwrap_or(v))
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|v: IpAddr| peer_addr = v);
        }
        if let Some(v) = req.headers().get("X-Real-IP") {
            v.to_str()
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|v: IpAddr| peer_addr = v);
        }
        // Check Header
        let mut check_header = false;
        if let Some(v) = req.headers().get(http::header::CONTENT_TYPE) {
            if v == "application/dns-message" {
                check_header = true;
            }
        }
        if !check_header {
            error!(
                self.logger,
                "invalid content type, peer_addr: {}", peer_addr
            );
            return Self::failed_response_with_body(http::StatusCode::BAD_REQUEST);
        }
        // GET Request
        let mut request = None;
        match *req.method() {
            http::Method::POST => {
                // POST
                if let Ok(data) = http_body_util::BodyExt::collect(req.into_body()).await {
                    if let Ok(r) = Message::from_vec(&data.to_bytes()) {
                        request = Some(r);
                    }
                }
                if request.is_none() {
                    error!(
                        self.logger,
                        "invalid request: missing request or deserialize message failed, peer_addr: {}",
                        peer_addr
                    );
                    return Self::failed_response_with_body(http::StatusCode::BAD_REQUEST);
                }
            }
            http::Method::GET => {
                if let Some(pq) = req.uri().path_and_query() {
                    if let Some(q) = pq.query() {
                        if let Some((_, v)) = q.split_once("dns=") {
                            if let Ok(buf) = base64::Engine::decode(
                                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                                v.split_once('&').map(|(v, _)| v).unwrap_or(v),
                            ) {
                                if let Ok(r) = Message::from_vec(&buf) {
                                    request = Some(r);
                                }
                            }
                        }
                    }
                }
                if request.is_none() {
                    error!(
                        self.logger,
                        "invalid request: missing request or deserialize message failed, peer_addr: {}",
                        peer_addr
                    );
                    return Self::failed_response_with_body(http::StatusCode::BAD_REQUEST);
                }
            }
            _ => {
                error!(
                    self.logger,
                    "invalid method: {}, peer_addr: {}",
                    req.method(),
                    peer_addr
                );
                return Self::failed_response_with_body(http::StatusCode::METHOD_NOT_ALLOWED);
            }
        }
        let request = request.unwrap();
        // Handle
        let res = super::handle(
            self.workflow.clone(),
            self.logger.clone(),
            self.listener_tag.clone(),
            self.query_timeout,
            peer_addr,
            request,
        )
        .await;
        match res {
            Some(msg) => {
                let buf = match msg.to_vec() {
                    Ok(v) => v,
                    Err(_) => {
                        return Self::failed_response_with_body(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                        );
                    }
                };
                let mut response =
                    http::Response::new(http_body_util::Full::new(bytes::Bytes::from(buf)));
                *response.status_mut() = http::StatusCode::OK;
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    "application/dns-message".parse().unwrap(),
                );
                response
            }
            None => Self::failed_response_with_body(http::StatusCode::NO_CONTENT),
        }
    }

    #[cfg(all(
        feature = "listener-https-support",
        feature = "listener-quic-support",
        feature = "listener-tls-support"
    ))]
    async fn http3_call(
        &self,
        peer_addr: SocketAddr,
        req: http::Request<()>,
        req_stream: &mut h3::server::RequestStream<
            h3_quinn::BidiStream<bytes::Bytes>,
            bytes::Bytes,
        >,
    ) {
        // GET Real IP
        let mut peer_addr = peer_addr.ip();
        if let Some(v) = req.headers().get("X-Forwarded-For") {
            v.to_str()
                .map(|v| v.split(',').next().unwrap_or(v))
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|v: IpAddr| peer_addr = v);
        }
        if let Some(v) = req.headers().get("X-Real-IP") {
            v.to_str()
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|v: IpAddr| peer_addr = v);
        }
        // Check Header
        let mut check_header = false;
        if let Some(v) = req.headers().get(http::header::CONTENT_TYPE) {
            if v == "application/dns-message" {
                check_header = true;
            }
        }
        if !check_header {
            error!(
                self.logger,
                "invalid content type, peer_addr: {}", peer_addr
            );
            Self::failed_response_with_req_stream(http::StatusCode::BAD_REQUEST, req_stream).await;
            return;
        }
        // GET Request
        let mut request = None;
        match *req.method() {
            http::Method::POST => {
                // POST
                let mut content_length = req
                    .headers()
                    .get(http::header::CONTENT_LENGTH)
                    .map(|v| {
                        v.to_str()
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0usize)
                    })
                    .unwrap_or(0usize);
                if content_length == 0 {
                    content_length = u32::MAX as usize;
                }
                let mut buf = bytes::BytesMut::with_capacity(content_length);
                while let Ok(Some(data)) = req_stream.recv_data().await {
                    bytes::BufMut::put(&mut buf, data);
                }
                if let Ok(r) = Message::from_vec(&buf) {
                    request = Some(r);
                }
                if request.is_none() {
                    error!(
                        self.logger,
                        "invalid request: missing request or deserialize message failed, peer_addr: {}",
                        peer_addr
                    );
                    Self::failed_response_with_req_stream(
                        http::StatusCode::BAD_REQUEST,
                        req_stream,
                    )
                    .await;
                    return;
                }
            }
            http::Method::GET => {
                if let Some(pq) = req.uri().path_and_query() {
                    if let Some(q) = pq.query() {
                        if let Some((_, v)) = q.split_once("dns=") {
                            if let Ok(buf) = base64::Engine::decode(
                                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                                v.split_once('&').map(|(v, _)| v).unwrap_or(v),
                            ) {
                                if let Ok(r) = Message::from_vec(&buf) {
                                    request = Some(r);
                                }
                            }
                        }
                    }
                }
                if request.is_none() {
                    error!(
                        self.logger,
                        "invalid request: missing request or deserialize message failed, peer_addr: {}",
                        peer_addr
                    );
                    Self::failed_response_with_req_stream(
                        http::StatusCode::BAD_REQUEST,
                        req_stream,
                    )
                    .await;
                    return;
                }
            }
            _ => {
                error!(
                    self.logger,
                    "invalid method: {}, peer_addr: {}",
                    req.method(),
                    peer_addr
                );
                Self::failed_response_with_req_stream(
                    http::StatusCode::METHOD_NOT_ALLOWED,
                    req_stream,
                )
                .await;
                return;
            }
        }
        let request = request.unwrap();
        // Handle
        let res = super::handle(
            self.workflow.clone(),
            self.logger.clone(),
            self.listener_tag.clone(),
            self.query_timeout,
            peer_addr,
            request,
        )
        .await;
        match res {
            Some(msg) => {
                let buf = match msg.to_vec() {
                    Ok(v) => v,
                    Err(_) => {
                        Self::failed_response_with_req_stream(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            req_stream,
                        )
                        .await;
                        return;
                    }
                };
                let mut response = http::Response::new(());
                *response.status_mut() = http::StatusCode::OK;
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    "application/dns-message".parse().unwrap(),
                );
                if let Ok(_) = req_stream.send_response(response).await {
                    req_stream.send_data(bytes::Bytes::from(buf)).await.ok();
                }
                req_stream.finish().await.ok();
            }
            None => {
                Self::failed_response_with_req_stream(http::StatusCode::NO_CONTENT, req_stream)
                    .await
            }
        }
    }
}

struct HTTPRequestState {
    service: Arc<HTTPService>,
    peer_addr: SocketAddr,
}

impl HTTPRequestState {
    fn new(service: Arc<HTTPService>, peer_addr: SocketAddr) -> Self {
        Self { service, peer_addr }
    }
}

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for HTTPRequestState {
    type Response = http::Response<http_body_util::Full<bytes::Bytes>>;
    type Error = String;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        let peer_addr = self.peer_addr.clone();
        let service = self.service.clone();
        Box::pin(async move { Ok(service.http_call(peer_addr, req).await) })
    }
}

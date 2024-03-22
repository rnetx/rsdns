use std::{
    error::Error,
    io::{self, IoSlice},
    net::SocketAddr,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{adapter, debug, error, info, log, option};

use super::network;

const QUIC_UPSTREAM_TYPE: &str = "quic";

pub(crate) struct QUICUpstream {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    address: network::SocksAddr,
    idle_timeout: Duration,
    quic_client_config: quinn::ClientConfig,
    server_name: rustls::ServerName,
    bootstrap: Option<super::Bootstrap>,
    dialer: network::Dialer,
    //
    last_use: Arc<AtomicI64>,
    connection: Arc<RwLock<Option<QUICConnection>>>,
    handler: Mutex<Option<JoinHandle<()>>>,
}

impl QUICUpstream {
    pub(super) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Arc<Box<dyn log::Logger>>,
        tag: String,
        options: option::QUICUpstreamOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let address = network::SocksAddr::parse_with_default_port(&options.address, 853)?;
        let dialer = network::Dialer::new(manager.clone(), options.dialer.unwrap_or_default())?;
        let (mut tls_client_config, server_name) = super::new_tls_config(options.tls)?;
        let server_name = match server_name {
            Some(s) => s,
            None => match &address {
                network::SocksAddr::DomainAddr(domain, _) => domain.clone(),
                network::SocksAddr::SocketAddr(addr) => addr.ip().to_string(),
            },
        };
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
        tls_client_config.alpn_protocols = vec![b"doq".to_vec()];
        let quic_client_config = quinn::ClientConfig::new(Arc::new(tls_client_config));
        Ok(Self {
            manager,
            logger,
            tag,
            idle_timeout: options
                .idle_timeout
                .unwrap_or_else(|| super::DEFAULT_IDLE_TIMEOUT),
            address,
            quic_client_config,
            server_name,
            bootstrap,
            dialer,
            last_use: Arc::new(AtomicI64::new(0)),
            connection: Arc::new(RwLock::new(None)),
            handler: Mutex::new(None),
        })
    }

    async fn handle(
        connection: Arc<RwLock<Option<QUICConnection>>>,
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

    async fn get_quic_connection(&self) -> Result<QUICConnection, Box<dyn Error + Send + Sync>> {
        let mut connection = self.connection.read().await.clone();
        if let Some(connection) = connection.take() {
            if !connection.is_closed() {
                self.flush_last_use();
                return Ok(connection);
            }
        }
        let address = self.address.clone();
        let (quic_endpoint, quic_connection) =
            if address.is_domain_addr() && self.bootstrap.is_some() {
                let (domain, port) = address.must_domain_addr();
                let bootstrap = self.bootstrap.as_ref().unwrap();
                debug!(self.logger, "lookup {}", domain);
                let ips = bootstrap.lookup(domain).await.map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!("lookup domain {} failed: {}", domain, err),
                    )
                })?;
                // TODO: Use First
                let addr = network::SocksAddr::from(SocketAddr::new(ips[0], port));
                self.dialer
                    .new_quic_connection(
                        addr,
                        self.quic_client_config.clone(),
                        &self.get_server_name_str(),
                    )
                    .await?
            } else {
                self.dialer
                    .new_quic_connection(
                        address,
                        self.quic_client_config.clone(),
                        &self.get_server_name_str(),
                    )
                    .await?
            };
        debug!(self.logger, "new quic connection");
        let quic_connection =
            QUICConnection::new(self.logger.clone(), quic_endpoint, quic_connection);
        self.connection
            .write()
            .await
            .replace(quic_connection.clone());
        self.flush_last_use();
        Ok(quic_connection)
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
        let quic_connection = self.get_quic_connection().await?;
        let (mut sender, mut receiver) = quic_connection.open_bi().await.map_err(|e| {
            quic_connection.set_close_tag();
            e
        })?;
        sender
            .write_vectored(&[
                IoSlice::new(&(request_bytes.len() as u16).to_be_bytes()),
                IoSlice::new(&request_bytes),
            ])
            .await
            .map_err(|e| {
                quic_connection.set_close_tag();
                e
            })?;
        sender.finish().await.map_err(|e| {
            quic_connection.set_close_tag();
            e
        })?;
        let length = receiver.read_u16().await.map_err(|e| {
            quic_connection.set_close_tag();
            e
        })?;
        let mut buf = vec![0u8; length as usize];
        receiver.read_exact(&mut buf).await.map_err(|e| {
            quic_connection.set_close_tag();
            e
        })?;
        let result = Message::from_vec(&buf).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
            format!("deserialize response failed: {}", err).into()
        });
        result
    }
}

#[async_trait::async_trait]
impl adapter::Common for QUICUpstream {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
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

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(handler) = self.handler.lock().await.take() {
            handler.abort();
        }
        if let Some(connection) = self.connection.write().await.take() {
            connection.set_close_tag();
        }
        self.dialer.close().await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::Upstream for QUICUpstream {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        QUIC_UPSTREAM_TYPE
    }

    fn dependencies(&self) -> Option<Vec<String>> {
        if let Some(bootstrap) = self.bootstrap.as_ref() {
            return Some(vec![bootstrap.upstream_tag().to_string()]);
        }
        None
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

// Connection

struct QUICConnection {
    token: CancellationToken,
    n: Arc<AtomicUsize>,
    close_tag: Arc<AtomicBool>,
    connection: quinn::Connection,
}

impl Clone for QUICConnection {
    fn clone(&self) -> Self {
        self.n.fetch_add(1, Ordering::Relaxed);
        Self {
            token: self.token.clone(),
            n: self.n.clone(),
            close_tag: self.close_tag.clone(),
            connection: self.connection.clone(),
        }
    }
}

impl QUICConnection {
    fn new(
        logger: Arc<Box<dyn log::Logger>>,
        endpoint: quinn::Endpoint,
        connection: quinn::Connection,
    ) -> Self {
        let token = CancellationToken::new();
        let token_driver = token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = endpoint.wait_idle() => {
                    token_driver.cancel();
                    debug!(logger, "close quic connection");
                    return;
                }
                _ = token_driver.cancelled() => {
                }
            }
            endpoint.close(0u8.into(), b"");
            endpoint.wait_idle().await;
            debug!(logger, "close quic connection");
        });
        Self {
            token,
            n: Arc::new(AtomicUsize::new(1)),
            close_tag: Arc::new(AtomicBool::new(false)),
            connection,
        }
    }

    fn set_close_tag(&self) {
        self.close_tag.store(true, Ordering::Relaxed);
    }

    fn is_closed(&self) -> bool {
        self.token.is_cancelled() || self.close_tag.load(Ordering::Relaxed)
    }
}

impl Drop for QUICConnection {
    fn drop(&mut self) {
        if self.n.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.close_tag.store(true, Ordering::Relaxed);
        }
        if self.close_tag.load(Ordering::Relaxed) {
            self.token.cancel();
            self.connection.close(0u8.into(), b"");
        }
    }
}

impl Deref for QUICConnection {
    type Target = quinn::Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for QUICConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

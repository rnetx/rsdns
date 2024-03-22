use std::{error::Error, io, sync::Arc, time::Duration};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};

use crate::{adapter, debug, error, info, log, option};

use super::network;

const TLS_UPSTREAM_TYPE: &str = "tls";

pub(crate) struct TLSUpstream {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    address: network::SocksAddr,
    enable_pipeline: bool,
    idle_timeout: Duration,
    tls_connector: tokio_rustls::TlsConnector,
    server_name: rustls::ServerName,
    bootstrap: Option<super::Bootstrap>,
    pool: RwLock<
        Option<
            super::Pool<
                super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
            >,
        >,
    >,
    pipeline_stream_pool: RwLock<
        Option<
            super::Pool<
                super::PipelineStream<
                    super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
                >,
            >,
        >,
    >,
    dialer: Arc<network::Dialer>,
}

impl TLSUpstream {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Arc<Box<dyn log::Logger>>,
        tag: String,
        options: option::TLSUpstreamOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let address = network::SocksAddr::parse_with_default_port(&options.address, 853)?;
        let dialer = Arc::new(network::Dialer::new(
            manager.clone(),
            options.dialer.unwrap_or_default(),
        )?);
        let (tls_client_config, server_name) = super::new_tls_config(options.tls)?;
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
        Ok(Self {
            manager,
            logger,
            tag,
            address,
            enable_pipeline: options.enable_pipeline,
            idle_timeout: options
                .idle_timeout
                .unwrap_or_else(|| super::DEFAULT_IDLE_TIMEOUT),
            tls_connector: tokio_rustls::TlsConnector::from(Arc::new(tls_client_config)),
            server_name,
            bootstrap,
            pool: RwLock::new(None),
            pipeline_stream_pool: RwLock::new(None),
            dialer,
        })
    }

    async fn new_tcp_stream(&self) -> io::Result<network::GenericTcpStream> {
        let address = self.address.clone();
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
            self.dialer.parallel_new_tcp_stream(ips, port).await
        } else {
            self.dialer.new_tcp_stream(address).await
        }
    }

    async fn new_tls_stream(
        &self,
    ) -> io::Result<tokio_rustls::client::TlsStream<network::GenericTcpStream>> {
        let tcp_stream = self.new_tcp_stream().await?;
        self.tls_connector
            .connect(self.server_name.clone(), tcp_stream)
            .await
    }

    async fn exchange_wrapper(
        &self,
        request: &Message,
    ) -> Result<Message, Box<dyn Error + Send + Sync>> {
        if !self.enable_pipeline {
            let request_bytes = request
                .to_vec()
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("serialize request failed: {}", err).into()
                })?;
            let pool_lock = self.pool.read().await;
            let pool = pool_lock.as_ref().unwrap();
            let mut tls_stream = match pool.get().await {
                Some(v) => v,
                None => {
                    let new_tls_stream = self
                        .new_tls_stream()
                        .await
                        .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                            format!("get tls stream failed: {}", err).into()
                        })?;
                    debug!(self.logger, "new tls stream");
                    super::LogStream::new(self.logger.clone(), "close tls stream", new_tls_stream)
                }
            };
            let request_length_bytes = (request_bytes.len() as u16).to_be_bytes();
            let mut request_chain =
                bytes::Buf::chain(&request_length_bytes[..], request_bytes.as_slice());
            tls_stream.write_all_buf(&mut request_chain).await?;
            tls_stream.flush().await?;
            let length = tls_stream.read_u16().await?;
            let mut buf = vec![0u8; length as usize];
            tls_stream.read_exact(&mut buf).await?;
            let result =
                Message::from_vec(&buf).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("deserialize response failed: {}", err).into()
                });
            pool.put(tls_stream).await;
            result
        } else {
            let (pipeline_stream, is_new) = match self
                .pipeline_stream_pool
                .read()
                .await
                .as_ref()
                .unwrap()
                .get_pipeline_stream()
                .await
            {
                Some(stream) => (stream, false),
                None => {
                    let new_tls_stream = self
                        .new_tls_stream()
                        .await
                        .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                            format!("handeshake tls stream failed: {}", err).into()
                        })?;
                    debug!(self.logger, "new tls pipeline stream");
                    let stream = super::LogStream::new(
                        self.logger.clone(),
                        "close tls pipeline stream",
                        new_tls_stream,
                    );
                    (
                        super::PipelineStream::new(self.logger.clone(), stream).await,
                        true,
                    )
                }
            };
            let res = pipeline_stream.exchange(request).await;
            if res.is_ok() {
                if !pipeline_stream.is_cancelled() && is_new {
                    self.pipeline_stream_pool
                        .read()
                        .await
                        .as_ref()
                        .unwrap()
                        .put(pipeline_stream)
                        .await;
                }
            }
            res
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for TLSUpstream {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.dialer.start().await;
        if let Some(bootstrap) = &self.bootstrap {
            bootstrap.start().await?;
        }
        if !self.enable_pipeline {
            let pool: super::Pool<
                super::LogStream<tokio_rustls::client::TlsStream<network::GenericTcpStream>>,
            > = super::Pool::new_pool(0, self.idle_timeout).await;
            self.pool.write().await.replace(pool);
        } else {
            let pipeline_stream_pool =
                super::Pool::new_pipeline_stream_pool(0, self.idle_timeout).await;
            self.pipeline_stream_pool
                .write()
                .await
                .replace(pipeline_stream_pool);
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if !self.enable_pipeline {
            if let Some(mut pool) = self.pool.write().await.take() {
                pool.close().await;
            }
        } else {
            if let Some(mut pool) = self.pipeline_stream_pool.write().await.take() {
                pool.close().await;
            }
        }
        self.dialer.close().await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::Upstream for TLSUpstream {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TLS_UPSTREAM_TYPE
    }

    fn dependencies(&self) -> Option<Vec<String>> {
        self.bootstrap
            .as_ref()
            .map(|b| vec![b.upstream_tag().to_string()])
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
        let res = self.exchange_wrapper(request).await;
        match &res {
            Ok(_) => {
                info!(
                    self.logger,
                    { option_tracker = log_tracker },
                    "exchange {} success",
                    &query_info
                );
            }
            Err(e) => {
                error!(
                    self.logger,
                    { option_tracker = log_tracker },
                    "exchange {} failed: {}",
                    &query_info,
                    e.to_string()
                );
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

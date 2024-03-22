use std::{
    error::Error,
    io::{self, IoSlice},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};

use crate::{adapter, debug, error, info, log, option, warn};

use super::network;

const UDP_UPSTREAM_TYPE: &str = "udp";

pub(crate) struct UDPUpstream {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    address: network::SocksAddr,
    fallback_tcp: bool,
    enable_pipeline: bool,
    idle_timeout: Duration,
    bootstrap: Option<super::Bootstrap>,
    tcp_pipeline_stream_pool: RwLock<
        Option<super::Pool<super::PipelineStream<super::LogStream<network::GenericTcpStream>>>>,
    >,
    tcp_pool: RwLock<Option<super::Pool<super::LogStream<network::GenericTcpStream>>>>,
    udp_pool: RwLock<
        Option<super::Pool<super::LogStream<(network::GenericUdpSocket, network::SocksAddr)>>>,
    >,
    dialer: Arc<network::Dialer>,
}

impl UDPUpstream {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Arc<Box<dyn log::Logger>>,
        tag: String,
        options: option::UDPUpstreamOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let address = network::SocksAddr::parse_with_default_port(&options.address, 53)?;
        let dialer = Arc::new(network::Dialer::new(
            manager.clone(),
            options.dialer.unwrap_or_default(),
        )?);
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
            fallback_tcp: options.fallback_tcp,
            enable_pipeline: options.enable_pipeline,
            idle_timeout: options
                .idle_timeout
                .unwrap_or_else(|| super::DEFAULT_IDLE_TIMEOUT),
            bootstrap,
            tcp_pipeline_stream_pool: RwLock::new(None),
            tcp_pool: RwLock::new(None),
            udp_pool: RwLock::new(None),
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

    async fn new_udp_socket(&self) -> io::Result<(network::GenericUdpSocket, network::SocksAddr)> {
        if self.address.is_domain_addr() && self.bootstrap.is_some() {
            let (domain, port) = self.address.must_domain_addr();
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
            self.dialer.new_udp_socket(&addr).await
        } else {
            self.dialer.new_udp_socket(&self.address).await
        }
    }

    async fn exchange_udp(
        &self,
        request: &Message,
    ) -> Result<Option<Message>, Box<dyn Error + Send + Sync>> {
        let request_bytes = request
            .to_vec()
            .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("serialize request failed: {}", err).into()
            })?;
        let udp_pool_lock = self.udp_pool.read().await;
        let udp_pool = udp_pool_lock.as_ref().unwrap();
        let s = match udp_pool.get().await {
            Some(v) => v,
            None => {
                let (new_udp_socket, remote_addr) = self.new_udp_socket().await.map_err::<Box<
                    dyn Error + Send + Sync,
                >, _>(
                    |err| format!("get udp socket failed: {}", err).into(),
                )?;
                debug!(self.logger, "new udp socket");
                super::LogStream::new(
                    self.logger.clone(),
                    "close udp socket",
                    (new_udp_socket, remote_addr),
                )
            }
        };
        let (udp_socket, remote_addr) = s.as_ref();
        udp_socket
            .send_to(&request_bytes, remote_addr.clone())
            .await?;
        let mut buf = vec![0u8; 4096];
        udp_socket.recv_buf_from(&mut buf).await?;
        let result = Message::from_vec(&buf).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
            format!("deserialize response failed: {}", err).into()
        });
        result.map(|m| if m.truncated() { None } else { Some(m) })
    }

    async fn exchange_tcp(
        &self,
        request: &Message,
    ) -> Result<Message, Box<dyn Error + Send + Sync>> {
        if !self.enable_pipeline {
            let request_bytes = request
                .to_vec()
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("serialize request failed: {}", err).into()
                })?;
            let tcp_pool_lock = self.tcp_pool.read().await;
            let tcp_pool = tcp_pool_lock.as_ref().unwrap();
            let mut tcp_stream = match tcp_pool.get().await {
                Some(v) => v,
                None => {
                    let new_tcp_stream = self
                        .new_tcp_stream()
                        .await
                        .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                            format!("get tcp stream failed: {}", err).into()
                        })?;
                    debug!(self.logger, "new tcp stream");
                    super::LogStream::new(self.logger.clone(), "close tcp stream", new_tcp_stream)
                }
            };
            tcp_stream
                .write_vectored(&[
                    IoSlice::new(&(request_bytes.len() as u16).to_be_bytes()),
                    IoSlice::new(&request_bytes),
                ])
                .await?;
            tcp_stream.flush().await?;
            let length = tcp_stream.read_u16().await?;
            let mut buf = Vec::with_capacity(length as usize);
            tcp_stream.read_buf(&mut buf).await?;
            let result =
                Message::from_vec(&buf).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("deserialize response failed: {}", err).into()
                });
            tcp_pool.put(tcp_stream).await;
            result
        } else {
            let (tcp_pipeline_stream, is_new) = match self
                .tcp_pipeline_stream_pool
                .read()
                .await
                .as_ref()
                .unwrap()
                .get_pipeline_stream()
                .await
            {
                Some(stream) => (stream, false),
                None => {
                    let new_tcp_stream = self
                        .new_tcp_stream()
                        .await
                        .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                            format!("get tcp stream failed: {}", err).into()
                        })?;
                    debug!(self.logger, "new tcp pipeline stream");
                    let stream = super::LogStream::new(
                        self.logger.clone(),
                        "close tcp pipeline stream",
                        new_tcp_stream,
                    );
                    (
                        super::PipelineStream::new(self.logger.clone(), stream).await,
                        true,
                    )
                }
            };
            let res = tcp_pipeline_stream.exchange(request).await;
            if res.is_ok() {
                if !tcp_pipeline_stream.is_cancelled() && is_new {
                    self.tcp_pipeline_stream_pool
                        .read()
                        .await
                        .as_ref()
                        .unwrap()
                        .put(tcp_pipeline_stream)
                        .await;
                }
            }
            res
        }
    }

    async fn exchange_wrapper(
        &self,
        log_tracker: Option<&log::Tracker>,
        request: &Message,
    ) -> Result<Message, Box<dyn Error + Send + Sync>> {
        let result = self.exchange_udp(request).await?;
        if let Some(message) = result {
            Ok(message)
        } else {
            if !self.fallback_tcp {
                warn!(
                    self.logger,
                    { option_tracker = log_tracker },
                    "udp response is truncated, you can enable fallback-tcp"
                );
                return Err("udp response is truncated, you can enable fallback-tcp".into());
            }
            self.exchange_tcp(request).await
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for UDPUpstream {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.dialer.start().await;
        if let Some(bootstrap) = &self.bootstrap {
            bootstrap.start().await?;
        }
        if self.fallback_tcp {
            if !self.enable_pipeline {
                let tcp_pool: super::Pool<super::LogStream<network::GenericTcpStream>> =
                    super::Pool::new_pool(0, self.idle_timeout).await;
                self.tcp_pool.write().await.replace(tcp_pool);
            } else {
                let tcp_pipeline_stream_pool =
                    super::Pool::new_pipeline_stream_pool(0, self.idle_timeout).await;
                self.tcp_pipeline_stream_pool
                    .write()
                    .await
                    .replace(tcp_pipeline_stream_pool);
            }
        }
        let udp_pool = super::Pool::new_pool(0, self.idle_timeout).await;
        self.udp_pool.write().await.replace(udp_pool);
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.fallback_tcp {
            if !self.enable_pipeline {
                if let Some(mut pool) = self.tcp_pool.write().await.take() {
                    pool.close().await;
                }
            } else {
                if let Some(mut pool) = self.tcp_pipeline_stream_pool.write().await.take() {
                    pool.close().await;
                }
            }
        }
        if let Some(mut pool) = self.udp_pool.write().await.take() {
            pool.close().await;
        }
        self.dialer.close().await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::Upstream for UDPUpstream {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        UDP_UPSTREAM_TYPE
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
        let res = self.exchange_wrapper(log_tracker, request).await;
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

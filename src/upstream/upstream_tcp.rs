use std::{
    io::{self, IoSlice},
    sync::Arc,
    time::Duration,
};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};

use crate::{adapter, debug, error, info, log, option};

use super::network;

const TCP_UPSTREAM_TYPE: &str = "tcp";

pub(crate) struct TCPUpstream {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    address: network::SocksAddr,
    enable_pipeline: bool,
    idle_timeout: Duration,
    bootstrap: Option<super::Bootstrap>,
    pool: RwLock<Option<super::Pool<super::LogStream<network::GenericTcpStream>>>>,
    pipeline_stream_pool: RwLock<
        Option<super::Pool<super::PipelineStream<super::LogStream<network::GenericTcpStream>>>>,
    >,
    dialer: Arc<network::Dialer>,
}

impl TCPUpstream {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Arc<Box<dyn log::Logger>>,
        tag: String,
        options: option::TCPUpstreamOptions,
    ) -> anyhow::Result<Self> {
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
            return Err(anyhow::anyhow!("domain address not supported, because dialer is unsupported, and bootstrap is not set"));
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
            bootstrap,
            pool: RwLock::new(None),
            pipeline_stream_pool: RwLock::new(None),
            dialer,
        })
    }

    async fn new_tcp_stream(&self) -> io::Result<network::GenericTcpStream> {
        super::Bootstrap::dial_with_bootstrap(
            &self.logger,
            self.address.clone(),
            &self.bootstrap,
            &self.dialer,
            |address, dialer| {
                let dialer = (*dialer).clone();
                async move { dialer.new_tcp_stream(address).await }
            },
            |ips, port, dialer| {
                let dialer = (*dialer).clone();
                async move { dialer.parallel_new_tcp_stream(ips, port).await }
            },
        )
        .await
    }

    async fn exchange_wrapper(&self, request: &Message) -> anyhow::Result<Message> {
        if !self.enable_pipeline {
            let request_bytes = request
                .to_vec()
                .map_err(|err| anyhow::anyhow!("serialize request failed: {}", err))?;
            let pool_lock = self.pool.read().await;
            let pool = pool_lock.as_ref().unwrap();
            let mut tcp_stream = match pool.get().await {
                Some(v) => v,
                None => {
                    let new_tcp_stream = self
                        .new_tcp_stream()
                        .await
                        .map_err(|err| anyhow::anyhow!("get tcp stream failed: {}", err))?;
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
            let result = Message::from_vec(&buf)
                .map_err(|err| anyhow::anyhow!("deserialize response failed: {}", err));
            pool.put(tcp_stream).await;
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
                    let new_tcp_stream = self
                        .new_tcp_stream()
                        .await
                        .map_err(|err| anyhow::anyhow!("get tcp stream failed: {}", err))?;
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
impl adapter::Common for TCPUpstream {
    async fn start(&self) -> anyhow::Result<()> {
        self.dialer.start().await;
        if let Some(bootstrap) = &self.bootstrap {
            bootstrap.start().await?;
        }
        if !self.enable_pipeline {
            let pool: super::Pool<super::LogStream<network::GenericTcpStream>> =
                super::Pool::new_pool(0, self.idle_timeout).await;
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

    async fn close(&self) -> anyhow::Result<()> {
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
impl adapter::Upstream for TCPUpstream {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TCP_UPSTREAM_TYPE
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
    ) -> anyhow::Result<Message> {
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

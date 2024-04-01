use std::{sync::Arc, time::Duration};

use hickory_proto::op::Message;

use crate::{adapter, error, log, option};

pub(super) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn show_query(message: &Message) -> String {
    if let Some(query) = message.query() {
        format!(
            "{} {} {}",
            query.name(),
            query.query_type(),
            query.query_class()
        )
    } else {
        "unknown".to_string()
    }
}

pub(crate) fn new_upstream(
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Box<dyn log::Logger>,
    tag: String,
    options: option::UpstreamOptions,
) -> anyhow::Result<Box<dyn adapter::Upstream>> {
    let logger = Arc::new(logger);
    match options.inner {
        option::UpstreamInnerOptions::UDPUpstream(udp_options) => {
            super::UDPUpstream::new(manager, logger, tag.clone(), udp_options)
                .map(|u| {
                    Box::new(GenericUpstream::new(u, options.query_timeout))
                        as Box<dyn adapter::Upstream>
                })
                .map_err(|err| anyhow::anyhow!("create udp upstream [{}] failed: {}", tag, err))
        }
        option::UpstreamInnerOptions::TCPUpstream(tcp_options) => {
            super::TCPUpstream::new(manager, logger, tag.clone(), tcp_options)
                .map(|u| {
                    Box::new(GenericUpstream::new(u, options.query_timeout))
                        as Box<dyn adapter::Upstream>
                })
                .map_err(|err| anyhow::anyhow!("create tcp upstream [{}] failed: {}", tag, err))
        }

        option::UpstreamInnerOptions::DHCPUpstream(dhcp_options) => {
            cfg_if::cfg_if! {
                if #[cfg(feature = "upstream-dhcp")] {
                    super::DHCPUpstream::new(manager, logger, tag.clone(), dhcp_options)
                        .map(|u| {
                            Box::new(GenericUpstream::new(u, options.query_timeout))
                                as Box<dyn adapter::Upstream>
                        })
                        .map_err(|err| anyhow::anyhow!("create dhcp upstream [{}] failed: {}", tag, err))
                } else {
                    Err("dhcp upstream is not supported")
                }
            }
        }

        option::UpstreamInnerOptions::TLSUpstream(tls_options) => {
            cfg_if::cfg_if! {
                if #[cfg(feature = "upstream-tls")] {
                    super::TLSUpstream::new(manager, logger, tag.clone(), tls_options)
                        .map(|u| {
                            Box::new(GenericUpstream::new(u, options.query_timeout))
                                as Box<dyn adapter::Upstream>
                        })
                        .map_err(|err| anyhow::anyhow!("create tls upstream [{}] failed: {}", tag, err))
                } else {
                    Err(anyhow::anyhow("tls upstream is not supported"))
                }
            }
        }

        option::UpstreamInnerOptions::HTTPSUpstream(https_options) => {
            cfg_if::cfg_if! {
                if #[cfg(all(feature = "upstream-https", feature = "upstream-tls"))] {
                    super::HTTPSUpstream::new(manager, logger, tag.clone(), https_options)
                        .map(|u| {
                            Box::new(GenericUpstream::new(u, options.query_timeout))
                                as Box<dyn adapter::Upstream>
                        })
                        .map_err(|err| anyhow::anyhow!("create https upstream [{}] failed: {}", tag, err))
                } else {
                    Err(anyhow::anyhow("https upstream is not supported"))
                }
            }
        }

        option::UpstreamInnerOptions::QUICUpstream(quic_options) => {
            cfg_if::cfg_if! {
                if #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))] {
                    super::QUICUpstream::new(manager, logger, tag.clone(), quic_options)
                        .map(|u| {
                            Box::new(GenericUpstream::new(u, options.query_timeout))
                                as Box<dyn adapter::Upstream>
                        })
                        .map_err(|err| anyhow::anyhow!("create quic upstream [{}] failed: {}", tag, err))
                } else {
                    Err(anyhow::anyhow("quic upstream is not supported"))
                }
            }
        }
    }
}

pub(crate) struct GenericUpstream<T: adapter::Upstream> {
    query_timeout: Option<Duration>,
    upstream: T,
}

impl<T: adapter::Upstream> GenericUpstream<T> {
    fn new(upstream: T, query_timeout: Option<Duration>) -> Self {
        Self {
            query_timeout,
            upstream,
        }
    }

    async fn exchange_wrapper(
        &self,
        log_tracker: Option<&log::Tracker>,
        request: &mut Message,
    ) -> anyhow::Result<Message> {
        let fut = self.upstream.exchange(log_tracker, request);
        match self.query_timeout {
            Some(timeout) => match tokio::time::timeout(timeout, fut).await {
                Ok(result) => result,
                Err(_) => {
                    let logger = self.upstream.logger();
                    let info = show_query(request);
                    error!(
                        logger,
                        { option_tracker = log_tracker },
                        "exchange {} timeout",
                        &info
                    );
                    Err(anyhow::anyhow!("exchange {} timeout", info))
                }
            },
            None => fut.await,
        }
    }
}

#[async_trait::async_trait]
impl<T: adapter::Upstream> adapter::Common for GenericUpstream<T> {
    async fn start(&self) -> anyhow::Result<()> {
        self.upstream.start().await
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.upstream.close().await
    }
}

#[async_trait::async_trait]
impl<T: adapter::Upstream> adapter::Upstream for GenericUpstream<T> {
    fn tag(&self) -> &str {
        self.upstream.tag()
    }

    fn r#type(&self) -> &'static str {
        self.upstream.r#type()
    }

    fn dependencies(&self) -> Option<Vec<String>> {
        self.upstream.dependencies()
    }

    fn logger(&self) -> &Arc<Box<dyn log::Logger>> {
        self.upstream.logger()
    }

    async fn exchange(
        &self,
        log_tracker: Option<&log::Tracker>,
        request: &mut Message,
    ) -> anyhow::Result<Message> {
        self.exchange_wrapper(log_tracker, request).await
    }
}

lazy_static::lazy_static! {
    static ref SUPPORTED_UPSTREAM_TYPES: Vec<&'static str> = {
        let mut types = vec!["tcp", "udp"];

        #[cfg(feature = "upstream-tls")]
        types.push("tls");

        #[cfg(all(feature = "upstream-https", feature = "upstream-tls"))]
        types.push("https");

        #[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
        types.push("quic");

        types
    };
}

pub fn supported_upstream_types() -> Vec<&'static str> {
    SUPPORTED_UPSTREAM_TYPES.clone()
}

use std::{net::SocketAddr, sync::Arc, time::Duration};

use crate::{adapter, common, fatal, info, log, option};
use hickory_proto::op::Message;
use tokio::{
    net::UdpSocket,
    sync::{mpsc, Mutex},
    task::JoinSet,
};

const UDP_LISTENER_TYPE: &str = "udp";

pub(crate) struct UDPListener {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    listen: SocketAddr,
    workflow_tag: String,
    query_timeout: Option<Duration>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl UDPListener {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::UDPListenerOptions,
    ) -> anyhow::Result<Self> {
        let listen = super::parse_with_default_port(&options.listen, 53)
            .map_err(|err| anyhow::anyhow!("invalid listen: {}", err))?;
        if options.generic.workflow.is_empty() {
            return Err(anyhow::anyhow!("missing workflow"));
        }
        Ok(Self {
            manager,
            logger: Arc::new(logger),
            tag,
            listen,
            workflow_tag: options.generic.workflow,
            query_timeout: options.generic.query_timeout,
            canceller: Mutex::new(None),
        })
    }

    async fn handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        socket: UdpSocket,
        canceller_guard: common::CancellerGuard,
    ) {
        let (sender, mut receiver) = mpsc::channel(64);
        let mut join_set = JoinSet::new();
        let mut buffer = vec![0u8; 4096];
        loop {
            tokio::select! {
                res = socket.recv_from(&mut buffer) => {
                    match res {
                        Ok((length, peer_addr)) => {
                            // TODO: Check Valid
                            let buf = buffer[..length].to_vec();
                            buffer = vec![0u8; 4096];
                            let workflow = workflow.clone();
                            let logger = logger.clone();
                            let sender = sender.clone();
                            let canceller_guard = canceller_guard.clone();
                            let listener_tag = listener_tag.clone();
                            join_set.spawn(async move {
                                let fut = async move {
                                    if let Ok(request) = Message::from_vec(&buf) {
                                        if let Some(response) = super::handle(workflow, logger, listener_tag, query_timeout, peer_addr.ip(), request).await {
                                            if let Ok(buf) = response.to_vec() {
                                                sender.send((buf, peer_addr)).await.ok();
                                            }
                                        }
                                    }
                                };
                                tokio::select! {
                                    _ = fut => {}
                                    _ = canceller_guard.cancelled() => {}
                                }
                            });
                            while futures_util::FutureExt::now_or_never(join_set.join_next()).flatten().is_some() {}
                        }
                        Err(e) => {
                            if !canceller_guard.is_cancelled() {
                                fatal!(logger, "failed to receive from UDP socket: {}", e);
                                manager.fail_to_close(format!("listener [{}]: failed to receive from UDP socket: {}", listener_tag, e)).await;
                            }
                            break;
                        }
                    }
                }
                res = receiver.recv() => {
                    if let Some((buf, peer_addr)) = res {
                        tokio::select! {
                            _ = socket.send_to(&buf, peer_addr) => {}
                            _ = canceller_guard.cancelled() => {
                                break;
                            }
                        }
                    }
                }
                _ = canceller_guard.cancelled() => {
                    break;
                }
            }
        }
        drop(sender);
        receiver.recv().await;
        join_set.shutdown().await;
    }
}

#[async_trait::async_trait]
impl adapter::Common for UDPListener {
    async fn start(&self) -> anyhow::Result<()> {
        let workflow =
            self.manager
                .get_workflow(&self.workflow_tag)
                .await
                .ok_or(anyhow::anyhow!(
                    "workflow [{}] not found",
                    self.workflow_tag
                ))?;
        let udp_socket = UdpSocket::bind(&self.listen).await?;
        info!(self.logger, "UDP socket listen on {}", self.listen,);
        let (canceller, canceller_guard) = common::new_canceller();
        let manager = self.manager.clone();
        let listener_tag = self.tag.clone();
        let logger = self.logger.clone();
        let query_timeout = self.query_timeout;
        tokio::spawn(async move {
            Self::handle(
                manager,
                workflow,
                logger,
                query_timeout,
                listener_tag,
                udp_socket,
                canceller_guard,
            )
            .await
        });
        self.canceller.lock().await.replace(canceller);
        Ok(())
    }

    async fn close(&self) -> anyhow::Result<()> {
        if let Some(mut canceller) = self.canceller.lock().await.take() {
            canceller.cancel_and_wait().await;
        }
        Ok(())
    }
}

impl adapter::Listener for UDPListener {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &str {
        UDP_LISTENER_TYPE
    }
}

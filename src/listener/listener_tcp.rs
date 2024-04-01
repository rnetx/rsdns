use std::{io::IoSlice, net::SocketAddr, sync::Arc, time::Duration};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
    task::JoinSet,
};

use crate::{adapter, common, error, fatal, info, log, option};

const TCP_LISTENER_TYPE: &str = "tcp";

pub(crate) struct TCPListener {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    listen: SocketAddr,
    workflow_tag: String,
    query_timeout: Option<Duration>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl TCPListener {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::TCPListenerOptions,
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
        listener: TcpListener,
        canceller_guard: common::CancellerGuard,
    ) {
        let mut join_set = JoinSet::new();
        loop {
            tokio::select! {
              res = listener.accept() => {
                match res {
                  Ok((stream, peer_addr)) => {
                    // TODO: Check Valid
                    let workflow = workflow.clone();
                    let logger = logger.clone();
                    let listener_tag = listener_tag.clone();
                    let canceller_guard = canceller_guard.clone();
                    join_set.spawn(async move {
                      Self::stream_handle(workflow, logger, query_timeout, listener_tag, stream, peer_addr, canceller_guard).await
                    });
                    while futures_util::FutureExt::now_or_never(join_set.join_next()).flatten().is_some() {}
                  }
                  Err(e) => {
                    if !canceller_guard.is_cancelled() {
                      fatal!(logger, "failed to listen on TCP listener: {}", e);
                      manager.fail_to_close(format!("listener [{}]: failed to listen on TCP listener: {}", listener_tag, e)).await;
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
        join_set.shutdown().await;
    }

    async fn stream_handle(
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        mut stream: TcpStream,
        peer_addr: SocketAddr,
        canceller_guard: common::CancellerGuard,
    ) {
        let mut join_set = JoinSet::new();
        let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(64);
        loop {
            tokio::select! {
              res = stream.read_u16() => {
                let length = match res {
                  Ok(v) => {
                      if v == 0 {
                          break;
                      }
                      v
                  }
                  Err(_) => {
                      break;
                  }
                };
                let mut buf = vec![0; length as usize];
                tokio::select! {
                  res = stream.read_exact(&mut buf) => {
                    match res {
                      Ok(n) => {
                        if n == 0 {
                          break;
                        }
                      }
                      Err(_) => {
                        break;
                      }
                    }
                  }
                  _ = canceller_guard.cancelled() => {
                    break;
                  }
                }
                let workflow = workflow.clone();
                let logger = logger.clone();
                let canceller_guard = canceller_guard.clone();
                let sender = sender.clone();
                let listener_tag = listener_tag.clone();
                join_set.spawn(async move {
                  let fut = async move {
                    if let Ok(request) = Message::from_vec(&buf) {
                      if let Some(response) = super::handle(workflow, logger, listener_tag, query_timeout, peer_addr.ip(), request).await {
                        if let Ok(buf) = response.to_vec() {
                            sender.send(buf).await.ok();
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
              res = receiver.recv() => {
                if let Some(buf) = res {
                  let data_length_bytes = (buf.len() as u16).to_be_bytes();
                  let vectored = [IoSlice::new(&data_length_bytes), IoSlice::new(&buf)];
                  tokio::select! {
                    res = stream.write_vectored(&vectored) => {
                      if let Err(e) = res {
                        error!(
                          logger,
                          "send response failed: {}, peer_addr: {}", e, peer_addr,
                        );
                        break;
                      }
                    }
                    _ = canceller_guard.cancelled() => {
                      break;
                    }
                  }
                  tokio::select! {
                    res = stream.flush() => {
                      if let Err(e) = res {
                        error!(
                          logger,
                          "send response failed: {}, peer_addr: {}", e, peer_addr,
                        );
                        break;
                      }
                    }
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
        join_set.shutdown().await;
    }
}

#[async_trait::async_trait]
impl adapter::Common for TCPListener {
    async fn start(&self) -> anyhow::Result<()> {
        let workflow =
            self.manager
                .get_workflow(&self.workflow_tag)
                .await
                .ok_or(anyhow::anyhow!(
                    "workflow [{}] not found",
                    self.workflow_tag
                ))?;
        let tcp_listener = TcpListener::bind(&self.listen).await?;
        info!(self.logger, "TCP listener listen on {}", self.listen,);
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
                tcp_listener,
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

impl adapter::Listener for TCPListener {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &str {
        TCP_LISTENER_TYPE
    }
}

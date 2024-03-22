use std::{error::Error, net::SocketAddr, sync::Arc, time::Duration};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
};

use crate::{adapter, common, error, fatal, info, log, option};

const TLS_LISTENER_TYPE: &str = "tls";

pub(crate) struct TLSListener {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    listen: SocketAddr,
    workflow_tag: String,
    query_timeout: Option<Duration>,
    tls_config: Arc<tokio_rustls::rustls::ServerConfig>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl TLSListener {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::TLSListenerOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let listen = super::parse_with_default_port(&options.listen, 853).map_err::<Box<
            dyn Error + Send + Sync,
        >, _>(|err| {
            format!("invalid listen: {}", err).into()
        })?;
        if options.generic.workflow.is_empty() {
            return Err("missing workflow".into());
        }
        let tls_config = super::new_tls_config(options.tls)?;
        Ok(Self {
            manager,
            logger: Arc::new(logger),
            tag,
            listen,
            workflow_tag: options.generic.workflow,
            query_timeout: options.generic.query_timeout,
            tls_config: Arc::new(tls_config),
            canceller: Mutex::new(None),
        })
    }

    async fn handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        tcp_listener: TcpListener,
        tls_acceptor: Arc<tokio_rustls::TlsAcceptor>,
        canceller_guard: common::CancellerGuard,
    ) {
        let (mut conn_canceller, conn_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = tcp_listener.accept() => {
                match res {
                  Ok((stream, peer_addr)) => {
                    // TODO: Check Valid
                    let workflow = workflow.clone();
                    let logger = logger.clone();
                    let listener_tag = listener_tag.clone();
                    let conn_canceller_guard = conn_canceller_guard.clone();
                    let tls_acceptor = tls_acceptor.clone();
                    tokio::spawn(async move {
                      let stream = tokio::select! {
                        res = tls_acceptor.accept(stream) => {
                          match res {
                            Ok(stream) => stream,
                            Err(_) => {
                              return;
                            }
                          }
                        },
                        _ = conn_canceller_guard.cancelled() => return,
                      };
                      Self::stream_handle(workflow, logger, query_timeout, listener_tag, stream, peer_addr, conn_canceller_guard).await
                    });
                  }
                  Err(e) => {
                    if !canceller_guard.is_cancelled() {
                      fatal!(logger, "failed to listen on TLS listener: {}", e);
                      manager.fail_to_close(format!("listener [{}]: failed to listen on TLS listener: {}", listener_tag, e)).await;
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
        drop(conn_canceller_guard);
        conn_canceller.cancel_and_wait().await;
    }

    async fn stream_handle(
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        mut stream: tokio_rustls::server::TlsStream<TcpStream>,
        peer_addr: SocketAddr,
        canceller_guard: common::CancellerGuard,
    ) {
        let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(64);
        let (mut stream_canceller, stream_canceller_guard) = common::new_canceller();
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
                let stream_canceller_guard = stream_canceller_guard.clone();
                let sender = sender.clone();
                let listener_tag = listener_tag.clone();
                tokio::spawn(async move {
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
                    _ = stream_canceller_guard.cancelled() => {}
                  }
                });
              }
              res = receiver.recv() => {
                if let Some(buf) = res {
                  let data_length_bytes = (buf.len() as u16).to_be_bytes();
                  let mut data_chain = bytes::Buf::chain(&data_length_bytes[..], buf.as_slice());
                  tokio::select! {
                    res = stream.write_all_buf(&mut data_chain) => {
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
        drop(stream_canceller_guard);
        stream_canceller.cancel_and_wait().await;
    }
}

#[async_trait::async_trait]
impl adapter::Common for TLSListener {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let workflow = self
            .manager
            .get_workflow(&self.workflow_tag)
            .await
            .ok_or(format!("workflow [{}] not found", self.workflow_tag))?;
        let tcp_listener = TcpListener::bind(&self.listen).await?;
        info!(self.logger, "TLS listener listen on {}", self.listen,);
        let tls_acceptor = Arc::new(tokio_rustls::TlsAcceptor::from(__self.tls_config.clone()));
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
                tls_acceptor,
                canceller_guard,
            )
            .await
        });
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

impl adapter::Listener for TLSListener {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &str {
        TLS_LISTENER_TYPE
    }
}

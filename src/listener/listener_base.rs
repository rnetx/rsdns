use std::{
    error::Error,
    io::IoSlice,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use crate::{adapter, common, error, fatal, info, log, option};
use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, Mutex},
};

const BASE_LISTENER_TYPE: &str = "base";

pub(crate) struct BaseListener {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    listen_port: u16,
    disable_ipv6: bool,
    workflow_tag: String,
    query_timeout: Option<Duration>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl BaseListener {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::BaseListenerOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        if options.listen_port == 0 {
            return Err("missing listen-port".into());
        }
        if options.generic.workflow.is_empty() {
            return Err("missing workflow".into());
        }
        Ok(Self {
            manager,
            logger: Arc::new(logger),
            tag,
            listen_port: options.listen_port,
            disable_ipv6: options.disable_ipv6,
            workflow_tag: options.generic.workflow,
            query_timeout: options.generic.query_timeout,
            canceller: Mutex::new(None),
        })
    }

    async fn udp_handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        socket: UdpSocket,
        canceller_guard: common::CancellerGuard,
    ) {
        let (sender, mut receiver) = mpsc::channel(64);
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
                            tokio::spawn(async move {
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
    }

    async fn tcp_handle(
        manager: Arc<Box<dyn adapter::Manager>>,
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        listener: TcpListener,
        canceller_guard: common::CancellerGuard,
    ) {
        let (mut conn_canceller, conn_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = listener.accept() => {
                match res {
                  Ok((stream, peer_addr)) => {
                    // TODO: Check Valid
                    let workflow = workflow.clone();
                    let logger = logger.clone();
                    let listener_tag = listener_tag.clone();
                    let conn_canceller_guard = conn_canceller_guard.clone();
                    tokio::spawn(async move {
                      Self::tcp_stream_handle(workflow, logger, query_timeout, listener_tag, stream, peer_addr, conn_canceller_guard).await
                    });
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
        drop(conn_canceller_guard);
        conn_canceller.cancel_and_wait().await;
    }

    async fn tcp_stream_handle(
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        mut stream: TcpStream,
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
        drop(stream_canceller_guard);
        stream_canceller.cancel_and_wait().await;
    }
}

#[async_trait::async_trait]
impl adapter::Common for BaseListener {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let workflow = self
            .manager
            .get_workflow(&self.workflow_tag)
            .await
            .ok_or(format!("workflow [{}] not found", self.workflow_tag))?;
        let (canceller, canceller_guard) = common::new_canceller();
        {
            // UDP IPv4
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), self.listen_port);
            let udp_socket = UdpSocket::bind(&addr).await?;
            info!(self.logger, "UDP socket listen on {}", addr);
            let manager = self.manager.clone();
            let workflow = workflow.clone();
            let listener_tag = self.tag.clone();
            let logger = self.logger.clone();
            let query_timeout = self.query_timeout;
            let canceller_guard = canceller_guard.clone();
            tokio::spawn(async move {
                Self::udp_handle(
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
        }
        {
            // TCP IPv4
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), self.listen_port);
            let tcp_listener = TcpListener::bind(&addr).await?;
            info!(self.logger, "TCP listener listen on {}", addr);
            let manager = self.manager.clone();
            let workflow = workflow.clone();
            let listener_tag = self.tag.clone();
            let logger = self.logger.clone();
            let query_timeout = self.query_timeout;
            let canceller_guard = canceller_guard.clone();
            tokio::spawn(async move {
                Self::tcp_handle(
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
        }
        if !self.disable_ipv6 {
            {
                // UDP IPv6
                let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), self.listen_port);
                let udp_socket = UdpSocket::bind(&addr).await?;
                info!(self.logger, "UDP socket listen on {}", addr);
                let manager = self.manager.clone();
                let workflow = workflow.clone();
                let listener_tag = self.tag.clone();
                let logger = self.logger.clone();
                let query_timeout = self.query_timeout;
                let canceller_guard = canceller_guard.clone();
                tokio::spawn(async move {
                    Self::udp_handle(
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
            }
            {
                // TCP IPv6
                let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), self.listen_port);
                let tcp_listener = TcpListener::bind(&addr).await?;
                info!(self.logger, "TCP listener listen on {}", addr);
                let manager = self.manager.clone();
                let workflow = workflow.clone();
                let listener_tag = self.tag.clone();
                let logger = self.logger.clone();
                let query_timeout = self.query_timeout;
                let canceller_guard = canceller_guard.clone();
                tokio::spawn(async move {
                    Self::tcp_handle(
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

impl adapter::Listener for BaseListener {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &str {
        BASE_LISTENER_TYPE
    }
}

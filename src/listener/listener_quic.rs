use std::{
    error::Error,
    net::{SocketAddr, UdpSocket},
    sync::Arc,
    time::Duration,
};

use hickory_proto::op::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};

use crate::{adapter, common, fatal, info, log, option};

const QUIC_LISTENER_TYPE: &str = "quic";

pub(crate) struct QUICListener {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    listen: SocketAddr,
    workflow_tag: String,
    query_timeout: Option<Duration>,
    tls_config: Arc<tokio_rustls::rustls::ServerConfig>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl QUICListener {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::QUICListenerOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let listen = super::parse_with_default_port(&options.listen, 853).map_err::<Box<
            dyn Error + Send + Sync,
        >, _>(|err| {
            format!("invalid listen: {}", err).into()
        })?;
        if options.generic.workflow.is_empty() {
            return Err("missing workflow".into());
        }
        let mut tls_config = super::new_tls_config(options.tls)?;
        tls_config.alpn_protocols = vec![b"doq".into()];
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
        endpoint: quinn::Endpoint,
        canceller_guard: common::CancellerGuard,
    ) {
        let (mut endpoint_canceller, endpoint_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = endpoint.accept() => {
                match res {
                    Some(c) => {
                        let workflow = workflow.clone();
                        let logger = logger.clone();
                        let endpoint_canceller_guard = endpoint_canceller_guard.clone();
                        let listener_tag = listener_tag.clone();
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
                            _ = endpoint_canceller_guard.cancelled() => {
                                return;
                            }
                          };
                          Self::conn_handle(workflow, logger, query_timeout, listener_tag, conn, endpoint_canceller_guard).await;
                        });
                    }
                    None => {
                        if !canceller_guard.is_cancelled() {
                            fatal!(logger, "failed to accept QUIC connection");
                            manager.fail_to_close(format!("listener [{}]: failed to accept QUIC connection", listener_tag)).await;
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
        drop(endpoint_canceller_guard);
        endpoint_canceller.cancel_and_wait().await;
        endpoint.close(0u32.into(), b"");
    }

    async fn conn_handle(
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        conn: quinn::Connection,
        canceller_guard: common::CancellerGuard,
    ) {
        let (mut conn_canceller, conn_canceller_guard) = common::new_canceller();
        loop {
            tokio::select! {
              res = conn.accept_bi() => {
                if let Ok((send_stream, recv_stream)) = res {
                  let workflow = workflow.clone();
                  let logger = logger.clone();
                  let conn_canceller_guard = conn_canceller_guard.clone();
                  let peer_addr = conn.remote_address();
                  let listener_tag = listener_tag.clone();
                  tokio::spawn(async move {
                    Self::stream_handle(workflow, logger, query_timeout, listener_tag, send_stream, recv_stream, peer_addr, conn_canceller_guard).await;
                  });
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
        drop(conn_canceller_guard);
        conn_canceller.cancel_and_wait().await;
        conn.close(0u32.into(), b"");
    }

    async fn stream_handle(
        workflow: Arc<Box<dyn adapter::Workflow>>,
        logger: Arc<Box<dyn log::Logger>>,
        query_timeout: Option<Duration>,
        listener_tag: String,
        mut send_stream: quinn::SendStream,
        mut recv_stream: quinn::RecvStream,
        peer_addr: SocketAddr,
        canceller_guard: common::CancellerGuard,
    ) {
        let length = tokio::select! {
            res = recv_stream.read_u16() => {
                match res {
                    Ok(v) => {
                        if v == 0 {
                            return;
                        }
                        v
                    }
                    Err(_) => {
                        return;
                    }
                }
            }
            _ = canceller_guard.cancelled() => {
                return;
            }
        };
        let mut buf = vec![0; length as usize];
        tokio::select! {
            res = recv_stream.read_exact(&mut buf) => {
                if let Err(_) = res {
                    return;
                }
            }
            _ = canceller_guard.cancelled() => {
                return;
            }
        }
        let fut = async move {
            if let Ok(request) = Message::from_vec(&buf) {
                if let Some(response) = super::handle(
                    workflow,
                    logger,
                    listener_tag,
                    query_timeout,
                    peer_addr.ip(),
                    request,
                )
                .await
                {
                    if let Ok(buf) = response.to_vec() {
                        let data_length_bytes = (buf.len() as u16).to_be_bytes();
                        let mut chain = bytes::Buf::chain(&data_length_bytes[..], buf.as_slice());
                        if let Ok(_) = send_stream.write_all_buf(&mut chain).await {
                            send_stream.finish().await.ok();
                        }
                    }
                }
            }
        };
        tokio::select! {
            _ = fut => {}
            _ = canceller_guard.cancelled() => {}
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for QUICListener {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let workflow = self
            .manager
            .get_workflow(&self.workflow_tag)
            .await
            .ok_or(format!("workflow [{}] not found", self.workflow_tag))?;
        let udp_socket = UdpSocket::bind(&self.listen)?;
        let quic_server_config = quinn_proto::ServerConfig::with_crypto(self.tls_config.clone());
        let quic_endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(quic_server_config),
            udp_socket,
            Arc::new(quinn::TokioRuntime),
        )?;
        info!(self.logger, "QUIC endpoint listen on {}", self.listen,);
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
                quic_endpoint,
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

impl adapter::Listener for QUICListener {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &str {
        QUIC_LISTENER_TYPE
    }
}

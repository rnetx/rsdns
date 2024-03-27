use std::{collections::HashMap, error::Error, net::IpAddr, pin::Pin, sync::Arc, time::Duration};

use futures_util::Future;
use rand::Rng;
use serde::Deserialize;
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::{adapter, common, debug, error, log};

pub(crate) const TYPE: &str = "ipset";

#[derive(Deserialize)]
struct Options {
    #[serde(default)]
    #[serde(rename = "ipv4-name")]
    ipv4_name: Option<String>,
    #[serde(default)]
    #[serde(rename = "ipv6-name")]
    ipv6_name: Option<String>,
    #[serde(default)]
    #[serde(rename = "ipv4-mask")]
    ipv4_mask: Option<u8>,
    #[serde(default)]
    #[serde(rename = "ipv6-mask")]
    ipv6_mask: Option<u8>,
    #[serde(default)]
    #[serde(rename = "ipv4-timeout")]
    ipv4_timeout: Option<Duration>,
    #[serde(default)]
    #[serde(rename = "ipv6-timeout")]
    ipv6_timeout: Option<Duration>,
}

#[derive(Deserialize, Clone)]
struct WorkflowArgs {
    #[serde(default)]
    mode: WorkflowMode,
}

#[derive(Deserialize, Clone, Copy)]
enum WorkflowMode {
    #[serde(rename = "add-response-ip")]
    AddResponseIP,
    #[serde(rename = "add-client-ip")]
    AddClientIP,
}

impl Default for WorkflowMode {
    fn default() -> Self {
        Self::AddResponseIP
    }
}

#[derive(Clone, Copy)]
enum IPSetCommand {
    Flush,
    List,
}

enum IPSetCommandResult {
    Flush(Result<(), String>),
    List(Result<Vec<String>, String>),
}

pub(crate) struct IPSet {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    //
    ipv4_name: Option<String>,
    ipv6_name: Option<String>,
    ipv4_mask: Option<u8>,
    ipv6_mask: Option<u8>,
    ipv4_timeout: Option<Duration>,
    ipv6_timeout: Option<Duration>,
    //
    args_map: RwLock<HashMap<u16, WorkflowArgs>>,
    ip_sender: RwLock<Option<mpsc::Sender<(IpAddr, Option<u32>)>>>,
    command_sender: std::sync::RwLock<
        Option<
            mpsc::Sender<(
                IPSetCommand,
                Box<
                    dyn FnOnce(IPSetCommandResult) -> Pin<Box<dyn Future<Output = ()> + Send>>
                        + Send,
                >,
            )>,
        >,
    >,
    canceller: Mutex<Option<common::Canceller>>,
}

impl IPSet {
    pub(crate) fn new(
        _: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: serde_yaml::Value,
    ) -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>> {
        let options =
            Options::deserialize(options).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize options: {}", err).into()
            })?;
        let logger = Arc::new(logger);

        match (&options.ipv4_name, &options.ipv6_name) {
            (Some(_), Some(_)) => {}
            _ => return Err("missing ipv4-name and ipv6-name".into()),
        }

        let s = Self {
            tag,
            logger,
            ipv4_name: options.ipv4_name,
            ipv6_name: options.ipv6_name,
            ipv4_mask: options.ipv4_mask,
            ipv6_mask: options.ipv6_mask,
            ipv4_timeout: options.ipv4_timeout,
            ipv6_timeout: options.ipv6_timeout,
            args_map: RwLock::new(HashMap::new()),
            ip_sender: RwLock::new(None),
            command_sender: std::sync::RwLock::new(None),
            canceller: Mutex::new(None),
        };

        Ok(Box::new(s))
    }

    async fn handle(
        logger: Arc<Box<dyn log::Logger>>,
        mut ip_receiver: mpsc::Receiver<(IpAddr, Option<u32>)>,
        mut command_receiver: mpsc::Receiver<(
            IPSetCommand,
            Box<dyn FnOnce(IPSetCommandResult) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>,
        )>,
        ipv4_name: Option<String>,
        ipv6_name: Option<String>,
        ipv4_mask: Option<u8>,
        ipv6_mask: Option<u8>,
        mut session4: Option<ipset::Session<ipset::types::HashNet>>,
        mut session6: Option<ipset::Session<ipset::types::HashNet>>,
        canceller_guard: common::CancellerGuard,
    ) {
        loop {
            tokio::select! {
              res = ip_receiver.recv() => {
                if let Some((ip, timeout)) = res {
                  match ip {
                    IpAddr::V4(ip) => {
                      if let Some(session4) = session4.as_mut() {
                        match session4.add(ipset::types::NetDataType::new(ip, ipv4_mask.unwrap_or(32)), timeout) {
                          Ok(_) => {
                            debug!(logger, "add ip to ipset: {} => {}", ip, ipv4_name.as_ref().unwrap());
                          }
                          Err(e) => {
                            error!(logger, "failed to add ip to ipset: {} => {}", e, ipv4_name.as_ref().unwrap());
                          }
                        }
                      }
                    }
                    IpAddr::V6(ip) => {
                      if let Some(session6) = session6.as_mut() {
                        match session6.add(ipset::types::NetDataType::new(ip, ipv6_mask.unwrap_or(128)), timeout) {
                          Ok(_) => {
                            debug!(logger, "add ip to ipset: {} => {}", ip, ipv6_name.as_ref().unwrap());
                          }
                          Err(e) => {
                            error!(logger, "failed to add ip to ipset: {} => {}", e, ipv6_name.as_ref().unwrap());
                          }
                        }
                      }
                    }
                  }
                }
              }
              res = command_receiver.recv() => {
                if let Some((command, callback)) = res {
                  match command {
                    IPSetCommand::Flush => {
                      let res1 = if let Some(session4) = session4.as_mut() {
                        session4.flush().map(|_| ()).map_err(|e| e.to_string())
                      } else {
                        Ok(())
                      };
                      let res2 = if let Some(session6) = session6.as_mut() {
                        session6.flush().map(|_| ()).map_err(|e| e.to_string())
                      } else {
                        Ok(())
                      };
                      let res = match (res1, res2) {
                        (Err(e1), Err(e2)) => {
                          Err(format!("failed to flush ipset (ipv4): {} | failed to flush ipset (ipv6): {}", e1, e2))
                        }
                        (Err(e1), Ok(_)) => Err(format!("failed to flush ipset (ipv4): {}", e1)),
                        (Ok(_), Err(e2)) => Err(format!("failed to flush ipset (ipv6): {}", e2)),
                        (Ok(_), Ok(_)) => Ok(()),
                      };
                      tokio::spawn(callback(IPSetCommandResult::Flush(res)));
                    }
                    IPSetCommand::List => {
                      let res1 = if let Some(session4) = session4.as_mut() {
                        session4.list().map_err(|e| e.to_string())
                      } else {
                        Ok(vec![])
                      };
                      let res2 = if let Some(session6) = session6.as_mut() {
                        session6.list().map_err(|e| e.to_string())
                      } else {
                        Ok(vec![])
                      };
                      let res = match (res1, res2) {
                        (Err(e1), Err(e2)) => {
                          Err(format!("failed to list ipset (ipv4): {} | failed to list ipset (ipv6): {}", e1, e2))
                        }
                        (Err(e1), Ok(_)) => Err(format!("failed to list ipset (ipv4): {}", e1)),
                        (Ok(_), Err(e2)) => Err(format!("failed to list ipset (ipv6): {}", e2)),
                        (Ok(list1), Ok(list2)) => {
                          let mut list = list1.into_iter().chain(list2.into_iter()).map(|item| {
                            if item.ip().is_ipv4() && item.cidr() == 32 {
                              item.ip().to_string()
                            } else if item.ip().is_ipv6() && item.cidr() == 128 {
                              item.ip().to_string()
                            } else {
                              item.to_string()
                            }
                          }).collect::<Vec<_>>();
                          list.shrink_to_fit();
                          Ok(list)
                        },
                      };
                      tokio::spawn(callback(IPSetCommandResult::List(res)));
                    }
                  }
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for IPSet {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let (session4, ipv4_name, ipv4_mask) = if let Some(ipv4_name) = self.ipv4_name.as_ref() {
            let mut session = ipset::Session::<ipset::types::HashNet>::new(ipv4_name.clone());
            session
                .create(|mut c| {
                    if let Some(ipv4_timeout) = self.ipv4_timeout.as_ref() {
                        c = c.with_timeout(ipv4_timeout.as_secs() as u32)?;
                    }
                    c.with_ipv6(false)?.build()
                })
                .map_err::<Box<dyn Error + Send + Sync>, _>(|e| {
                    format!("failed to create ipset session (ipv4): {}", e).into()
                })?;
            (
                Some(session),
                Some(ipv4_name.clone()),
                self.ipv4_mask.clone(),
            )
        } else {
            (None, None, None)
        };
        let (session6, ipv6_name, ipv6_mask) = if let Some(ipv6_name) = self.ipv6_name.as_ref() {
            let mut session = ipset::Session::<ipset::types::HashNet>::new(ipv6_name.clone());
            session
                .create(|mut c| {
                    if let Some(ipv6_timeout) = self.ipv6_timeout.as_ref() {
                        c = c.with_timeout(ipv6_timeout.as_secs() as u32)?;
                    }
                    c.with_ipv6(true)?.build()
                })
                .map_err::<Box<dyn Error + Send + Sync>, _>(|e| {
                    format!("failed to create ipset session (ipv6): {}", e).into()
                })?;
            (
                Some(session),
                Some(ipv6_name.clone()),
                self.ipv6_mask.clone(),
            )
        } else {
            (None, None, None)
        };
        let (ip_sender, ip_receiver) = mpsc::channel(128);
        let (command_sender, command_receiver) = mpsc::channel(8);
        let (canceller, canceller_guard) = common::new_canceller();
        let logger = self.logger.clone();
        tokio::spawn(async move {
            Self::handle(
                logger,
                ip_receiver,
                command_receiver,
                ipv4_name,
                ipv6_name,
                ipv4_mask,
                ipv6_mask,
                session4,
                session6,
                canceller_guard,
            )
            .await;
        });
        self.ip_sender.write().await.replace(ip_sender);
        self.command_sender.write().unwrap().replace(command_sender);
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

#[async_trait::async_trait]
impl adapter::ExecutorPlugin for IPSet {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(
        &self,
        args: serde_yaml::Value,
    ) -> Result<u16, Box<dyn Error + Send + Sync>> {
        let args = if args.is_string() {
            WorkflowMode::deserialize(args)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to deserialize args: {}", err).into()
                })
                .map(|mode| WorkflowArgs { mode })?
        } else {
            WorkflowArgs::deserialize(args).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize args: {}", err).into()
            })?
        };
        let mut args_map = self.args_map.write().await;
        let mut rng = rand::thread_rng();
        loop {
            let args_id = rng.gen::<u16>();
            if !args_map.contains_key(&args_id) {
                args_map.insert(args_id, args);
                return Ok(args_id);
            }
        }
    }

    async fn execute(
        &self,
        ctx: &mut adapter::Context,
        args_id: u16,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        let args = self.args_map.read().await.get(&args_id).cloned().unwrap();
        match args.mode {
            WorkflowMode::AddResponseIP => {
                if let Some(response) = ctx.response() {
                    let ip_sender = self.ip_sender.read().await.clone().unwrap();
                    for answer in response.answers() {
                        let timeout = answer.ttl();
                        if let Some(data) = answer.data() {
                            if let Some(a) = data.as_a() {
                                ip_sender.send((a.0.into(), Some(timeout))).await.ok();
                            }
                            if let Some(aaaa) = data.as_aaaa() {
                                ip_sender.send((aaaa.0.into(), Some(timeout))).await.ok();
                            }
                        }
                    }
                }
            }
            WorkflowMode::AddClientIP => {
                let client_ip = ctx.client_ip();
                let ip_sender = self.ip_sender.read().await.clone().unwrap();
                ip_sender.send((*client_ip, None)).await.ok();
            }
        }
        Ok(adapter::ReturnMode::Continue)
    }

    #[cfg(feature = "api")]
    fn api_router(&self) -> Option<axum::Router> {
        Some(api::APIHandler::new(self).api_router())
    }
}

#[cfg(feature = "api")]
mod api {
    use std::{pin::Pin, sync::Arc};

    use axum::response::IntoResponse;
    use futures_util::Future;
    use tokio::sync::{mpsc, oneshot};

    use crate::{common, debug, error, log};

    pub(crate) struct APIHandler {
        logger: Arc<Box<dyn log::Logger>>,
        command_sender: mpsc::Sender<(
            super::IPSetCommand,
            Box<
                dyn FnOnce(super::IPSetCommandResult) -> Pin<Box<dyn Future<Output = ()> + Send>>
                    + Send,
            >,
        )>,
    }

    impl APIHandler {
        pub(super) fn new(i: &super::IPSet) -> Arc<Self> {
            Arc::new(Self {
                logger: i.logger.clone(),
                command_sender: i.command_sender.read().unwrap().clone().unwrap(),
            })
        }

        pub(super) fn api_router(self: Arc<Self>) -> axum::Router {
            axum::Router::new()
                .route("/flush", axum::routing::delete(Self::flush))
                .route("/list", axum::routing::get(Self::list))
                .with_state(self)
        }

        // DELETE /flush
        async fn flush(
            ctx: common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl IntoResponse {
            let (sender, receiver) = oneshot::channel();
            let callback = Box::new(move |res| {
                future_into_pin_box(async move {
                    sender.send(res).ok();
                })
            });
            if ctx
                .state
                .command_sender
                .send((super::IPSetCommand::Flush, callback))
                .await
                .is_err()
            {
                return common::GenericResponse::new_empty(http::StatusCode::INTERNAL_SERVER_ERROR);
            }
            if let Some(res) = receiver.await.ok() {
                match res {
                    super::IPSetCommandResult::Flush(Ok(_)) => {
                        debug!(ctx.state.logger, "flush ipset success");
                        return common::GenericResponse::new_empty(http::StatusCode::NO_CONTENT);
                    }
                    super::IPSetCommandResult::Flush(Err(e)) => {
                        error!(ctx.state.logger, "flush ipset failed: {}", e);
                        return common::GenericResponse::new_empty(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                        );
                    }
                    _ => unreachable!(),
                }
            }
            common::GenericResponse::new_empty(http::StatusCode::INTERNAL_SERVER_ERROR)
        }

        // GET /list
        async fn list(ctx: common::GenericStateRequestContext<Arc<Self>, ()>) -> impl IntoResponse {
            let (sender, receiver) = oneshot::channel();
            let callback = Box::new(move |res| {
                future_into_pin_box(async move {
                    sender.send(res).ok();
                })
            });
            if ctx
                .state
                .command_sender
                .send((super::IPSetCommand::List, callback))
                .await
                .is_err()
            {
                return common::GenericResponse::new_empty(http::StatusCode::INTERNAL_SERVER_ERROR);
            }
            if let Some(res) = receiver.await.ok() {
                match res {
                    super::IPSetCommandResult::List(Ok(list)) => {
                        return common::GenericResponse::new(http::StatusCode::OK, list.into());
                    }
                    super::IPSetCommandResult::List(Err(e)) => {
                        error!(ctx.state.logger, "list ipset failed: {}", e);
                        return common::GenericResponse::new_empty(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                        );
                    }
                    _ => unreachable!(),
                }
            }
            common::GenericResponse::new_empty(http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }

    fn future_into_pin_box<O>(
        f: impl Future<Output = O> + Send + 'static,
    ) -> Pin<Box<dyn Future<Output = O> + Send>> {
        Box::pin(f)
    }
}

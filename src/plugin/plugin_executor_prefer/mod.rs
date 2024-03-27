use std::{collections::HashMap, error::Error, sync::Arc};

use hickory_proto::{op::{Message, MessageType, OpCode}, rr::RecordType};
use rand::Rng;
use serde::Deserialize;
use tokio::{sync::RwLock, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{adapter, common, debug, error, log};

pub(crate) const TYPE: &str = "prefer";

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Strategy {
    #[serde(rename = "prefer-ipv4")]
    PreferIPv4,
    #[serde(rename = "prefer-ipv6")]
    PreferIPv6,
    #[serde(rename = "only-ipv4")]
    OnlyIPv4,
    #[serde(rename = "only-ipv6")]
    OnlyIPv6,
}

impl Default for Strategy {
    fn default() -> Self {
        Self::PreferIPv4
    }
}

#[derive(Deserialize)]
struct Options {
    upstream: String,
    #[serde(default)]
    strategy: Strategy,
}

type WorkflowArgsPrepare = Options;

#[derive(Clone)]
struct WorkflowArgs {
    upstream: Arc<Box<dyn adapter::Upstream>>,
    strategy: Strategy,
}

pub(crate) struct Prefer {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    // Options
    upstream_tag: Option<String>,
    strategy: Option<Strategy>,
    upstream: RwLock<Option<Arc<Box<dyn adapter::Upstream>>>>,
    //
    args_map: RwLock<HashMap<u16, Option<WorkflowArgs>>>,
}

impl Prefer {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        mut options: serde_yaml::Value,
    ) -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>> {
        if options.as_mapping().map(|m| m.len() == 0).unwrap_or(false) {
            options = serde_yaml::Value::Null;
        }
        let (upstream_tag, strategy) = if !options.is_null() {
            let options = Options::deserialize(options)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to deserialize options: {}", err).into()
                })?;
            if options.upstream.is_empty() {
                return Err("missing upstream".into());
            }
            (Some(options.upstream), Some(options.strategy))
        } else {
            (None, None)
        };
        let logger = Arc::new(logger);

        let s = Self {
            manager,
            logger,
            tag,
            upstream_tag,
            strategy,
            upstream: RwLock::new(None),
            args_map: RwLock::new(HashMap::new()),
        };

        Ok(Box::new(s))
    }

    // Return: Is A|AAAA request
    fn check_is_ip_request(request: &Message) -> Option<RecordType> {
        if let Some(query) = request.query() {
            return match query.query_type() {
                RecordType::A | RecordType::AAAA => Some(query.query_type()),
                _ => None,
            };
        }
        None
    }

    fn generate_extra_message(request: &Message, query_type: RecordType) -> Message {
        let mut new_request = Message::new();
        new_request.set_id(rand::thread_rng().gen());
        new_request.set_op_code(OpCode::Query);
        new_request.set_message_type(MessageType::Query);
        new_request.set_recursion_desired(true);
        if let Some(mut query) = request.query().cloned() {
            query.set_query_type(query_type);
            new_request.add_query(query);
        }
        new_request
    }
}

#[async_trait::async_trait]
impl adapter::Common for Prefer {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let (Some(upstream_tag), Some(_)) = (&self.upstream_tag, &self.strategy) {
            let upstream = self
                .manager
                .get_upstream(upstream_tag)
                .await
                .ok_or_else::<Box<dyn Error + Send + Sync>, _>(|| {
                    format!("upstream [{}] not found", upstream_tag).into()
                })?;
            self.upstream.write().await.replace(upstream);
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::ExecutorPlugin for Prefer {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(
        &self,
        mut args: serde_yaml::Value,
    ) -> Result<u16, Box<dyn Error + Send + Sync>> {
        if args.as_mapping().map(|m| m.len() == 0).unwrap_or(false) {
            args = serde_yaml::Value::Null;
        }
        let args = if !args.is_null() {
            let args = WorkflowArgsPrepare::deserialize(args)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to deserialize args: {}", err).into()
                })?;
            let upstream = self
                .manager
                .get_upstream(&args.upstream)
                .await
                .ok_or_else::<Box<dyn Error + Send + Sync>, _>(|| {
                    format!("upstream [{}] not found", args.upstream).into()
                })?;
            Some(WorkflowArgs {
                upstream,
                strategy: args.strategy,
            })
        } else {
            None
        };
        if args.is_none() && self.upstream_tag.is_none() {
            return Err("missing args, because plugin options is empty".into());
        }
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
        let request_type = Self::check_is_ip_request(ctx.request());
        if request_type.is_none() {
            debug!(
                self.logger,
                { tracker = ctx.log_tracker() },
                "not a A|AAAA request, skip"
            );
            return Ok(adapter::ReturnMode::Continue);
        }
        let request_type = request_type.unwrap();

        let args = self.args_map.read().await.get(&args_id).cloned().unwrap();

        let (upstream, strategy) = match args {
            Some(args) => (args.upstream, args.strategy),
            None => (
                self.upstream.read().await.clone().unwrap(),
                self.strategy.unwrap(),
            ),
        };

        match (strategy, request_type) {
            (Strategy::PreferIPv4 | Strategy::OnlyIPv4, RecordType::A)
            | (Strategy::PreferIPv6 | Strategy::OnlyIPv6, RecordType::AAAA) => {
                let log_tracker = ctx.log_tracker().clone();
                let response = match upstream
                    .exchange(Some(&log_tracker), ctx.request_mut())
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        error!(
                            self.logger,
                            { tracker = log_tracker },
                            "failed to exchange: {}",
                            e
                        );
                        return Err(e);
                    }
                };
                ctx.replace_response(response);
            }
            (Strategy::OnlyIPv4, RecordType::AAAA) | (Strategy::OnlyIPv6, RecordType::A) => {
                debug!(
                    self.logger,
                    { tracker = ctx.log_tracker() },
                    "query is {} request, but strategy is {}, generate empty response",
                    if request_type == RecordType::AAAA {
                        "AAAA"
                    } else {
                        "A"
                    },
                    if strategy == Strategy::OnlyIPv4 {
                        "only-ipv4"
                    } else {
                        "only-ipv6"
                    }
                );
                let response = common::generate_empty_message(ctx.request());
                ctx.replace_response(response);
            }
            _ => {
                // PreferIPv4, But request is AAAA | PreferIPv6, But request is A
                let upstream_extra = upstream.clone();
                let mut request = ctx.request().clone();
                let mut request_extra = match request_type {
                    RecordType::A => Self::generate_extra_message(ctx.request(), RecordType::AAAA), // Generate AAAA Request
                    RecordType::AAAA => Self::generate_extra_message(ctx.request(), RecordType::A), // Generate A Request
                    _ => unreachable!(),
                };
                let log_tracker = ctx.log_tracker().clone();
                let log_tracker_extra = log_tracker.clone();
                let token = CancellationToken::new();
                let token_handle = token.clone();
                let token_handle_extra = token.clone();
                let mut join_set = JoinSet::new(); // Return: (Result<(Response, IsSOA), Error>, IsExtra)
                join_set.spawn(async move {
                    tokio::select! {
                        _ = token_handle.cancelled_owned() => {
                            (Err("cancelled".into()), false)
                        }
                        res = upstream.exchange(Some(&log_tracker), &mut request) => {
                            let res = res.map(|response| {
                                let mut is_soa = false;
                                for answer in response.answers() {
                                    if let Some(data) = answer.data() {
                                        if !(data.is_a() || data.is_aaaa()) || data.is_soa() {
                                            is_soa = true;
                                            break;
                                        }
                                    }
                                }
                                (response, is_soa)
                            });
                            (res, false)
                        }
                    }
                });
                join_set.spawn(async move {
                    tokio::select! {
                        _ = token_handle_extra.cancelled_owned() => {
                            (Err("cancelled".into()), false)
                        }
                        res = upstream_extra.exchange(Some(&log_tracker_extra), &mut request_extra) => {
                            let res = res.map(|response| {
                                let mut is_soa = false;
                                for answer in response.answers() {
                                    if let Some(data) = answer.data() {
                                        if !(data.is_a() || data.is_aaaa()) || data.is_soa() {
                                            is_soa = true;
                                            break;
                                        }
                                    }
                                }
                                (response, is_soa)
                            });
                            (res, true)
                        }
                    }
                });
                let mut err = None;
                let mut ok = false;
                while let Some(Ok((res, is_extra))) = join_set.join_next().await {
                    match res {
                        Ok((mut response, is_soa)) => match (is_extra, is_soa) {
                            (false, false) | (true, true) => {
                                debug!(
                                    self.logger,
                                    { tracker = ctx.log_tracker() },
                                    "exchange success, strategy: {}",
                                    if strategy == Strategy::PreferIPv4 {
                                        "prefer-ipv4"
                                    } else {
                                        "prefer-ipv6"
                                    }
                                );
                                response.set_id(ctx.request().id());
                                ctx.replace_response(response);
                                ok = true;
                                token.cancel();
                                break;
                            }
                            (false, true) => {
                                debug!(
                                    self.logger,
                                    { tracker = ctx.log_tracker() },
                                    "exchange success, but strategy: {}, generate empty response",
                                    if strategy == Strategy::PreferIPv4 {
                                        "prefer-ipv4"
                                    } else {
                                        "prefer-ipv6"
                                    }
                                );
                                let empty_response = common::generate_empty_message(ctx.request());
                                ctx.replace_response(empty_response);
                                ok = true;
                                token.cancel();
                                break;
                            }
                            (true, false) => {
                                continue;
                            }
                        },
                        Err(e) => {
                            if err.is_some() {
                                err = Some(format!(
                                    "{} | {}: {}",
                                    err.unwrap(),
                                    if is_extra {
                                        "prefer-extra-request"
                                    } else {
                                        "request"
                                    },
                                    e
                                ));
                            } else {
                                err = Some(format!(
                                    "{}: {}",
                                    if is_extra {
                                        "prefer-extra-request"
                                    } else {
                                        "request"
                                    },
                                    e
                                ));
                            }
                        }
                    }
                }
                join_set.abort_all();
                if !ok {
                    let err = err.unwrap();
                    error!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "failed to exchange: {}",
                        err
                    );
                    return Err(err.into());
                }
            }
        }

        Ok(adapter::ReturnMode::Continue)
    }
}

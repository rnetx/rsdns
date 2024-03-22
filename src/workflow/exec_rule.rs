use std::{
    collections::HashMap,
    error::Error,
    net::IpAddr,
    ops::{Deref, DerefMut},
    str::FromStr,
    sync::Arc,
};

use hickory_proto::{
    op::ResponseCode,
    rr::{RData, Record, RecordType},
};
use tokio::sync::RwLock;

use crate::{adapter, common, debug, error, log, option};

pub(super) enum ExecItemRule {
    Mark(i16),
    Metadata(HashMap<String, String>),
    SetTTL(u32),
    SetRespIP(Vec<IpAddr>),
    Plugin(RwLock<super::ExecPlugin>),
    Upstream(RwLock<(Option<String>, Option<Arc<Box<dyn adapter::Upstream>>>)>),
    JumpTo(
        RwLock<(
            Option<Vec<String>>,
            Option<Vec<Arc<Box<dyn adapter::Workflow>>>>,
        )>,
    ),
    GoTo(RwLock<(Option<String>, Option<Arc<Box<dyn adapter::Workflow>>>)>),
    CleanResp,
    Return(Return),
}

impl ExecItemRule {
    pub(super) fn new(
        options: option::ExecItemRuleOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        match options {
            option::ExecItemRuleOptions::Mark(mark) => Ok(Self::Mark(mark)),
            option::ExecItemRuleOptions::Metadata(m) => {
                let mut map = HashMap::with_capacity(m.len());
                for (k, v) in m {
                    let k = if let serde_yaml::Value::String(k) = k {
                        k
                    } else {
                        return Err(format!("invalid metadata: key: {:?}", k).into());
                    };
                    let kk = k.trim();
                    if kk.is_empty() {
                        return Err("invalid metadata: empty key".into());
                    }
                    if let serde_yaml::Value::String(v) = v {
                        map.insert(kk.to_string(), v);
                    } else {
                        return Err(format!("invalid metadata: value: {:?}", v).into());
                    }
                }
                Ok(Self::Metadata(map))
            }
            option::ExecItemRuleOptions::SetTTL(ttl) => Ok(Self::SetTTL(ttl)),
            option::ExecItemRuleOptions::SetRespIP(list) => Ok(Self::SetRespIP(list.into_list())),
            option::ExecItemRuleOptions::Plugin(v) => Ok(Self::Plugin(RwLock::new(v.into()))),
            option::ExecItemRuleOptions::Upstream(v) => {
                Ok(Self::Upstream(RwLock::new((Some(v), None))))
            }
            option::ExecItemRuleOptions::JumpTo(v) => {
                Ok(Self::JumpTo(RwLock::new((Some(v.into_list()), None))))
            }
            option::ExecItemRuleOptions::GoTo(v) => Ok(Self::GoTo(RwLock::new((Some(v), None)))),
            option::ExecItemRuleOptions::CleanResp(_) => Ok(Self::CleanResp),
            option::ExecItemRuleOptions::Return(v) => Ok(Self::Return(v.parse()?)),
        }
    }

    pub(super) async fn check(
        &self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self {
            ExecItemRule::Plugin(v) => {
                let mut p = v.write().await;
                p.prepare(manager).await?;
            }
            ExecItemRule::Upstream(v) => {
                let mut u = v.write().await;
                let (tag, upstream) = u.deref_mut();
                let t = tag.take().unwrap();
                let uu = manager
                    .get_upstream(&t)
                    .await
                    .ok_or(format!("upstream [{}] not found", t))?;
                upstream.replace(uu);
            }
            ExecItemRule::JumpTo(v) => {
                let mut j = v.write().await;
                let (tags, workflows) = j.deref_mut();
                let ts = tags.take().unwrap();
                let mut ww = Vec::with_capacity(ts.len());
                for t in ts {
                    let w = manager
                        .get_workflow(&t)
                        .await
                        .ok_or(format!("workflow [{}] not found", t))?;
                    ww.push(w);
                }
                workflows.replace(ww);
            }
            ExecItemRule::GoTo(v) => {
                let mut g = v.write().await;
                let (tag, workflow) = g.deref_mut();
                let t = tag.take().unwrap();
                let w = manager
                    .get_workflow(&t)
                    .await
                    .ok_or(format!("workflow [{}] not found", t))?;
                workflow.replace(w);
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) async fn execute(
        &self,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        match self {
            ExecItemRule::Mark(mark) => {
                ctx.set_mark(*mark);
            }
            ExecItemRule::Metadata(m) => {
                let mm = ctx.metadata_mut();
                for (k, v) in m {
                    mm.insert(k.clone(), v.clone());
                }
            }
            ExecItemRule::SetTTL(ttl) => {
                if *ttl > 0 {
                    if let Some(response) = ctx.response_mut() {
                        for answer in response.answers_mut() {
                            answer.set_ttl(*ttl);
                        }
                    }
                }
            }
            ExecItemRule::SetRespIP(list) => {
                let qname = ctx.request().query().map(|q| q.name().clone());
                if let Some(qname) = qname {
                    if let Some(response) = ctx.response_mut() {
                        for ip in list {
                            match ip {
                                IpAddr::V4(ip) => {
                                    let mut answer =
                                        Record::with(qname.clone(), RecordType::A, 30 * 60);
                                    answer.set_data(Some(RData::A(ip.clone().into())));
                                    response.add_answer(answer);
                                }
                                IpAddr::V6(ip) => {
                                    let mut answer =
                                        Record::with(qname.clone(), RecordType::AAAA, 30 * 60);
                                    answer.set_data(Some(RData::AAAA(ip.clone().into())));
                                    response.add_answer(answer);
                                }
                            }
                        }
                    }
                }
            }
            ExecItemRule::Plugin(p) => {
                let locker = p.read().await;
                let (plugin, args_id) = locker.get();
                let res = plugin.execute(ctx, args_id).await;
                match res {
                    Ok(r) => return Ok(r),
                    Err(e) => {
                        error!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "executor-plugin [{}] execute failed: {}",
                            plugin.tag(),
                            e
                        );
                        return Err(e);
                    }
                }
            }
            ExecItemRule::Upstream(u) => {
                let locker = u.read().await;
                let (_, upstream) = locker.deref();
                let upstream = upstream.as_ref().unwrap();
                let log_tracker = ctx.log_tracker().clone();
                let request = ctx.request_mut();
                let res = upstream.exchange(Some(&log_tracker), request).await;
                match res {
                    Ok(r) => {
                        ctx.replace_response(r);
                    }
                    Err(e) => {
                        error!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "upstream [{}] exchange failed: {}",
                            upstream.tag(),
                            e
                        );
                        return Err(e);
                    }
                }
            }
            ExecItemRule::JumpTo(j) => {
                let locker = j.read().await;
                let (_, workflows) = locker.deref();
                let workflows = workflows.as_ref().unwrap();
                for w in workflows {
                    let res = w.execute(ctx).await;
                    match res {
                        Ok(adapter::ReturnMode::Continue) => {}
                        Ok(adapter::ReturnMode::ReturnAll) => {
                            return Ok(adapter::ReturnMode::ReturnAll)
                        }
                        Ok(adapter::ReturnMode::ReturnOnce) => {
                            return Ok(adapter::ReturnMode::ReturnOnce)
                        }
                        Err(e) => {
                            error!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "workflow [{}] execute failed: {}",
                                w.tag(),
                                e
                            );
                            return Err(e);
                        }
                    }
                }
            }
            ExecItemRule::GoTo(g) => {
                let locker = g.read().await;
                let (_, workflow) = locker.deref();
                let workflow = workflow.as_ref().unwrap();
                return workflow.execute(ctx).await;
            }
            ExecItemRule::CleanResp => {
                ctx.take_response();
            }
            ExecItemRule::Return(r) => match r {
                Return::All => return Ok(adapter::ReturnMode::ReturnAll),
                Return::Once => return Ok(adapter::ReturnMode::ReturnOnce),
                _ => {
                    ctx.take_response();
                    let request = ctx.request();
                    if request.query().is_some() {
                        let mut response = common::generate_empty_message(request);
                        match r {
                            Return::Success => {
                                response.set_response_code(ResponseCode::NoError);
                            }
                            Return::Failure => {
                                response.set_response_code(ResponseCode::ServFail);
                            }
                            Return::NxDomain => {
                                response.set_response_code(ResponseCode::NXDomain);
                            }
                            Return::Refused => {
                                response.set_response_code(ResponseCode::Refused);
                            }
                            _ => unreachable!(),
                        }
                        ctx.replace_response(response);
                    } else {
                        debug!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "query not found, skip"
                        );
                    }
                    return Ok(adapter::ReturnMode::ReturnAll);
                }
            },
        }
        Ok(adapter::ReturnMode::Continue)
    }
}

pub(super) enum Return {
    All,
    Once,
    Success,
    Failure,
    NxDomain,
    Refused,
}

impl FromStr for Return {
    type Err = Box<dyn Error + Send + Sync>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "all" => Ok(Self::All),
            "once" => Ok(Self::Once),
            "success" => Ok(Self::Success),
            "failure" => Ok(Self::Failure),
            "nxdomain" => Ok(Self::NxDomain),
            "refused" => Ok(Self::Refused),
            _ => Err(format!("invalid return: {:?}", s).into()),
        }
    }
}

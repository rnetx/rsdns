use std::{collections::HashMap, error::Error, str::FromStr, sync::Arc};

use hickory_proto::rr::{Name, RecordType};
use tokio::sync::RwLock;

use crate::{adapter, common, debug, error, log, option};

pub(super) struct MatchItemRule {
    invert: bool,
    inner: MatchItemInnerRule,
}

impl MatchItemRule {
    pub(super) fn new(
        options: option::MatchItemRuleOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let (inner, invert) = match options {
            option::MatchItemRuleOptions::Listener(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Listener(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertListener(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Listener(v))?,
                true,
            ),
            option::MatchItemRuleOptions::ClientIP(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::ClientIP(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertClientIP(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::ClientIP(v))?,
                true,
            ),
            option::MatchItemRuleOptions::QType(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::QType(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertQType(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::QType(v))?,
                true,
            ),
            option::MatchItemRuleOptions::QName(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::QName(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertQName(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::QName(v))?,
                true,
            ),
            option::MatchItemRuleOptions::HasRespMsg(v) => {
                (MatchItemInnerRule::HasRespMsg(v), false)
            }
            option::MatchItemRuleOptions::InvertHasRespMsg(v) => {
                (MatchItemInnerRule::HasRespMsg(v), true)
            }
            option::MatchItemRuleOptions::RespIP(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::RespIP(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertRespIP(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::RespIP(v))?,
                true,
            ),
            option::MatchItemRuleOptions::Mark(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Mark(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertMark(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Mark(v))?,
                true,
            ),
            option::MatchItemRuleOptions::Metadata(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Metadata(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertMetadata(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Metadata(v))?,
                true,
            ),
            option::MatchItemRuleOptions::Env(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Env(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertEnv(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Env(v))?,
                true,
            ),
            option::MatchItemRuleOptions::Plugin(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Plugin(v))?,
                false,
            ),
            option::MatchItemRuleOptions::InvertPlugin(v) => (
                MatchItemInnerRule::new(option::MatchItemRuleOptions::Plugin(v))?,
                true,
            ),
        };
        Ok(Self { invert, inner })
    }

    pub(super) async fn check(
        &self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.inner.check(manager).await
    }

    pub(super) async fn r#match(
        &self,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let res = self.inner.r#match(logger, ctx).await?;
        if self.invert {
            debug!(
                logger,
                { tracker = ctx.log_tracker() },
                "invert match: {} => {}",
                res,
                !res
            );
            Ok(!res)
        } else {
            Ok(res)
        }
    }
}

enum MatchItemInnerRule {
    Listener(HashMap<String, ()>),
    ClientIP(Vec<common::IPRange>),
    QType(HashMap<RecordType, ()>),
    QName(HashMap<Name, ()>),
    HasRespMsg(bool),
    RespIP(Vec<common::IPRange>),
    Mark(HashMap<i16, ()>),
    Metadata(HashMap<String, String>),
    Env(HashMap<String, String>),
    Plugin(RwLock<super::MatchPlugin>), // Plugin, Args
}

impl MatchItemInnerRule {
    fn new(options: option::MatchItemRuleOptions) -> Result<Self, Box<dyn Error + Send + Sync>> {
        match options {
            option::MatchItemRuleOptions::Listener(list) => {
                let list = list.into_list();
                let mut map = HashMap::with_capacity(list.len());
                for item in list {
                    let item_str = item.trim();
                    if !item_str.is_empty() {
                        map.insert(item_str.to_string(), ());
                    }
                }
                Ok(Self::Listener(map))
            }
            option::MatchItemRuleOptions::ClientIP(list) => {
                let list = list.into_list();
                let mut l = Vec::with_capacity(list.len());
                for item in list {
                    let item_str = item.trim();
                    if !item_str.is_empty() {
                        let range = common::IPRange::from_str(item_str)
                            .map_err(|_| format!("invalid client-ip: {}", item_str))?;
                        l.push(range);
                    }
                }
                Ok(Self::ClientIP(l))
            }
            option::MatchItemRuleOptions::QType(list) => {
                let list = list.into_list();
                let mut map = HashMap::with_capacity(list.len());
                for item in list {
                    match item {
                        serde_yaml::Value::Number(n) => {
                            if let Some(n) = n.as_u64() {
                                if n > u16::MAX as u64 {
                                    return Err(format!("invalid qtype: {}", n).into());
                                }
                                map.insert(RecordType::from(n as u16), ());
                            }
                            return Err(format!("invalid qtype: {}", n).into());
                        }
                        serde_yaml::Value::String(s) => {
                            let item_str = s.trim();
                            if !item_str.is_empty() {
                                let r =
                                    RecordType::from_str(item_str.to_ascii_uppercase().as_str())
                                        .map_err(|_| format!("invalid qtype: {}", item_str))?;
                                map.insert(r, ());
                            }
                        }
                        _ => return Err(format!("invalid qtype: {:?}", item).into()),
                    }
                }
                Ok(Self::QType(map))
            }
            option::MatchItemRuleOptions::QName(list) => {
                let list = list.into_list();
                let mut map = HashMap::with_capacity(list.len());
                for item in list {
                    let item_str = item.trim();
                    if !item_str.is_empty() {
                        let name = Name::from_str(item_str)
                            .map_err(|_| format!("invalid qname: {}", item_str))?;
                        map.insert(name, ());
                    }
                }
                Ok(Self::QName(map))
            }
            option::MatchItemRuleOptions::HasRespMsg(b) => Ok(Self::HasRespMsg(b)),
            option::MatchItemRuleOptions::RespIP(list) => {
                let list = list.into_list();
                let mut l = Vec::with_capacity(list.len());
                for item in list {
                    let item_str = item.trim();
                    if !item_str.is_empty() {
                        let range = common::IPRange::from_str(item_str)
                            .map_err(|_| format!("invalid resp-ip: {}", item_str))?;
                        l.push(range);
                    }
                }
                Ok(Self::RespIP(l))
            }
            option::MatchItemRuleOptions::Mark(list) => {
                let list = list.into_list();
                let mut map = HashMap::with_capacity(list.len());
                for item in list {
                    map.insert(item, ());
                }
                Ok(Self::Mark(map))
            }
            option::MatchItemRuleOptions::Metadata(m) => {
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
            option::MatchItemRuleOptions::Env(m) => {
                let mut map = HashMap::with_capacity(m.len());
                for (k, v) in m {
                    let k = if let serde_yaml::Value::String(k) = k {
                        k
                    } else {
                        return Err(format!("invalid env: key: {:?}", k).into());
                    };
                    let kk = k.trim();
                    if kk.is_empty() {
                        return Err("invalid env: empty key".into());
                    }
                    if let serde_yaml::Value::String(v) = v {
                        map.insert(kk.to_string(), v);
                    } else {
                        return Err(format!("invalid env: value: {:?}", v).into());
                    }
                }
                Ok(Self::Env(map))
            }
            option::MatchItemRuleOptions::Plugin(v) => Ok(Self::Plugin(RwLock::new(v.into()))),
            _ => unreachable!(),
        }
    }

    async fn check(
        &self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self {
            MatchItemInnerRule::Plugin(v) => {
                let mut p = v.write().await;
                p.prepare(manager).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn r#match(
        &self,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        match self {
            MatchItemInnerRule::Listener(map) => {
                let res = map.contains_key(ctx.listener());
                if res {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "listener matched: {}",
                        ctx.listener()
                    );
                } else {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "listener not matched: {}",
                        ctx.listener()
                    );
                }
                Ok(res)
            }
            MatchItemInnerRule::ClientIP(list) => {
                let res = list.iter().any(|range| range.contains(ctx.client_ip()));
                if res {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "client-ip matched: {}",
                        ctx.client_ip()
                    );
                } else {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "client-ip not matched: {}",
                        ctx.client_ip()
                    );
                }
                Ok(res)
            }
            MatchItemInnerRule::QType(map) => {
                let query = ctx.request().query().ok_or("no query found")?;
                let qtype = query.query_type();
                let res = map.contains_key(&qtype);
                if res {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "qtype matched: {}",
                        qtype
                    );
                } else {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "qtype not matched: {}",
                        qtype
                    );
                }
                Ok(res)
            }
            MatchItemInnerRule::QName(map) => {
                let query = ctx.request().query().ok_or("no query found")?;
                let qname = query.name();
                let res = map.contains_key(&qname);
                if res {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "qname matched: {}",
                        qname
                    );
                } else {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "qname not matched: {}",
                        qname
                    );
                }
                Ok(res)
            }
            MatchItemInnerRule::HasRespMsg(b) => {
                let res = if *b {
                    ctx.response().is_some()
                } else {
                    ctx.response().is_none()
                };
                if res {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "has-resp-msg matched: {}",
                        b
                    );
                } else {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "has-resp-msg not matched: {}",
                        b
                    );
                }
                Ok(res)
            }
            MatchItemInnerRule::RespIP(list) => {
                if let Some(response) = ctx.response() {
                    for answer in response.answers() {
                        if let Some(data) = answer.data() {
                            if let Some(a) = data.as_a() {
                                let res = list.iter().any(|range| range.contains_v4(&a.0));
                                if res {
                                    debug!(
                                        logger,
                                        { tracker = ctx.log_tracker() },
                                        "resp-ip matched: {}",
                                        a
                                    );
                                }
                                return Ok(res);
                            }
                            if let Some(aaaa) = data.as_aaaa() {
                                let res = list.iter().any(|range| range.contains_v6(&aaaa.0));
                                if res {
                                    debug!(
                                        logger,
                                        { tracker = ctx.log_tracker() },
                                        "resp-ip matched: {}",
                                        aaaa
                                    );
                                }
                                return Ok(res);
                            }
                        }
                    }
                }
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "resp-ip not matched: no response or no A/AAAA record or no ip matched"
                );
                Ok(false)
            }
            MatchItemInnerRule::Mark(map) => {
                let res = map.contains_key(&ctx.mark());
                if res {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "mark matched: {}",
                        ctx.mark()
                    );
                } else {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "mark not matched: {}",
                        ctx.mark()
                    );
                }
                Ok(res)
            }
            MatchItemInnerRule::Metadata(map) => {
                for (k, v) in map {
                    if let Some(mv) = ctx.metadata().get(k) {
                        if mv != v {
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "metadata not matched: key-value: {} => {} (rule: {})",
                                k,
                                mv,
                                v
                            );
                            return Ok(false);
                        }
                    } else {
                        debug!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "metadata not matched: key not found: {}",
                            k
                        );
                        return Ok(false);
                    }
                }
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "metadata matched: {:?}",
                    ctx.metadata()
                );
                Ok(true)
            }
            MatchItemInnerRule::Env(map) => {
                for (k, v) in map {
                    if let Ok(ev) = std::env::var(k) {
                        if &ev != v {
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "env not matched: key-value: {} => {} (rule: {})",
                                k,
                                ev,
                                v
                            );
                            return Ok(false);
                        }
                    } else {
                        debug!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "env not matched: key not found: {}",
                            k
                        );
                        return Ok(false);
                    }
                }
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "env matched: {:?}",
                    map
                );
                Ok(true)
            }
            MatchItemInnerRule::Plugin(p) => {
                let locker = p.read().await;
                let (plugin, args_id) = locker.get();
                let res = plugin.r#match(ctx, args_id).await;
                match res {
                    Ok(b) => {
                        if b {
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "matcher-plugin [{}] matched",
                                plugin.tag()
                            );
                        } else {
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "matcher-plugin [{}] not matched",
                                plugin.tag()
                            );
                        }
                        Ok(b)
                    }
                    Err(e) => {
                        error!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "matcher-plugin [{}] match failed: {}",
                            plugin.tag(),
                            e
                        );
                        Err(e)
                    }
                }
            }
        }
    }
}

use std::{error::Error, net::IpAddr, sync::Arc, time::Duration};

use chrono::{DateTime, Local};
use hickory_proto::{
    op::{Message, Query},
    rr::{DNSClass, Name, RData, RecordType},
};
use rand::Rng;
use tokio::{sync::RwLock, task::JoinSet};

use crate::{adapter, option};

pub(super) struct Bootstrap {
    manager: Arc<Box<dyn adapter::Manager>>,
    strategy: option::BootstrapStrategy,
    upstream_tag: String,
    upstream: RwLock<Option<Arc<Box<dyn adapter::Upstream>>>>,
    cache: RwLock<Option<(Vec<IpAddr>, DateTime<Local>)>>,
}

impl Bootstrap {
    pub(super) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        options: option::BootstrapOptions,
    ) -> Self {
        Self {
            manager,
            strategy: options.strategy,
            upstream_tag: options.upstream,
            upstream: RwLock::new(None),
            cache: RwLock::new(None),
        }
    }

    pub(super) async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = self
            .manager
            .get_upstream(&self.upstream_tag)
            .await
            .ok_or(format!("upstream [{}] not found", &self.upstream_tag))?;
        self.upstream.write().await.replace(upstream);
        Ok(())
    }

    pub(super) fn upstream_tag(&self) -> &str {
        &self.upstream_tag
    }

    fn create_message(
        domain: &str,
        is_aaaa: bool,
    ) -> Result<Message, Box<dyn Error + Send + Sync>> {
        let name: Name = domain.parse()?;
        let mut query = Query::new();
        query.set_name(name);
        query.set_query_class(DNSClass::IN);
        query.set_query_type(if is_aaaa {
            RecordType::AAAA
        } else {
            RecordType::A
        });
        let mut message = Message::new();
        message.set_id(rand::thread_rng().gen());
        message.set_recursion_desired(true);
        message.add_query(query);
        Ok(message)
    }

    async fn exchange_wrapper(
        upstream: Arc<Box<dyn adapter::Upstream>>,
        mut request: Message,
    ) -> Result<(Vec<IpAddr>, Duration), Box<dyn Error + Send + Sync>> {
        let mut response = upstream.exchange(None, &mut request).await?;
        let mut ips = Vec::with_capacity(response.answer_count() as usize);
        let mut min_ttl = 0u32;
        for answer in response.take_answers() {
            let ttl = answer.ttl();
            if let Some(data) = answer.into_data() {
                match data {
                    RData::A(a) => {
                        if min_ttl == 0 || min_ttl > ttl {
                            min_ttl = ttl;
                        }
                        ips.push(IpAddr::V4(a.0));
                    }
                    RData::AAAA(aaaa) => {
                        if min_ttl == 0 || min_ttl > ttl {
                            min_ttl = ttl;
                        }
                        ips.push(IpAddr::V6(aaaa.0));
                    }
                    _ => {}
                }
            }
        }
        if ips.len() == 0 {
            return Err("no ip found".into());
        }
        Ok((ips, Duration::from_secs(min_ttl as u64)))
    }

    async fn lookup_wrapper(
        &self,
        domain: &str,
    ) -> Result<(Vec<IpAddr>, Duration), Box<dyn Error + Send + Sync>> {
        let upstream = self.upstream.read().await.as_ref().unwrap().clone();
        match &self.strategy {
            option::BootstrapStrategy::OnlyIPv4 => {
                return Self::exchange_wrapper(upstream, Self::create_message(domain, false)?).await
            }
            option::BootstrapStrategy::OnlyIPv6 => {
                return Self::exchange_wrapper(upstream, Self::create_message(domain, true)?).await
            }
            _ => {}
        }
        let request_message_a = Self::create_message(domain, false)?;
        let request_message_aaaa = Self::create_message(domain, true)?;
        let upstream_a = upstream;
        let upstream_aaaa = upstream_a.clone();
        let mut join_set = JoinSet::new();
        join_set.spawn(async move { Self::exchange_wrapper(upstream_a, request_message_a).await });
        join_set.spawn(
            async move { Self::exchange_wrapper(upstream_aaaa, request_message_aaaa).await },
        );
        let mut err = None;
        let mut result: Option<(Vec<IpAddr>, Duration)> = None;
        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Ok((mut ips, ttl))) => {
                    if ips[0].is_ipv4() {
                        match result.take() {
                            Some((mut ips2, ttl2)) => {
                                if let option::BootstrapStrategy::PreferIPv4 = &self.strategy {
                                    ips.extend_from_slice(&ips2);
                                    result = Some((ips, ttl.min(ttl2)));
                                } else {
                                    ips2.extend_from_slice(&ips);
                                    result = Some((ips2, ttl2.min(ttl)));
                                }
                            }
                            None => {
                                result = Some((ips, ttl));
                            }
                        }
                    } else {
                        match result.take() {
                            Some((mut ips2, ttl2)) => {
                                if let option::BootstrapStrategy::PreferIPv6 = &self.strategy {
                                    ips.extend_from_slice(&ips2);
                                    result = Some((ips, ttl.min(ttl2)));
                                } else {
                                    ips2.extend_from_slice(&ips);
                                    result = Some((ips2, ttl2.min(ttl)));
                                }
                            }
                            None => {
                                result = Some((ips, ttl));
                            }
                        }
                    }
                }
                Ok(Err(e)) => match &err {
                    Some(e2) => err = Some(format!("{} | {}", e2, e).into()),
                    None => err = Some(e),
                },
                Err(e) => match &err {
                    Some(e2) => err = Some(format!("{} | {}", e2, e).into()),
                    None => err = Some(format!("{}", e).into()),
                },
            }
        }
        join_set.abort_all();
        if let Some((ips, ttl)) = result {
            return Ok((ips, ttl));
        }
        Err(err.unwrap())
    }

    pub(super) async fn lookup(
        &self,
        domain: &str,
    ) -> Result<Vec<IpAddr>, Box<dyn Error + Send + Sync>> {
        let now = Local::now();
        {
            let cache = self.cache.read().await;
            if let Some((ips, expire)) = cache.as_ref() {
                if now < *expire {
                    return Ok(ips.clone());
                }
            }
        }
        let (ips, ttl) = self.lookup_wrapper(domain).await?;
        self.cache.write().await.replace((ips.clone(), now + ttl));
        Ok(ips)
    }
}

use std::{io, sync::Arc, time::Duration};

use hickory_proto::{
    op::{Message, Query},
    rr::{DNSClass, Name, RecordType},
};
use rand::Rng;
use tokio::task::JoinSet;

use crate::{adapter, log, option, upstream};

struct NopManager {
    state_map: Arc<state::TypeMap![Send + Sync]>,
}

impl Default for NopManager {
    fn default() -> Self {
        Self {
            state_map: Arc::new(<state::TypeMap![Send + Sync]>::new()),
        }
    }
}

#[async_trait::async_trait]
impl adapter::Manager for NopManager {
    async fn get_upstream(&self, _: &str) -> Option<Arc<Box<dyn adapter::Upstream>>> {
        None
    }

    async fn list_upstream(&self) -> Vec<Arc<Box<dyn adapter::Upstream>>> {
        vec![]
    }

    async fn fail_to_close(&self, _: String) {}

    async fn get_workflow(&self, _: &str) -> Option<Arc<Box<dyn adapter::Workflow>>> {
        None
    }

    async fn list_workflow(&self) -> Vec<Arc<Box<dyn adapter::Workflow>>> {
        vec![]
    }

    async fn get_matcher_plugin(&self, _: &str) -> Option<Arc<Box<dyn adapter::MatcherPlugin>>> {
        None
    }

    async fn list_matcher_plugin(&self) -> Vec<Arc<Box<dyn adapter::MatcherPlugin>>> {
        vec![]
    }

    async fn get_executor_plugin(&self, _: &str) -> Option<Arc<Box<dyn adapter::ExecutorPlugin>>> {
        None
    }

    async fn list_executor_plugin(&self) -> Vec<Arc<Box<dyn adapter::ExecutorPlugin>>> {
        vec![]
    }

    fn get_state_map(&self) -> &state::TypeMap![Send + Sync] {
        &self.state_map
    }

    fn api_enabled(&self) -> bool {
        false
    }
}

fn message(s: &str) -> Message {
    let name: Name = s.parse().unwrap();
    let mut query = Query::new();
    query.set_name(name);
    query.set_query_class(DNSClass::IN);
    query.set_query_type(RecordType::A);
    let mut message = Message::new();
    message.set_id(rand::thread_rng().gen());
    message.set_recursion_desired(true);
    message.add_query(query);
    message
}

async fn test_message(upstream: &Arc<Box<dyn adapter::Upstream>>) {
    let domains = vec![
        "www.baidu.com",
        "www.zhihu.com",
        "www.bilibili.com",
        "www.douyin.com",
        "www.qq.com",
        "www.taobao.com",
        "www.jd.com",
        "www.163.com",
        "www.baidu.com",
        "www.zhihu.com",
        "www.bilibili.com",
        "www.douyin.com",
        "www.qq.com",
        "www.taobao.com",
        "www.jd.com",
        "www.163.com",
        "www.baidu.com",
        "www.zhihu.com",
        "www.bilibili.com",
        "www.douyin.com",
        "www.qq.com",
        "www.taobao.com",
        "www.jd.com",
        "www.163.com",
    ];
    let mut join_set = JoinSet::new();
    for domain in domains.iter() {
        let mut message = message(domain);
        let upstream = upstream.clone();
        tokio::time::sleep(Duration::from_millis(100)).await;
        join_set.spawn(async move {
            let result = upstream.exchange(None, &mut message).await;
            println!("{}: {}", message.query().unwrap().name(), result.is_ok());
        });
    }
    let mut i = 0;
    println!("join_next: {}/{}", i, domains.len());
    while let Some(_) = join_set.join_next().await {
        i = i + 1;
        println!("join_next: {}/{}", i, domains.len());
    }
}

async fn test_upstream(options: option::UpstreamOptions) {
    let manager = Box::new(NopManager::default());
    let basic_logger =
        log::BasicLogger::new(false, log::Level::Debug, Box::new(io::stdout())).into_box();
    let tag = options.tag.clone();
    let u = upstream::new_upstream(Arc::new(manager), basic_logger, tag, options)
        .map(|u| Arc::new(u))
        .unwrap();
    u.start().await.unwrap();
    test_message(&u).await;
    u.close().await.unwrap();
}

#[tokio::test]
async fn test_udp_upstream() {
    test_upstream(option::UpstreamOptions {
        tag: "udp".to_string(),
        query_timeout: None,
        inner: option::UpstreamInnerOptions::UDPUpstream(option::UDPUpstreamOptions {
            address: "223.5.5.5".to_string(),
            idle_timeout: None,
            fallback_tcp: false,
            enable_pipeline: false,
            bootstrap: None,
            dialer: None,
        }),
    })
    .await;
}

#[tokio::test]
async fn test_tcp_upstream() {
    test_upstream(option::UpstreamOptions {
        tag: "tcp".to_string(),
        query_timeout: None,
        inner: option::UpstreamInnerOptions::TCPUpstream(option::TCPUpstreamOptions {
            address: "223.5.5.5".to_string(),
            idle_timeout: None,
            enable_pipeline: true,
            bootstrap: None,
            dialer: None,
        }),
    })
    .await;
}

#[cfg(feature = "upstream-tls")]
#[tokio::test]
async fn test_tls_upstream() {
    test_upstream(option::UpstreamOptions {
        tag: "tls".to_string(),
        query_timeout: None,
        inner: option::UpstreamInnerOptions::TLSUpstream(option::TLSUpstreamOptions {
            address: "223.5.5.5".to_string(),
            idle_timeout: None,
            enable_pipeline: true,
            tls: Default::default(),
            bootstrap: None,
            dialer: None,
        }),
    })
    .await;
}

#[cfg(feature = "upstream-https")]
#[tokio::test]
async fn test_https_upstream() {
    test_upstream(option::UpstreamOptions {
        tag: "https".to_string(),
        query_timeout: None,
        inner: option::UpstreamInnerOptions::HTTPSUpstream(option::HTTPSUpstreamOptions {
            address: "223.5.5.5".to_string(),
            idle_timeout: None,
            path: None,
            header: None,
            host: None,
            use_http3: false,
            use_post: false,
            tls: Default::default(),
            bootstrap: None,
            dialer: None,
        }),
    })
    .await;
}

#[cfg(feature = "upstream-quic")]
#[tokio::test]
async fn test_quic_upstream() {
    test_upstream(option::UpstreamOptions {
        tag: "https".to_string(),
        query_timeout: None,
        inner: option::UpstreamInnerOptions::QUICUpstream(option::QUICUpstreamOptions {
            address: "223.5.5.5".to_string(),
            idle_timeout: None,
            disable_add_prefix: false,
            tls: Default::default(),
            bootstrap: None,
            dialer: None,
        }),
    })
    .await;
}

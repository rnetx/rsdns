use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use futures_util::{Future, StreamExt};
use hickory_proto::{
    op::{Message, Query},
    rr::{DNSClass, Name, RecordType},
    xfer::{DnsRequest, DnsRequestOptions, DnsRequestSender, DnsStreamHandle},
};
use rand::Rng as _;
use tokio::{
    sync::{oneshot, Mutex},
    task::JoinSet,
};

use crate::{common, log, manager, option};

async fn build(mut listener_options: option::ListenerInnerOptions) -> manager::Manager {
    const UPSTREAM_TAG: &str = "test-upstream";
    const WORKFLOW_TAG: &str = "test-workflow";
    const LISTENER_TAG: &str = "test-listener";

    match &mut listener_options {
        option::ListenerInnerOptions::UDPListener(udp_listener_options) => {
            udp_listener_options.generic.workflow = WORKFLOW_TAG.to_string();
        }
        option::ListenerInnerOptions::TCPListener(tcp_listener_options) => {
            tcp_listener_options.generic.workflow = WORKFLOW_TAG.to_string();
        }
        option::ListenerInnerOptions::BaseListener(base_listener_options) => {
            base_listener_options.generic.workflow = WORKFLOW_TAG.to_string();
        }

        #[cfg(feature = "listener-tls")]
        option::ListenerInnerOptions::TLSListener(tls_listener_options) => {
            tls_listener_options.generic.workflow = WORKFLOW_TAG.to_string();
        }

        #[cfg(feature = "listener-http")]
        option::ListenerInnerOptions::HTTPListener(http_listener_options) => {
            http_listener_options.generic.workflow = WORKFLOW_TAG.to_string();
        }

        #[cfg(all(feature = "listener-quic", feature = "listener-tls"))]
        option::ListenerInnerOptions::QUICListener(quic_listener_options) => {
            quic_listener_options.generic.workflow = WORKFLOW_TAG.to_string();
        }
    }

    let options = option::Options {
        log: option::LogOptions {
            level: log::Level::Debug,
            output: "stdout".to_owned(),
            ..Default::default()
        },
        api: Some(option::APIServerOptions {
            listen: "0.0.0.0:9088".parse().unwrap(),
            secret: None,
        }),
        upstreams: vec![option::UpstreamOptions {
            tag: UPSTREAM_TAG.to_string(),
            inner: option::UpstreamInnerOptions::TCPUpstream(option::TCPUpstreamOptions {
                address: "223.5.5.5".to_owned(),
                idle_timeout: None,
                enable_pipeline: true,
                bootstrap: None,
                dialer: None,
            }),
            query_timeout: Some(Duration::from_secs(5)),
        }],
        workflows: vec![option::WorkflowOptions {
            tag: WORKFLOW_TAG.to_string(),
            rules: vec![option::WorkflowRuleOptions::Exec(option::ExecRuleOptions {
                exec: vec![option::ExecItemRuleOptions::Upstream(
                    UPSTREAM_TAG.to_string(),
                )],
            })],
        }],
        listeners: vec![option::ListenerOptions {
            tag: LISTENER_TAG.to_string(),
            inner: listener_options,
        }],
        ..Default::default()
    };

    manager::Manager::prepare(options).await.unwrap()
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

async fn exchange<P, F, Fut>(handle_params: P, handle: F)
where
    P: Clone,
    F: Fn(Message, P) -> Fut,
    Fut: Future<Output = Result<Message, String>> + Send + 'static,
{
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
        tokio::time::sleep(Duration::from_millis(100)).await;
        let fut = handle(message(domain), handle_params.clone());
        let domain_str = domain.to_string();
        join_set.spawn(async move {
            let result = fut.await;
            println!("{}: {}", domain_str, result.is_ok());
        });
        while futures_util::FutureExt::now_or_never(join_set.join_next())
            .flatten()
            .is_some()
        {}
    }
    let mut i = 0;
    println!("join_next: {}/{}", i, domains.len());
    while let Some(_) = join_set.join_next().await {
        i = i + 1;
        println!("join_next: {}/{}", i, domains.len());
    }
}

#[tokio::test]
async fn test_udp_listener() {
    let mgr = build(option::ListenerInnerOptions::UDPListener(
        option::UDPListenerOptions {
            listen: "127.0.0.1:5363".to_string(),
            generic: option::GenericListenerOptions {
                query_timeout: None,
                workflow: Default::default(),
            },
        },
    ))
    .await;

    let (mut canceller, canceller_guard) = common::new_canceller();
    tokio::spawn(async move {
        mgr.run(canceller_guard.clone_token()).await.unwrap();
    });

    exchange((), |message, _| async move {
        let mut stream = hickory_proto::udp::UdpClientStream::<tokio::net::UdpSocket, _>::new(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363),
        )
        .await
        .map_err(|e| e.to_string())?;

        let mut response_stream =
            stream.send_message(DnsRequest::new(message, DnsRequestOptions::default()));
        let response_message = response_stream
            .next()
            .await
            .ok_or_else(|| format!("{}", "response not found"))?
            .map_err(|e| e.to_string())?;

        Ok(response_message.into_message())
    })
    .await;

    canceller.cancel_and_wait().await;
}

#[tokio::test]
async fn test_tcp_listener() {
    let mgr = build(option::ListenerInnerOptions::TCPListener(
        option::TCPListenerOptions {
            listen: "127.0.0.1:5363".to_string(),
            generic: option::GenericListenerOptions {
                query_timeout: None,
                workflow: Default::default(),
            },
        },
    ))
    .await;
    let started_notify_token = mgr.started_notify_token();

    let (mut canceller, canceller_guard) = common::new_canceller();
    let canceller_guard_mgr = canceller_guard.clone();
    tokio::spawn(async move {
        mgr.run(canceller_guard_mgr.clone_token()).await.unwrap();
    });

    started_notify_token.cancelled_owned().await;

    let (response_stream_fut, send_stream) =
        hickory_proto::tcp::TcpClientStream::<
            hickory_proto::iocompat::AsyncIoTokioAsStd<tokio::net::TcpStream>,
        >::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363));
    let mut response_stream = response_stream_fut.await.unwrap();

    let channel_map = Arc::new(Mutex::new(
        HashMap::<u16, oneshot::Sender<Message>, _>::new(),
    ));
    let channel_map_receive = channel_map.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
              res = response_stream.next() => {
                if let Some(res) = res {
                  match res {
                    Ok(msg) => {
                      match msg.to_message() {
                        Ok(message) => {
                          if let Some(sender) = channel_map_receive.lock().await.remove(&message.id()) {
                            sender.send(message).ok();
                          }
                        },
                        Err(e) => {
                          println!("error: {}", e);
                        }
                      }
                    }
                    Err(e) => {
                      println!("error: {}", e);
                    }
                  }
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
    });

    exchange(
        (send_stream, channel_map),
        |message, (mut send_stream, channel_map)| async move {
            let msg = message.to_vec().map_err(|e| e.to_string())?;
            let (sender, receiver) = oneshot::channel();
            channel_map.lock().await.insert(message.id(), sender);
            send_stream
                .send((msg, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363)).into())
                .map_err(|e| e.to_string())?;
            receiver.await.map_err(|e| e.to_string())
        },
    )
    .await;

    canceller.cancel_and_wait().await;
}

#[cfg(feature = "listener-tls")]
#[tokio::test]
async fn test_tls_listener() {
    use crate::upstream;

    let mgr = build(option::ListenerInnerOptions::TLSListener(
        option::TLSListenerOptions {
            listen: "127.0.0.1:5363".to_string(),
            tls: option::ListenerTLSOptions {
                client_ca_file: Default::default(),
                server_cert_file: "/rsdns/src/tests/test-server-cert.pem".to_string(),
                server_key_file: "/rsdns/src/tests/test-server-key.pem".to_string(),
            },
            generic: option::GenericListenerOptions {
                query_timeout: None,
                workflow: Default::default(),
            },
        },
    ))
    .await;
    let started_notify_token = mgr.started_notify_token();

    let (mut canceller, canceller_guard) = common::new_canceller();
    let canceller_guard_mgr = canceller_guard.clone();
    tokio::spawn(async move {
        mgr.run(canceller_guard_mgr.clone_token()).await.unwrap();
    });

    started_notify_token.cancelled_owned().await;

    let tls_config = {
        let mut tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();

        {
            let mut dangerous_config = rustls::client::DangerousClientConfig {
                cfg: &mut tls_config,
            };
            dangerous_config.set_certificate_verifier(Arc::new(upstream::DangerousServerVerifier));
        }

        tls_config
    };

    let (response_stream_fut, send_stream) = hickory_proto::tcp::TcpClientStream::<
        hickory_proto::iocompat::AsyncIoTokioAsStd<
            tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
        >,
    >::with_future(
        async move {
            let tcp_stream = tokio::net::TcpStream::connect(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                5363,
            ))
            .await?;
            tokio_rustls::TlsConnector::from(Arc::new(tls_config))
                .connect(
                    rustls::ServerName::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    tcp_stream,
                )
                .await
                .map(|stream| hickory_proto::iocompat::AsyncIoTokioAsStd(stream))
        },
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363),
        Duration::from_secs(5),
    );
    let mut response_stream = response_stream_fut.await.unwrap();

    let channel_map = Arc::new(Mutex::new(
        HashMap::<u16, oneshot::Sender<Message>, _>::new(),
    ));
    let channel_map_receive = channel_map.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
              res = response_stream.next() => {
                if let Some(res) = res {
                  match res {
                    Ok(msg) => {
                      match msg.to_message() {
                        Ok(message) => {
                          if let Some(sender) = channel_map_receive.lock().await.remove(&message.id()) {
                            sender.send(message).ok();
                          }
                        },
                        Err(e) => {
                          println!("error: {}", e);
                        }
                      }
                    }
                    Err(e) => {
                      println!("error: {}", e);
                    }
                  }
                }
              }
              _ = canceller_guard.cancelled() => {
                break;
              }
            }
        }
    });

    exchange(
        (send_stream, channel_map),
        |message, (mut send_stream, channel_map)| async move {
            let msg = message.to_vec().map_err(|e| e.to_string())?;
            let (sender, receiver) = oneshot::channel();
            channel_map.lock().await.insert(message.id(), sender);
            send_stream
                .send((msg, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363)).into())
                .map_err(|e| e.to_string())?;
            receiver.await.map_err(|e| e.to_string())
        },
    )
    .await;

    canceller.cancel_and_wait().await;
}

#[cfg(feature = "listener-http")]
#[tokio::test]
async fn test_https_listener() {
    use crate::upstream;

    let mgr = build(option::ListenerInnerOptions::HTTPListener(
        option::HTTPListenerOptions {
            listen: "127.0.0.1:5363".to_string(),
            path: Some("/dns-query".to_string()),
            use_http3: false,
            tls: Some(option::ListenerTLSOptions {
                client_ca_file: Default::default(),
                server_cert_file: "/rsdns/src/tests/test-server-cert.pem".to_string(),
                server_key_file: "/rsdns/src/tests/test-server-key.pem".to_string(),
            }),
            generic: option::GenericListenerOptions {
                query_timeout: None,
                workflow: Default::default(),
            },
        },
    ))
    .await;
    let started_notify_token = mgr.started_notify_token();

    let (mut canceller, canceller_guard) = common::new_canceller();
    tokio::spawn(async move {
        mgr.run(canceller_guard.clone_token()).await.unwrap();
    });

    started_notify_token.cancelled_owned().await;

    let tls_config = {
        let mut tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();

        {
            let mut dangerous_config = rustls::client::DangerousClientConfig {
                cfg: &mut tls_config,
            };
            dangerous_config.set_certificate_verifier(Arc::new(upstream::DangerousServerVerifier));
        }

        tls_config
    };

    let stream =
        hickory_proto::h2::HttpsClientStreamBuilder::with_client_config(Arc::new(tls_config))
            .build::<hickory_proto::iocompat::AsyncIoTokioAsStd<tokio::net::TcpStream>>(
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363),
                "127.0.0.1".to_string(),
            )
            .await
            .unwrap();

    exchange(stream, |message, mut stream| async move {
        let mut response_stream =
            stream.send_message(DnsRequest::new(message, DnsRequestOptions::default()));
        let response_message = response_stream
            .next()
            .await
            .ok_or_else(|| format!("{}", "response not found"))?
            .map_err(|e| e.to_string())?;

        Ok(response_message.into_message())
    })
    .await;

    canceller.cancel_and_wait().await;
}

#[cfg(all(feature = "listener-http", feature = "listener-quic"))]
#[tokio::test]
async fn test_h3_listener() {
    use crate::upstream;

    let mgr = build(option::ListenerInnerOptions::HTTPListener(
        option::HTTPListenerOptions {
            listen: "127.0.0.1:5363".to_string(),
            path: Some("/dns-query".to_string()),
            use_http3: true,
            tls: Some(option::ListenerTLSOptions {
                client_ca_file: Default::default(),
                server_cert_file: "/rsdns/src/tests/test-server-cert.pem".to_string(),
                server_key_file: "/rsdns/src/tests/test-server-key.pem".to_string(),
            }),
            generic: option::GenericListenerOptions {
                query_timeout: None,
                workflow: Default::default(),
            },
        },
    ))
    .await;
    let started_notify_token = mgr.started_notify_token();

    let (mut canceller, canceller_guard) = common::new_canceller();
    tokio::spawn(async move {
        mgr.run(canceller_guard.clone_token()).await.unwrap();
    });

    started_notify_token.cancelled_owned().await;

    let tls_config = {
        let mut tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();

        {
            let mut dangerous_config = rustls::client::DangerousClientConfig {
                cfg: &mut tls_config,
            };
            dangerous_config.set_certificate_verifier(Arc::new(upstream::DangerousServerVerifier));
        }

        tls_config
    };

    let mut h3_builder = hickory_proto::h3::H3ClientStreamBuilder::default();
    h3_builder.crypto_config(tls_config);

    exchange(h3_builder, |message, h3_builder| async move {
        let mut stream = h3_builder
            .build(
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363),
                "127.0.0.1".to_string(),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut response_stream =
            stream.send_message(DnsRequest::new(message, DnsRequestOptions::default()));
        let response_message = response_stream
            .next()
            .await
            .ok_or_else(|| format!("{}", "response not found"))?
            .map_err(|e| e.to_string())?;

        Ok(response_message.into_message())
    })
    .await;

    canceller.cancel_and_wait().await;
}

#[cfg(feature = "listener-quic")]
#[tokio::test]
async fn test_quic_listener() {
    use crate::upstream;

    let mgr = build(option::ListenerInnerOptions::QUICListener(
        option::QUICListenerOptions {
            listen: "127.0.0.1:5363".to_string(),
            tls: option::ListenerTLSOptions {
                client_ca_file: Default::default(),
                server_cert_file: "/rsdns/src/tests/test-server-cert.pem".to_string(),
                server_key_file: "/rsdns/src/tests/test-server-key.pem".to_string(),
            },
            disable_prefix: false,
            generic: option::GenericListenerOptions {
                query_timeout: None,
                workflow: Default::default(),
            },
        },
    ))
    .await;
    let started_notify_token = mgr.started_notify_token();

    let (mut canceller, canceller_guard) = common::new_canceller();
    tokio::spawn(async move {
        mgr.run(canceller_guard.clone_token()).await.unwrap();
    });

    started_notify_token.cancelled_owned().await;

    let tls_config = {
        let mut tls_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();

        {
            let mut dangerous_config = rustls::client::DangerousClientConfig {
                cfg: &mut tls_config,
            };
            dangerous_config.set_certificate_verifier(Arc::new(upstream::DangerousServerVerifier));
        }

        tls_config
    };

    let mut quic_builder = hickory_proto::quic::QuicClientStreamBuilder::default();
    quic_builder.crypto_config(tls_config);

    exchange(quic_builder, |message, quic_builder| async move {
        let mut stream = quic_builder
            .build(
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5363),
                "127.0.0.1".to_string(),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut response_stream =
            stream.send_message(DnsRequest::new(message, DnsRequestOptions::default()));
        let response_message = response_stream
            .next()
            .await
            .ok_or_else(|| format!("{}", "response not found"))?
            .map_err(|e| e.to_string())?;

        Ok(response_message.into_message())
    })
    .await;

    canceller.cancel_and_wait().await;
}

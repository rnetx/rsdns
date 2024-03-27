use std::time::Duration;

use serde::Deserialize;

use crate::common;

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerOptions {
    pub tag: String,
    #[serde(flatten)]
    pub inner: ListenerInnerOptions,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ListenerInnerOptions {
    #[serde(alias = "tcp")]
    TCPListener(TCPListenerOptions),
    #[serde(alias = "udp")]
    UDPListener(UDPListenerOptions),
    #[serde(alias = "base")]
    BaseListener(BaseListenerOptions),
    #[serde(alias = "tls")]
    TLSListener(TLSListenerOptions),
    #[serde(alias = "http")]
    HTTPListener(HTTPListenerOptions),
    #[serde(alias = "quic")]
    QUICListener(QUICListenerOptions),
}

#[derive(Debug, Clone, Deserialize)]
pub struct UDPListenerOptions {
    pub listen: String,
    #[serde(flatten)]
    #[serde(default)]
    pub generic: GenericListenerOptions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TCPListenerOptions {
    pub listen: String,
    #[serde(flatten)]
    #[serde(default)]
    pub generic: GenericListenerOptions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaseListenerOptions {
    #[serde(rename = "listen-port")]
    pub listen_port: u16,
    #[serde(default)]
    #[serde(rename = "disable-ipv6")]
    pub disable_ipv6: bool,
    #[serde(flatten)]
    #[serde(default)]
    pub generic: GenericListenerOptions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TLSListenerOptions {
    pub listen: String,
    #[serde(flatten)]
    #[serde(default)]
    pub tls: ListenerTLSOptions,
    #[serde(flatten)]
    #[serde(default)]
    pub generic: GenericListenerOptions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HTTPListenerOptions {
    pub listen: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(rename = "use-http3")]
    #[serde(default)]
    pub use_http3: bool,
    #[serde(flatten)]
    #[serde(default)]
    pub tls: Option<ListenerTLSOptions>,
    #[serde(flatten)]
    #[serde(default)]
    pub generic: GenericListenerOptions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QUICListenerOptions {
    pub listen: String,
    #[serde(flatten)]
    #[serde(default)]
    pub tls: ListenerTLSOptions,
    #[serde(flatten)]
    #[serde(default)]
    pub generic: GenericListenerOptions,
}

// TLS

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerTLSOptions {
    #[serde(rename = "client-ca-file")]
    #[serde(default)]
    pub client_ca_file: common::SingleOrList<String>,
    #[serde(rename = "server-cert-file")]
    pub server_cert_file: String,
    #[serde(rename = "server-key-file")]
    pub server_key_file: String,
}

// Generic

#[derive(Debug, Clone, Deserialize)]
pub struct GenericListenerOptions {
    #[serde(rename = "query-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub query_timeout: Option<Duration>,
    pub workflow: String,
}

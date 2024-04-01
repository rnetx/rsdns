use std::{
    collections::HashMap,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamOptions {
    pub tag: String,
    #[serde(flatten)]
    pub inner: UpstreamInnerOptions,
    #[serde(rename = "query-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub query_timeout: Option<Duration>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum UpstreamInnerOptions {
    #[serde(alias = "tcp")]
    TCPUpstream(TCPUpstreamOptions),
    #[serde(alias = "udp")]
    UDPUpstream(UDPUpstreamOptions),
    #[serde(alias = "dhcp")]
    DHCPUpstream(DHCPUpstreamOptions),
    #[serde(alias = "tls")]
    TLSUpstream(TLSUpstreamOptions),
    #[serde(alias = "https")]
    HTTPSUpstream(HTTPSUpstreamOptions),
    #[serde(alias = "quic")]
    QUICUpstream(QUICUpstreamOptions),
}

#[derive(Debug, Clone, Deserialize)]
pub struct UDPUpstreamOptions {
    pub address: String,
    #[serde(rename = "idle-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub idle_timeout: Option<Duration>,
    #[serde(rename = "fallback-tcp")]
    #[serde(default)]
    pub fallback_tcp: bool,
    #[serde(rename = "enable-pipeline")]
    #[serde(default)]
    pub enable_pipeline: bool,
    #[serde(default)]
    pub bootstrap: Option<BootstrapOptions>,
    #[serde(flatten)]
    #[serde(default)]
    pub dialer: Option<DialerOptions>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TCPUpstreamOptions {
    pub address: String,
    #[serde(rename = "idle-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub idle_timeout: Option<Duration>,
    #[serde(rename = "enable-pipeline")]
    #[serde(default)]
    pub enable_pipeline: bool,
    #[serde(default)]
    pub bootstrap: Option<BootstrapOptions>,
    #[serde(flatten)]
    #[serde(default)]
    pub dialer: Option<DialerOptions>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DHCPUpstreamOptions {
    pub interface: String,
    #[serde(rename = "check-interval")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub check_interval: Option<Duration>,
    #[serde(rename = "idle-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub idle_timeout: Option<Duration>,
    #[serde(rename = "enable-pipeline")]
    #[serde(default)]
    pub enable_pipeline: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TLSUpstreamOptions {
    pub address: String,
    #[serde(rename = "idle-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub idle_timeout: Option<Duration>,
    #[serde(rename = "enable-pipeline")]
    #[serde(default)]
    pub enable_pipeline: bool,
    #[serde(default)]
    #[serde(flatten)]
    pub tls: UpstreamTLSOptions,
    #[serde(default)]
    pub bootstrap: Option<BootstrapOptions>,
    #[serde(flatten)]
    #[serde(default)]
    pub dialer: Option<DialerOptions>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HTTPSUpstreamOptions {
    pub address: String,
    #[serde(rename = "idle-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub idle_timeout: Option<Duration>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub header: Option<HashMap<String, String>>,
    #[serde(default)]
    #[serde(rename = "use-post")]
    pub use_post: bool,
    #[serde(default)]
    #[serde(rename = "use-http3")]
    pub use_http3: bool,
    #[serde(flatten)]
    #[serde(default)]
    pub tls: UpstreamTLSOptions,
    #[serde(default)]
    pub bootstrap: Option<BootstrapOptions>,
    #[serde(flatten)]
    #[serde(default)]
    pub dialer: Option<DialerOptions>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QUICUpstreamOptions {
    pub address: String,
    #[serde(rename = "idle-timeout")]
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub idle_timeout: Option<Duration>,
    #[serde(rename = "disable-add-prefix")]
    #[serde(default)]
    pub disable_add_prefix: bool,
    #[serde(default)]
    #[serde(flatten)]
    pub tls: UpstreamTLSOptions,
    #[serde(default)]
    pub bootstrap: Option<BootstrapOptions>,
    #[serde(flatten)]
    #[serde(default)]
    pub dialer: Option<DialerOptions>,
}

// TLS

#[serde_with::serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamTLSOptions {
    #[serde(rename = "server-name")]
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub insecure: bool,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(rename = "server-ca-file")]
    #[serde(default)]
    pub server_ca_file: Vec<String>,
    #[serde(rename = "client-cert-file")]
    #[serde(default)]
    pub client_cert_file: Option<String>,
    #[serde(rename = "client-key-file")]
    #[serde(default)]
    pub client_key_file: Option<String>,
}

// Bootstrap

#[derive(Debug, Default, Clone, Deserialize)]
pub struct BootstrapOptions {
    #[serde(default)]
    pub strategy: BootstrapStrategy,
    pub upstream: String,
}

#[derive(Debug, Clone, Deserialize)]
pub enum BootstrapStrategy {
    #[serde(rename = "prefer-ipv4")]
    PreferIPv4,
    #[serde(rename = "prefer-ipv6")]
    PreferIPv6,
    #[serde(rename = "only-ipv4")]
    OnlyIPv4,
    #[serde(rename = "only-ipv6")]
    OnlyIPv6,
}

impl Default for BootstrapStrategy {
    fn default() -> Self {
        Self::PreferIPv4
    }
}

// Dialer

#[derive(Debug, Default, Clone, Deserialize)]
pub struct DialerOptions {
    #[serde(flatten)]
    #[serde(default)]
    pub basic: BasicDialerOptions,
    pub socks5: Option<Socks5DialerOptions>,
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    pub connect_timeout: Option<Duration>,
}

// Dialer - Basic

#[derive(Debug, Default, Clone, Deserialize)]
pub struct BasicDialerOptions {
    #[serde(default)]
    #[serde(rename = "bind-ipv4")]
    pub bind_ipv4: Option<Ipv4Addr>,
    #[serde(default)]
    #[serde(rename = "bind-ipv6")]
    pub bind_ipv6: Option<Ipv6Addr>,
    #[serde(default)]
    #[serde(rename = "bind-interface")]
    pub bind_interface: Option<String>,
    #[serde(default)]
    #[serde(rename = "tcp-fast-open")]
    pub tcp_fast_open: bool,
    #[serde(default)]
    #[serde(rename = "so-mark")]
    pub so_mark: Option<u32>,
}

// Dialer - Socks5

#[derive(Debug, Clone, Deserialize)]
pub struct Socks5DialerOptions {
    pub address: SocketAddr,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use crate::{adapter, log, option};

pub(super) fn parse_with_default_port(
    s: &str,
    default_port: u16,
) -> Result<SocketAddr, Box<dyn Error + Send + Sync>> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if s.starts_with('[') && s.ends_with(']') {
        if let Ok(addr) = s[1..s.len() - 1].parse::<IpAddr>() {
            return Ok(SocketAddr::new(addr, default_port));
        }
    }
    if let Ok(addr) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(addr, default_port));
    }
    if let Ok(port) = s.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
    }
    if s.starts_with(':') {
        if let Ok(port) = s.trim_start_matches(':').parse::<u16>() {
            return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
        }
    }
    Err(format!("invalid address: {}", s).into())
}

pub(crate) fn new_listener(
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Box<dyn log::Logger>,
    tag: String,
    options: option::ListenerOptions,
) -> Result<Box<dyn adapter::Listener>, Box<dyn Error + Send + Sync>> {
    match options.inner {
        option::ListenerInnerOptions::UDPListener(udp_options) => {
            super::UDPListener::new(manager, logger, tag.clone(), udp_options)
                .map(|l| Box::new(l) as Box<dyn adapter::Listener>)
                .map_err(|err| format!("create udp listener [{}] failed: {}", tag, err).into())
        }
        option::ListenerInnerOptions::TCPListener(tcp_options) => {
            super::TCPListener::new(manager, logger, tag.clone(), tcp_options)
                .map(|l| Box::new(l) as Box<dyn adapter::Listener>)
                .map_err(|err| format!("create tcp listener [{}] failed: {}", tag, err).into())
        }
        option::ListenerInnerOptions::BaseListener(base_options) => {
            super::BaseListener::new(manager, logger, tag.clone(), base_options)
                .map(|l| Box::new(l) as Box<dyn adapter::Listener>)
                .map_err(|err| format!("create base listener [{}] failed: {}", tag, err).into())
        }

        #[cfg(feature = "listener-tls-support")]
        option::ListenerInnerOptions::TLSListener(tls_options) => {
            super::TLSListener::new(manager, logger, tag.clone(), tls_options)
                .map(|l| Box::new(l) as Box<dyn adapter::Listener>)
                .map_err(|err| format!("create tls listener [{}] failed: {}", tag, err).into())
        }

        #[cfg(feature = "listener-http-support")]
        option::ListenerInnerOptions::HTTPListener(http_options) => {
            super::HTTPListener::new(manager, logger, tag.clone(), http_options)
                .map(|l| Box::new(l) as Box<dyn adapter::Listener>)
                .map_err(|err| format!("create http listener [{}] failed: {}", tag, err).into())
        }

        #[cfg(all(feature = "listener-quic-support", feature = "listener-tls-support"))]
        option::ListenerInnerOptions::QUICListener(quic_options) => {
            super::QUICListener::new(manager, logger, tag.clone(), quic_options)
                .map(|l| Box::new(l) as Box<dyn adapter::Listener>)
                .map_err(|err| format!("create quic listener [{}] failed: {}", tag, err).into())
        }
    }
}

lazy_static::lazy_static! {
    static ref SUPPORTED_LISTENER_TYPES: Vec<&'static str> = {
        let mut types = vec!["tcp", "udp"];

        #[cfg(feature = "listener-tls-support")]
        types.push("tls");

        cfg_if::cfg_if! {
            if #[cfg(all(feature = "listener-http-support", feature = "listener-https-support", feature = "listener-quic-support", feature = "listener-tls-support"))] {
                types.push("http(support https and http3)");
            } else if #[cfg(all(feature = "listener-http-support", feature = "listener-https-support", feature = "listener-tls-support"))] {
                types.push("http(support https)");
            } else if #[cfg(all(feature = "listener-http-support"))] {
                types.push("http");
            }
        }

        #[cfg(all(feature = "listener-quic-support", feature = "listener-tls-support"))]
        types.push("quic");

        types
    };
}

pub fn supported_listener_types() -> Vec<&'static str> {
    SUPPORTED_LISTENER_TYPES.clone()
}

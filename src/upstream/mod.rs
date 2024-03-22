mod bootstrap;
mod network;
mod pipeline;
mod pool;
mod stream;
mod upstream;

mod upstream_tcp;
mod upstream_udp;

#[cfg(feature = "upstream-tls-support")]
mod tls;

#[cfg(feature = "upstream-tls-support")]
mod upstream_tls;

#[cfg(all(feature = "upstream-https-support", feature = "upstream-tls-support"))]
mod upstream_https;

#[cfg(all(feature = "upstream-quic-support", feature = "upstream-tls-support"))]
mod upstream_quic;

#[cfg(feature = "upstream-dhcp-support")]
mod upstream_dhcp;

use bootstrap::*;
pub use network::support_socks5_dialer;
use pipeline::*;
use pool::*;
use stream::*;
pub use upstream::*;
use upstream_tcp::*;
use upstream_udp::*;

#[cfg(feature = "upstream-tls-support")]
use tls::*;

#[cfg(all(feature = "upstream-https-support", feature = "upstream-tls-support"))]
use upstream_https::*;

#[cfg(feature = "upstream-tls-support")]
use upstream_tls::*;

#[cfg(all(feature = "upstream-quic-support", feature = "upstream-tls-support"))]
use upstream_quic::*;

#[cfg(feature = "upstream-dhcp-support")]
use upstream_dhcp::*;

// API

#[cfg(feature = "api")]
mod api;

#[cfg(feature = "api")]
pub(crate) use api::*;

// Test

#[cfg(test)]
mod test;

mod bootstrap;
mod network;
mod pipeline;
mod pool;
mod stream;
mod upstream;

mod upstream_tcp;
mod upstream_udp;

#[cfg(feature = "upstream-tls")]
mod tls;

#[cfg(feature = "upstream-tls")]
mod upstream_tls;

#[cfg(all(feature = "upstream-https", feature = "upstream-tls"))]
mod upstream_https;

#[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
mod upstream_quic;

#[cfg(feature = "upstream-dhcp")]
mod upstream_dhcp;

use bootstrap::*;
pub use network::support_socks5_dialer;
use pipeline::*;
use pool::*;
use stream::*;
pub use upstream::*;
use upstream_tcp::*;
use upstream_udp::*;

#[cfg(feature = "upstream-tls")]
use tls::*;

#[cfg(feature = "upstream-tls")]
pub(crate) use tls::DangerousServerVerifier;

#[cfg(all(feature = "upstream-https", feature = "upstream-tls"))]
use upstream_https::*;

#[cfg(feature = "upstream-tls")]
use upstream_tls::*;

#[cfg(all(feature = "upstream-quic", feature = "upstream-tls"))]
use upstream_quic::*;

#[cfg(feature = "upstream-dhcp")]
use upstream_dhcp::*;

// API

#[cfg(feature = "api")]
mod api;

#[cfg(feature = "api")]
pub(crate) use api::*;

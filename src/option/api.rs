use std::net::SocketAddr;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct APIServerOptions {
    pub listen: SocketAddr,
    pub secret: Option<String>,
}

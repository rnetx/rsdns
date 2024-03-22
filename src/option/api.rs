use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APIServerOptions {
    pub listen: SocketAddr,
    pub secret: Option<String>,
}

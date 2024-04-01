use std::{fmt, net::SocketAddr, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocksAddr {
    DomainAddr(String, u16),
    SocketAddr(SocketAddr),
}

impl fmt::Display for SocksAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DomainAddr(domain, port) => write!(f, "{}:{}", domain, port),
            Self::SocketAddr(addr) => write!(f, "{}", addr),
        }
    }
}

impl From<SocketAddr> for SocksAddr {
    fn from(value: SocketAddr) -> Self {
        Self::SocketAddr(value)
    }
}

impl FromStr for SocksAddr {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl SocksAddr {
    pub(crate) fn parse(s: &str) -> anyhow::Result<Self> {
        if s.is_empty() {
            return Err(anyhow::anyhow!("missing address"));
        }
        if let Ok(addr) = s.parse() {
            return Ok(Self::SocketAddr(addr));
        }
        let (domain, port) = match s.rsplit_once(':') {
            Some((domain, port)) => (domain, port),
            None => return Err(anyhow::anyhow!("invalid address: {}", s)),
        };
        if domain.is_empty() {
            return Err(anyhow::anyhow!("invalid address: {}", s));
        }
        if port.is_empty() {
            return Err(anyhow::anyhow!("invalid address: {}", s));
        }
        let port: u16 = port
            .parse()
            .map_err(|err| anyhow::anyhow!("invalid address port: {}: {}", port, err))?;
        if port == 0 {
            return Err(anyhow::anyhow!("invalid address port: {}", port));
        }
        Ok(Self::DomainAddr(domain.to_string(), port))
    }

    pub(crate) fn parse_with_default_port(s: &str, default_port: u16) -> anyhow::Result<Self> {
        if s.is_empty() {
            return Err(anyhow::anyhow!("missing address"));
        }
        if let Ok(addr) = s.parse() {
            return Ok(Self::SocketAddr(addr));
        }
        if let Ok(addr) = s.parse() {
            return Ok(Self::SocketAddr(SocketAddr::new(addr, default_port)));
        }
        if s.starts_with('[') && s.ends_with(']') {
            if let Ok(addr) = s[1..s.len() - 1].parse() {
                return Ok(Self::SocketAddr(SocketAddr::new(addr, default_port)));
            }
        }
        let (domain, port) = match s.rsplit_once(':') {
            Some((domain, port)) => (domain, port),
            None => return Ok(Self::DomainAddr(s.to_string(), default_port)),
        };
        if domain.is_empty() {
            return Err(anyhow::anyhow!("invalid address: {}", s));
        }
        if port.is_empty() {
            return Err(anyhow::anyhow!("invalid address: {}", s));
        }
        let port: u16 = port
            .parse()
            .map_err(|err| anyhow::anyhow!("invalid address port: {}: {}", port, err))?;
        if port == 0 {
            return Err(anyhow::anyhow!("invalid address port: {}", port));
        }
        Ok(Self::DomainAddr(domain.to_string(), port))
    }

    pub(crate) fn is_domain_addr(&self) -> bool {
        matches!(self, Self::DomainAddr(..))
    }

    pub(crate) fn port(&self) -> u16 {
        match self {
            Self::DomainAddr(_, port) => *port,
            Self::SocketAddr(addr) => addr.port(),
        }
    }

    pub(crate) fn must_domain_addr(&self) -> (&str, u16) {
        match self {
            Self::DomainAddr(domain, port) => (domain, *port),
            Self::SocketAddr(_) => panic!("not a domain address: {}", self),
        }
    }

    pub(crate) fn try_to_socket_addr_ref(&self) -> Option<&SocketAddr> {
        match self {
            Self::DomainAddr(_, _) => None,
            Self::SocketAddr(addr) => Some(addr),
        }
    }

    pub(crate) fn try_to_socket_addr(self) -> Option<SocketAddr> {
        match self {
            Self::DomainAddr(_, _) => None,
            Self::SocketAddr(addr) => Some(addr),
        }
    }
}

// Socks5 Serialization
#[cfg(feature = "upstream-dialer-socks5")]
impl SocksAddr {
    pub(crate) fn encode_to(&self, mut buf: impl bytes::BufMut) {
        match self {
            Self::DomainAddr(domain, port) => {
                buf.put_u8(0x03);
                buf.put_u8(domain.len() as u8);
                buf.put_slice(domain.as_bytes());
                let port_bytes = port.to_be_bytes();
                buf.put_slice(&port_bytes[..]);
            }
            Self::SocketAddr(SocketAddr::V4(addr)) => {
                buf.put_u8(0x01);
                buf.put_slice(&addr.ip().octets());
                let port_bytes = addr.port().to_be_bytes();
                buf.put_slice(&port_bytes[..]);
            }
            Self::SocketAddr(SocketAddr::V6(addr)) => {
                buf.put_u8(0x04);
                buf.put_slice(&addr.ip().octets());
                let port_bytes = addr.port().to_be_bytes();
                buf.put_slice(&port_bytes[..]);
            }
        }
    }

    pub(crate) fn encode_length(&self) -> usize {
        match self {
            Self::DomainAddr(domain, _) => 1 + 1 + domain.len() + 2,
            Self::SocketAddr(SocketAddr::V4(_)) => 1 + 4 + 2,
            Self::SocketAddr(SocketAddr::V6(_)) => 1 + 16 + 2,
        }
    }

    pub(crate) fn decode_from(buf: &[u8]) -> anyhow::Result<Self> {
        if buf.len() < 2 {
            return Err(anyhow::anyhow!("invalid address length"));
        }
        let address_type = buf[0];
        match address_type {
            0x01 => {
                if buf.len() < 7 {
                    return Err(anyhow::anyhow!("invalid address length"));
                }
                let ip =
                    std::net::IpAddr::V4(std::net::Ipv4Addr::new(buf[1], buf[2], buf[3], buf[4]));
                let port = u16::from_be_bytes([buf[5], buf[6]]);
                Ok(Self::SocketAddr(SocketAddr::new(ip, port)))
            }
            0x03 => {
                let domain_len = buf[1] as usize;
                if buf.len() < 2 + domain_len + 2 {
                    return Err(anyhow::anyhow!("invalid address length"));
                }
                let domain = String::from_utf8(buf[2..2 + domain_len].to_vec())?;
                let port = u16::from_be_bytes([buf[2 + domain_len], buf[3 + domain_len]]);
                Ok(Self::DomainAddr(domain, port))
            }
            0x04 => {
                if buf.len() < 19 {
                    return Err(anyhow::anyhow!("invalid address length"));
                }
                let ip = std::net::IpAddr::V6(std::net::Ipv6Addr::new(
                    u16::from_be_bytes([buf[1], buf[2]]),
                    u16::from_be_bytes([buf[3], buf[4]]),
                    u16::from_be_bytes([buf[5], buf[6]]),
                    u16::from_be_bytes([buf[7], buf[8]]),
                    u16::from_be_bytes([buf[9], buf[10]]),
                    u16::from_be_bytes([buf[11], buf[12]]),
                    u16::from_be_bytes([buf[13], buf[14]]),
                    u16::from_be_bytes([buf[15], buf[16]]),
                ));
                let port = u16::from_be_bytes([buf[17], buf[18]]);
                Ok(Self::SocketAddr(SocketAddr::new(ip, port)))
            }
            _ => Err(anyhow::anyhow!("invalid address type: {}", address_type)),
        }
    }
}

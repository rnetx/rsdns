use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum IPRange {
    V4(IPv4Range),
    V6(IPv6Range),
}

impl fmt::Debug for IPRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4(range) => range.fmt(f),
            Self::V6(range) => range.fmt(f),
        }
    }
}

impl fmt::Display for IPRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4(range) => range.fmt(f),
            Self::V6(range) => range.fmt(f),
        }
    }
}

impl IPRange {
    pub(crate) fn contains(&self, other: &IpAddr) -> bool {
        match self {
            Self::V4(range) => match other {
                IpAddr::V4(ip) => range.contains(ip),
                _ => false,
            },
            Self::V6(range) => match other {
                IpAddr::V6(ip) => range.contains(ip),
                _ => false,
            },
        }
    }

    pub(crate) fn contains_v4(&self, other: &Ipv4Addr) -> bool {
        match self {
            Self::V4(range) => range.contains(other),
            _ => false,
        }
    }

    pub(crate) fn contains_v6(&self, other: &Ipv6Addr) -> bool {
        match self {
            Self::V6(range) => range.contains(other),
            _ => false,
        }
    }

    pub(crate) fn range_to_ip_or_cidr(self) -> Vec<Self> {
        match self {
            Self::V4(range) => range
                .range_to_ip_or_cidr()
                .into_iter()
                .map(Self::V4)
                .collect(),
            Self::V6(range) => range
                .range_to_ip_or_cidr()
                .into_iter()
                .map(Self::V6)
                .collect(),
        }
    }
}

impl FromStr for IPRange {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err_fn = || Err(anyhow::anyhow!("invalid ip-range: {}", s));

        if let Ok(ip) = s.parse::<IpAddr>() {
            return Ok(match ip {
                IpAddr::V4(ip) => Self::V4(IPv4Range::Single(ip)),
                IpAddr::V6(ip) => Self::V6(IPv6Range::Single(ip)),
            });
        }

        if s.contains('/') {
            if let Ok(addr) = s.parse::<ipnet::IpNet>() {
                return Ok(match addr {
                    ipnet::IpNet::V4(cidr) => Self::V4(IPv4Range::CIDR(cidr)),
                    ipnet::IpNet::V6(cidr) => Self::V6(IPv6Range::CIDR(cidr)),
                });
            }
        }

        if s.contains('-') {
            if let Some((start, end)) = s.split_once('-') {
                let start = match start.trim().parse::<IpAddr>() {
                    Ok(ip) => ip,
                    Err(_) => return err_fn(),
                };
                let end = match end.trim().parse::<IpAddr>() {
                    Ok(ip) => ip,
                    Err(_) => return err_fn(),
                };
                match (start, end) {
                    (IpAddr::V4(start), IpAddr::V4(end)) => {
                        if start > end {
                            return err_fn();
                        }
                        if start == end {
                            return Ok(Self::V4(IPv4Range::Single(start)));
                        }
                        return Ok(Self::V4(IPv4Range::Range(start, end)));
                    }
                    (IpAddr::V6(start), IpAddr::V6(end)) => {
                        if start > end {
                            return err_fn();
                        }
                        if start == end {
                            return Ok(Self::V6(IPv6Range::Single(start)));
                        }
                        return Ok(Self::V6(IPv6Range::Range(start, end)));
                    }
                    _ => return err_fn(),
                }
            }
        }

        err_fn()
    }
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum IPv4Range {
    Single(Ipv4Addr),
    Range(Ipv4Addr, Ipv4Addr),
    CIDR(ipnet::Ipv4Net),
}

impl fmt::Debug for IPv4Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(ip) => write!(f, "{}", ip),
            Self::Range(start, end) => write!(f, "{} - {}", start, end),
            Self::CIDR(cidr) => write!(f, "{}", cidr),
        }
    }
}

impl fmt::Display for IPv4Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(ip) => write!(f, "{}", ip),
            Self::Range(start, end) => write!(f, "{} - {}", start, end),
            Self::CIDR(cidr) => write!(f, "{}", cidr),
        }
    }
}

impl IPv4Range {
    pub(crate) fn contains(&self, other: &Ipv4Addr) -> bool {
        match self {
            Self::Single(ip) => ip == other,
            Self::Range(start, end) => start <= other && other <= end,
            Self::CIDR(cidr) => cidr.contains(other),
        }
    }

    pub(crate) fn range_to_ip_or_cidr(self) -> Vec<Self> {
        match self {
            Self::Single(ip) => vec![Self::Single(ip)],
            Self::CIDR(cidr) => vec![Self::CIDR(cidr)],
            Self::Range(start, end) => {
                let mut start_num = u32::from_be_bytes(start.octets());
                let end_num = u32::from_be_bytes(end.octets());
                let mut result = Vec::new();
                while start_num < end_num {
                    let mut n = start_num.trailing_zeros() as u8;
                    let mut mask = u32::MAX << n;
                    let mut range_start = start_num & mask;
                    let mut range_end = range_start | !mask;
                    while range_end > end_num {
                        n -= 1;
                        mask = u32::MAX << n;
                        range_start = start_num & mask;
                        range_end = range_start | !mask;
                    }
                    result.push(ipnet::Ipv4Net::new(Ipv4Addr::from(range_start), 32 - n).unwrap());
                    start_num = range_end + 1;
                }
                if start_num == end_num {
                    result.push(ipnet::Ipv4Net::new(Ipv4Addr::from(start_num), 32).unwrap());
                }
                result
                    .into_iter()
                    .map(|n| {
                        if n.prefix_len() == 32 {
                            Self::Single(n.addr())
                        } else {
                            Self::CIDR(n)
                        }
                    })
                    .collect()
            }
        }
    }
}

impl FromStr for IPv4Range {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err_fn = || Err(anyhow::anyhow!("invalid ip-range: {}", s));

        if let Ok(ip) = s.parse::<Ipv4Addr>() {
            return Ok(Self::Single(ip));
        }

        if s.contains('/') {
            if let Ok(addr) = s.parse::<ipnet::Ipv4Net>() {
                return Ok(Self::CIDR(addr));
            }
        }

        if s.contains('-') {
            if let Some((start, end)) = s.split_once('-') {
                let start = match start.trim().parse::<Ipv4Addr>() {
                    Ok(ip) => ip,
                    Err(_) => return err_fn(),
                };
                let end = match end.trim().parse::<Ipv4Addr>() {
                    Ok(ip) => ip,
                    Err(_) => return err_fn(),
                };
                if start > end {
                    return err_fn();
                }
                if start == end {
                    return Ok(Self::Single(start));
                }
                return Ok(Self::Range(start, end));
            }
        }

        err_fn()
    }
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum IPv6Range {
    Single(Ipv6Addr),
    Range(Ipv6Addr, Ipv6Addr),
    CIDR(ipnet::Ipv6Net),
}

impl fmt::Debug for IPv6Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(ip) => write!(f, "{}", ip),
            Self::Range(start, end) => write!(f, "{} - {}", start, end),
            Self::CIDR(cidr) => write!(f, "{}", cidr),
        }
    }
}

impl fmt::Display for IPv6Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(ip) => write!(f, "{}", ip),
            Self::Range(start, end) => write!(f, "{} - {}", start, end),
            Self::CIDR(cidr) => write!(f, "{}", cidr),
        }
    }
}

impl IPv6Range {
    pub(crate) fn contains(&self, other: &Ipv6Addr) -> bool {
        match self {
            Self::Single(ip) => ip == other,
            Self::Range(start, end) => start <= other && other <= end,
            Self::CIDR(cidr) => cidr.contains(other),
        }
    }

    pub(crate) fn range_to_ip_or_cidr(self) -> Vec<Self> {
        match self {
            Self::Single(ip) => vec![Self::Single(ip)],
            Self::CIDR(cidr) => vec![Self::CIDR(cidr)],
            Self::Range(start, end) => {
                let mut start_num = u128::from_be_bytes(start.octets());
                let end_num = u128::from_be_bytes(end.octets());
                let mut result = Vec::new();
                while start_num < end_num {
                    let mut n = start_num.trailing_zeros() as u8;
                    let mut mask = u128::MAX << n;
                    let mut range_start = start_num & mask;
                    let mut range_end = range_start | !mask;
                    while range_end > end_num {
                        n -= 1;
                        mask = u128::MAX << n;
                        range_start = start_num & mask;
                        range_end = range_start | !mask;
                    }
                    result.push(ipnet::Ipv6Net::new(Ipv6Addr::from(range_start), 128 - n).unwrap());
                    start_num = range_end + 1;
                }
                if start_num == end_num {
                    result.push(ipnet::Ipv6Net::new(Ipv6Addr::from(start_num), 128).unwrap());
                }
                result
                    .into_iter()
                    .map(|n| {
                        if n.prefix_len() == 128 {
                            Self::Single(n.addr())
                        } else {
                            Self::CIDR(n)
                        }
                    })
                    .collect()
            }
        }
    }
}

impl FromStr for IPv6Range {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err_fn = || Err(anyhow::anyhow!("invalid ip-range: {}", s));

        if let Ok(ip) = s.parse::<Ipv6Addr>() {
            return Ok(Self::Single(ip));
        }

        if s.contains('/') {
            if let Ok(addr) = s.parse::<ipnet::Ipv6Net>() {
                return Ok(Self::CIDR(addr));
            }
        }

        if s.contains('-') {
            if let Some((start, end)) = s.split_once('-') {
                let start = match start.trim().parse::<Ipv6Addr>() {
                    Ok(ip) => ip,
                    Err(_) => return err_fn(),
                };
                let end = match end.trim().parse::<Ipv6Addr>() {
                    Ok(ip) => ip,
                    Err(_) => return err_fn(),
                };
                if start > end {
                    return err_fn();
                }
                if start == end {
                    return Ok(Self::Single(start));
                }
                return Ok(Self::Range(start, end));
            }
        }

        err_fn()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_ip_range() {
        println!("{}", IPRange::from_str("192.168.0.1").unwrap());
        println!("{}", IPRange::from_str("192.168.0.1/24").unwrap());
        println!("{}", IPRange::from_str("192.168.0.0/24").unwrap());
        println!(
            "{}",
            IPRange::from_str("192.168.0.1 - 192.168.0.10").unwrap()
        );
        println!("{}", IPRange::from_str("2001:db1::").unwrap());
        println!("{}", IPRange::from_str("2001:db1::/48").unwrap());
        println!("{}", IPRange::from_str("2001:db1::1/48").unwrap());
        println!(
            "{}",
            IPRange::from_str("2001:db1:: - 2001:db1::10").unwrap()
        );
    }

    #[test]
    fn test_range_to_ip_cidr() {
        let ip_start = Ipv4Addr::from_str("1.1.1.78").unwrap();
        let ip_end = Ipv4Addr::from_str("1.1.1.111").unwrap();
        let range = IPv4Range::Range(ip_start, ip_end);
        let v = range.range_to_ip_or_cidr();
        println!("{:?}", v);

        let ip_start = Ipv6Addr::from_str("2001:db8::").unwrap();
        let ip_end = Ipv6Addr::from_str("2001:dcc::").unwrap();
        let range = IPv6Range::Range(ip_start, ip_end);
        let v = range.range_to_ip_or_cidr();
        println!("{:?}", v);
    }
}

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use dhcproto::{Decodable, Encodable};
use hickory_proto::op::Message;
use tokio::{
    net::UdpSocket,
    sync::{Mutex, RwLock},
};

use crate::{
    adapter::{self, Common as _},
    common, debug, error, info, log, option,
};

use super::network;

const DHCP_UPSTREAM_TYPE: &str = "dhcp";

#[derive(Clone)]
pub(crate) struct DHCPUpstream {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    interface: String,
    enable_pipeline: bool,
    idle_timeout: Option<Duration>,
    check_interval: Option<Duration>,
    //
    servers: Arc<Mutex<Vec<IpAddr>>>,
    upstream_map: Arc<RwLock<HashMap<IpAddr, Arc<Box<dyn adapter::Upstream>>>>>,
    canceller: Arc<Mutex<Option<common::Canceller>>>,
}

impl DHCPUpstream {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Arc<Box<dyn log::Logger>>,
        tag: String,
        options: option::DHCPUpstreamOptions,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            manager,
            logger,
            tag,
            interface: options.interface,
            enable_pipeline: options.enable_pipeline,
            idle_timeout: options.idle_timeout,
            check_interval: options.check_interval,
            servers: Arc::new(Mutex::new(Vec::new())),
            upstream_map: Arc::new(RwLock::new(HashMap::new())),
            canceller: Arc::new(Mutex::new(None)),
        })
    }

    async fn find_dns_servers(&self) -> anyhow::Result<Vec<IpAddr>> {
        let interface = self
            .manager
            .get_state_map()
            .try_get::<network::InterfaceFinder>()
            .ok_or(anyhow::anyhow!("interface finder not found"))?
            .find_interface(&self.interface)
            .await
            .ok_or(anyhow::anyhow!("interface [{}] not found", self.interface))?;
        tokio::select! {
          _ = tokio::time::sleep(Duration::from_secs(10)) => {
            Err(anyhow::anyhow!("find dns servers timeout"))
          }
          res = Self::find_dns_server_v4_by_interface(&self.manager, &interface) => {
            match res {
              Ok(v) => {
                let mut ips = v.into_iter().map(|x| IpAddr::V4(x)).collect::<Vec<_>>();
                ips.sort();
                Ok(ips)
              },
              Err(e) => Err(anyhow::anyhow!("find dns server failed: {}", e))
            }
          }
        }
    }

    async fn find_dns_server_v4_by_interface(
        manager: &Box<dyn adapter::Manager>,
        interface: &network_interface::NetworkInterface,
    ) -> anyhow::Result<Vec<Ipv4Addr>> {
        let mac_addr = if let Some(mac_addr) = &interface.mac_addr {
            mac_addr
        } else {
            return Err(anyhow::anyhow!("mac address not found"));
        };

        cfg_if::cfg_if! {
          if #[cfg(any(target_os = "linux", target_os = "android"))] {
            const LISTEN_ADDR: &str = "255.255.255.255:68";
          } else {
            const LISTEN_ADDR: &str = "0.0.0.0:68";
          }
        }
        const DST_ADDR: &str = "255.255.255.255:67";

        let listen_addr = LISTEN_ADDR.parse::<SocketAddr>().unwrap();
        let dst_addr = DST_ADDR.parse::<SocketAddr>().unwrap();
        let udp_socket = UdpSocket::bind(listen_addr).await?;
        if let Some(finder) = manager
            .get_state_map()
            .try_get::<network::InterfaceFinder>()
        {
            network::set_interface(&udp_socket, finder, &interface.name, false).await?;
        } else {
            return Err(anyhow::anyhow!("interface finder not found"));
        }
        udp_socket.set_broadcast(true)?;
        // DHCP
        let mac_addr_slice = mac_addr
            .split(':')
            .map(|x| {
                u8::from_str_radix(x, 10).map_err(|_| anyhow::anyhow!("malformed mac address"))
            })
            .collect::<anyhow::Result<Vec<u8>>>()?;
        let mut msg = dhcproto::v4::Message::default();
        msg.set_flags(dhcproto::v4::Flags::default().set_broadcast())
            .set_chaddr(&mac_addr_slice)
            .opts_mut()
            .insert(dhcproto::v4::DhcpOption::MessageType(
                dhcproto::v4::MessageType::Discover,
            ));
        msg.opts_mut()
            .insert(dhcproto::v4::DhcpOption::ParameterRequestList(vec![
                dhcproto::v4::OptionCode::SubnetMask,
                dhcproto::v4::OptionCode::Router,
                dhcproto::v4::OptionCode::DomainNameServer,
                dhcproto::v4::OptionCode::DomainName,
            ]));
        let xid = msg.xid();
        {
            let buf = msg.to_vec().expect("dhcp: failed to encode dhcp message");
            udp_socket.send_to(&buf, dst_addr).await?;
        }
        loop {
            let mut buf = vec![0u8; 576];
            let (n, _) = udp_socket.recv_from(&mut buf).await?;
            if let Ok(msg) = dhcproto::v4::Message::from_bytes(&buf[..n]) {
                if msg.xid() == xid {
                    if let Some(op) = msg.opts().get(dhcproto::v4::OptionCode::MessageType) {
                        match op {
                            dhcproto::v4::DhcpOption::MessageType(msg_type) => {
                                if msg_type == &dhcproto::v4::MessageType::Offer {
                                    if let Some(op) =
                                        msg.opts().get(dhcproto::v4::OptionCode::DomainNameServer)
                                    {
                                        match op {
                                            dhcproto::v4::DhcpOption::DomainNameServer(dns) => {
                                                return Ok(dns.clone());
                                            }
                                            _ => {
                                                return Err(anyhow::anyhow!(
                                                    "malformed response dhcp message"
                                                ))
                                            }
                                        }
                                    }
                                }
                            }
                            _ => return Err(anyhow::anyhow!("malformed response dhcp message")),
                        }
                    }
                }
            }
        }
    }

    async fn reload_upstream(&self, new_servers: Vec<IpAddr>) {
        let mut old_servers = self.servers.lock().await;
        let need_delete_ips = old_servers
            .iter()
            .filter(|x| !new_servers.contains(x))
            .cloned()
            .collect::<Vec<_>>();
        let need_add_ips = new_servers
            .iter()
            .filter(|x| !old_servers.contains(x))
            .cloned()
            .collect::<Vec<_>>();
        *old_servers = new_servers;

        // Add Upstream
        let mut need_add_upstreams = Vec::with_capacity(need_add_ips.len());
        for ip in &need_add_ips {
            let options = option::UDPUpstreamOptions {
                address: SocketAddr::new(*ip, 53).to_string(),
                idle_timeout: self.idle_timeout,
                enable_pipeline: self.enable_pipeline,
                fallback_tcp: true,
                bootstrap: None,
                dialer: None,
            };
            let u = super::UDPUpstream::new(
                self.manager.clone(),
                self.logger.clone(),
                self.tag.clone(),
                options,
            )
            .expect("dhcp: failed to create upstream");
            u.start().await.expect("dhcp: failed to start upstream");
            need_add_upstreams.push((*ip, Arc::new(Box::new(u) as Box<dyn adapter::Upstream>)));
        }
        let mut need_delete_upstreams = Vec::with_capacity(need_delete_ips.len());
        // Delete Upstream && Add Upstream
        {
            let mut upstream_map = self.upstream_map.write().await;
            for ip in &need_delete_ips {
                if let Some(u) = upstream_map.remove(ip) {
                    need_delete_upstreams.push(u);
                }
            }
            for (ip, u) in need_add_upstreams {
                upstream_map.insert(ip, u);
            }
        }
        // Close Upstream
        for u in need_delete_upstreams {
            u.close().await.expect("dhcp: failed to close upstream");
        }
    }

    async fn handle(self, canceller_guard: common::CancellerGuard) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.check_interval.unwrap()) => {
                    let ips = tokio::select! {
                        res = self.find_dns_servers() => {
                            match res {
                                Ok(ips) => {
                                    debug!(self.logger, "find dns servers: {:?}", ips);
                                    ips
                                }
                                Err(e) => {
                                    error!(self.logger, "find dns servers failed: {}", e);
                                    continue;
                                }
                            }
                        },
                        _ = canceller_guard.cancelled() => {
                            break;
                        }
                    };
                    tokio::select! {
                        _ = self.reload_upstream(ips) => {
                            debug!(self.logger, "reload upstream success");
                        }
                        _ = canceller_guard.cancelled() => {
                            break;
                        }
                    }
                }
                _ = canceller_guard.cancelled() => {
                    break;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for DHCPUpstream {
    async fn start(&self) -> anyhow::Result<()> {
        let ips = self.find_dns_servers().await?;
        info!(
            self.logger,
            "find dns servers: {:?}, interface: {}", ips, self.interface
        );
        self.reload_upstream(ips).await;
        if self.check_interval.is_some() {
            let s = self.clone();
            let (canceller, canceller_guard) = common::new_canceller();
            tokio::spawn(async move { s.handle(canceller_guard).await });
            *self.canceller.lock().await = Some(canceller);
        }
        Ok(())
    }

    async fn close(&self) -> anyhow::Result<()> {
        if let Some(mut canceller) = self.canceller.lock().await.take() {
            canceller.cancel_and_wait().await;
        }
        let mut upstream_map = self.upstream_map.write().await;
        for (_, u) in upstream_map.drain() {
            u.close().await.ok();
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::Upstream for DHCPUpstream {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        DHCP_UPSTREAM_TYPE
    }

    fn dependencies(&self) -> Option<Vec<String>> {
        None
    }

    fn logger(&self) -> &Arc<Box<dyn log::Logger>> {
        &self.logger
    }

    async fn exchange(
        &self,
        log_tracker: Option<&log::Tracker>,
        request: &mut Message,
    ) -> anyhow::Result<Message> {
        let upstream_map = self.upstream_map.read().await;
        if let Some(u) = upstream_map.values().next() {
            return u.exchange(log_tracker, request).await;
        } else {
            error!(
                self.logger,
                { option_tracker = log_tracker },
                "no available upstream"
            );
            return Err(anyhow::anyhow!("no available upstream"));
        }
    }
}

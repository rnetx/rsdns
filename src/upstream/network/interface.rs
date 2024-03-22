use std::{collections::HashMap, sync::Arc};

use network_interface::NetworkInterfaceConfig;
use tokio::sync::{Mutex, RwLock};

use crate::common;

pub(crate) struct InterfaceFinder {
    map: Arc<RwLock<HashMap<String, network_interface::NetworkInterface>>>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl Default for InterfaceFinder {
    fn default() -> Self {
        Self::new()
    }
}

impl InterfaceFinder {
    pub(crate) fn new() -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
            canceller: Mutex::new(None),
        }
    }

    fn list_interfaces() -> HashMap<String, network_interface::NetworkInterface> {
        let mut map_name_to_idx = HashMap::new();
        if let Ok(list) = network_interface::NetworkInterface::show() {
            for interface in list {
                map_name_to_idx.insert(interface.name.clone(), interface);
            }
        }
        map_name_to_idx.shrink_to_fit();
        map_name_to_idx
    }

    async fn handle(
        canceller_guard: common::CancellerGuard,
        map: Arc<RwLock<HashMap<String, network_interface::NetworkInterface>>>,
    ) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                    let map_name_to_idx = Self::list_interfaces();
                    let mut map = map.write().await;
                    *map = map_name_to_idx;
                }
                _ = canceller_guard.cancelled() => {
                    break;
                }
            }
        }
    }

    pub(crate) async fn start(&self) {
        let map_name_to_idx = Self::list_interfaces();
        *self.map.write().await = map_name_to_idx;
        let map = self.map.clone();
        let (canceller, canceller_guard) = common::new_canceller();
        tokio::spawn(async move { Self::handle(canceller_guard, map).await });
        self.canceller.lock().await.replace(canceller);
    }

    pub(crate) async fn close(&self) {
        if let Some(mut canceller) = self.canceller.lock().await.take() {
            canceller.cancel_and_wait().await;
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn find_id_by_tag(&self, tag: &str) -> Option<u32> {
        let locker = self.map.read().await;
        locker.get(tag).map(|iface| iface.index)
    }

    #[allow(dead_code)]
    pub(crate) async fn find_interface(
        &self,
        tag: &str,
    ) -> Option<network_interface::NetworkInterface> {
        let locker = self.map.read().await;
        locker.get(tag).cloned()
    }
}

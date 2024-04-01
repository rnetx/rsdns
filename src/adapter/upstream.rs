use std::sync::Arc;

use hickory_proto::op::Message;

use crate::log;

#[async_trait::async_trait]
pub(crate) trait Upstream: super::Common + Send + Sync {
    fn tag(&self) -> &str;
    fn r#type(&self) -> &'static str;
    fn dependencies(&self) -> Option<Vec<String>>;
    fn logger(&self) -> &Arc<Box<dyn log::Logger>>;
    async fn exchange(
        &self,
        log_tracker: Option<&log::Tracker>,
        request: &mut Message,
    ) -> anyhow::Result<Message>;
}

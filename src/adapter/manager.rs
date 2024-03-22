use std::sync::Arc;

#[async_trait::async_trait]
pub(crate) trait Manager: Send + Sync {
    async fn fail_to_close(&self, msg: String);

    async fn get_upstream(&self, tag: &str) -> Option<Arc<Box<dyn super::Upstream>>>;
    async fn list_upstream(&self) -> Vec<Arc<Box<dyn super::Upstream>>>;

    async fn get_workflow(&self, tag: &str) -> Option<Arc<Box<dyn super::Workflow>>>;
    async fn list_workflow(&self) -> Vec<Arc<Box<dyn super::Workflow>>>;

    async fn get_matcher_plugin(&self, tag: &str) -> Option<Arc<Box<dyn super::MatcherPlugin>>>;
    async fn list_matcher_plugin(&self) -> Vec<Arc<Box<dyn super::MatcherPlugin>>>;

    async fn get_executor_plugin(&self, tag: &str) -> Option<Arc<Box<dyn super::ExecutorPlugin>>>;
    async fn list_executor_plugin(&self) -> Vec<Arc<Box<dyn super::ExecutorPlugin>>>;

    fn get_state_map(&self) -> &state::TypeMap![Send + Sync];

    fn api_enabled(&self) -> bool;
}

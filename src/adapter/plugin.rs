#[async_trait::async_trait]
pub(crate) trait MatcherPlugin: super::Common + Send + Sync {
    /// matcher plugin tag, used to identify the matcher plugin, must be unique
    fn tag(&self) -> &str;

    /// matcher plugin type
    fn r#type(&self) -> &'static str;

    /// prepare workflow args, return args_id, plugin must store the args
    async fn prepare_workflow_args(&self, args: serde_yaml::Value) -> anyhow::Result<u16>;

    /// match the workflow, return true if matched
    async fn r#match(&self, ctx: &mut super::Context, args_id: u16) -> anyhow::Result<bool>;

    #[cfg(feature = "api")]
    /// return the api router, if the plugin does not provide the api, return None
    fn api_router(&self) -> Option<axum::Router> {
        None
    }
}

#[async_trait::async_trait]
pub(crate) trait ExecutorPlugin: super::Common + Send + Sync {
    /// executor plugin tag, used to identify the executor plugin, must be unique
    fn tag(&self) -> &str;

    /// executor plugin type
    fn r#type(&self) -> &'static str;

    /// prepare workflow args, return args_id, plugin must store the args
    async fn prepare_workflow_args(&self, args: serde_yaml::Value) -> anyhow::Result<u16>;

    /// execute the workflow, return the return-mode
    async fn execute(
        &self,
        ctx: &mut super::Context,
        args_id: u16,
    ) -> anyhow::Result<super::ReturnMode>;

    #[cfg(feature = "api")]
    /// return the api router, if the plugin does not provide the api, return None
    fn api_router(&self) -> Option<axum::Router> {
        None
    }
}

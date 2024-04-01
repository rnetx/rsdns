#[async_trait::async_trait]
pub(crate) trait Common {
    async fn start(&self) -> anyhow::Result<()>;
    async fn close(&self) -> anyhow::Result<()>;
}

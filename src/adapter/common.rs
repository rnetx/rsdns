use std::error::Error;

#[async_trait::async_trait]
pub(crate) trait Common {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>>;
    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>>;
}

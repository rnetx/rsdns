use std::{error::Error, fmt};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnMode {
    Continue,
    ReturnOnce,
    ReturnAll,
}

impl fmt::Debug for ReturnMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continue => write!(f, "continue"),
            Self::ReturnOnce => write!(f, "return-once"),
            Self::ReturnAll => write!(f, "return-all"),
        }
    }
}

impl fmt::Display for ReturnMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continue => write!(f, "continue"),
            Self::ReturnOnce => write!(f, "return-once"),
            Self::ReturnAll => write!(f, "return-all"),
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait Workflow: Send + Sync {
    fn tag(&self) -> &str;
    async fn check(&self) -> Result<(), Box<dyn Error + Send + Sync>>;
    async fn execute(
        &self,
        ctx: &mut super::Context,
    ) -> Result<ReturnMode, Box<dyn Error + Send + Sync>>;
}

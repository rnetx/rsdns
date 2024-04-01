use std::sync::Arc;

use crate::{adapter, debug, log, option};

pub(crate) struct Workflow {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    tag: String,
    rules: Vec<super::WorkflowRule>,
}

impl Workflow {
    pub(crate) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: option::WorkflowOptions,
    ) -> anyhow::Result<Self> {
        if options.rules.is_empty() {
            return Err(anyhow::anyhow!("missing rule"));
        }
        let mut l = Vec::with_capacity(options.rules.len());
        for (i, rule) in options.rules.into_iter().enumerate() {
            let r = super::WorkflowRule::new(rule)
                .map_err(|err| anyhow::anyhow!("create rule[{}] failed: {}", i, err))?;
            l.push(r);
        }
        Ok(Self {
            manager,
            logger: Arc::new(logger),
            tag,
            rules: l,
        })
    }
}

#[async_trait::async_trait]
impl adapter::Workflow for Workflow {
    fn tag(&self) -> &str {
        &self.tag
    }

    async fn check(&self) -> anyhow::Result<()> {
        for (i, rule) in self.rules.iter().enumerate() {
            if let Err(e) = rule.check(&self.manager).await {
                return Err(anyhow::anyhow!("check rule[{}] failed: {}", i, e));
            }
        }
        Ok(())
    }

    async fn execute(&self, ctx: &mut adapter::Context) -> anyhow::Result<adapter::ReturnMode> {
        for (i, rule) in self.rules.iter().enumerate() {
            debug!(
                self.logger,
                { tracker = ctx.log_tracker() },
                "run rule[{}]",
                i
            );
            match rule.run(&self.logger, ctx).await {
                Ok(adapter::ReturnMode::Continue) => {}
                Ok(adapter::ReturnMode::ReturnOnce) => {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "run rule[{}]: return once",
                        i
                    );
                    break;
                }
                Ok(adapter::ReturnMode::ReturnAll) => {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "run rule[{}]: return all",
                        i
                    );
                    return Ok(adapter::ReturnMode::ReturnAll);
                }
                Err(e) => return Err(anyhow::anyhow!("run rule[{}] failed: {}", i, e)),
            }
        }
        Ok(adapter::ReturnMode::Continue)
    }
}

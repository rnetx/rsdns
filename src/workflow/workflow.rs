use std::{error::Error, sync::Arc};

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
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let rules = options.rules.into_list();
        if rules.len() == 0 {
            return Err("missing rule".into());
        }
        let mut l = Vec::with_capacity(rules.len());
        for (i, rule) in rules.into_iter().enumerate() {
            let r = super::WorkflowRule::new(rule).map_err::<Box<dyn Error + Send + Sync>, _>(
                |err| format!("create rule[{}] failed: {}", i, err).into(),
            )?;
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

    async fn check(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        for (i, rule) in self.rules.iter().enumerate() {
            if let Err(e) = rule.check(&self.manager).await {
                return Err(format!("check rule[{}] failed: {}", i, e).into());
            }
        }
        Ok(())
    }

    async fn execute(
        &self,
        ctx: &mut adapter::Context,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
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
                Err(e) => return Err(format!("run rule[{}] failed: {}", i, e).into()),
            }
        }
        Ok(adapter::ReturnMode::Continue)
    }
}

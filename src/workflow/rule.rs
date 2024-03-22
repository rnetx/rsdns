use std::{error::Error, sync::Arc};

use crate::{adapter, debug, error, log, option};

pub(super) enum WorkflowRule {
    MatchAnd(MatchRule),
    MatchOr(MatchRule),
    Exec(ExecRule),
}

impl WorkflowRule {
    pub(super) fn new(
        options: option::WorkflowRuleOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        match options {
            option::WorkflowRuleOptions::MatchAnd(options) => {
                let rule = MatchRule::new_and(options)?;
                Ok(Self::MatchAnd(rule))
            }
            option::WorkflowRuleOptions::MatchOr(options) => {
                let rule = MatchRule::new_or(options)?;
                Ok(Self::MatchOr(rule))
            }
            option::WorkflowRuleOptions::Exec(options) => {
                let rule = ExecRule::new(options)?;
                Ok(Self::Exec(rule))
            }
        }
    }

    pub(super) async fn check(
        &self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self {
            Self::MatchAnd(rule) => rule.check(manager).await,
            Self::MatchOr(rule) => rule.check(manager).await,
            Self::Exec(rule) => rule.check(manager).await,
        }
    }

    pub(super) async fn run(
        &self,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        match self {
            Self::MatchAnd(rule) => rule.run(logger, ctx).await,
            Self::MatchOr(rule) => rule.run(logger, ctx).await,
            Self::Exec(rule) => rule.run(logger, ctx).await,
        }
    }
}

enum Logical {
    And,
    Or,
}

pub(super) struct MatchRule {
    logical: Logical,
    matchers: Vec<super::MatchItemRule>,
    exec: Option<Vec<super::ExecItemRule>>,
    else_exec: Option<Vec<super::ExecItemRule>>,
}

impl MatchRule {
    pub(super) fn new_and(
        options: option::MatchAndRuleOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let match_and = options.match_and.into_list();
        let mut matchers = Vec::with_capacity(match_and.len());
        for (i, options) in match_and.into_iter().enumerate() {
            let rule = super::MatchItemRule::new(options)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("create match-and-rule: match-and[{}] failed: {}", i, err).into()
                })?;
            matchers.push(rule);
        }
        let execs = if let Some(exec) = options.exec {
            let exec = exec.into_list();
            let mut execs = Vec::with_capacity(exec.len());
            for (i, options) in exec.into_iter().enumerate() {
                let rule = super::ExecItemRule::new(options)
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("create match-and-rule: exec[{}] failed: {}", i, err).into()
                    })?;
                execs.push(rule);
            }
            Some(execs)
        } else {
            None
        };
        let else_exec = if let Some(else_exec) = options.else_exec {
            let else_exec = else_exec.into_list();
            let mut else_execs = Vec::with_capacity(else_exec.len());
            for (i, options) in else_exec.into_iter().enumerate() {
                let rule = super::ExecItemRule::new(options)
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("create match-and-rule: else-exec[{}] failed: {}", i, err).into()
                    })?;
                else_execs.push(rule);
            }
            Some(else_execs)
        } else {
            None
        };
        Ok(Self {
            logical: Logical::And,
            matchers,
            exec: execs,
            else_exec,
        })
    }

    pub(super) fn new_or(
        options: option::MatchOrRuleOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let match_or = options.match_or.into_list();
        let mut matchers = Vec::with_capacity(match_or.len());
        for (i, options) in match_or.into_iter().enumerate() {
            let rule = super::MatchItemRule::new(options)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("create match-or-rule: match-or[{}] failed: {}", i, err).into()
                })?;
            matchers.push(rule);
        }
        let execs = if let Some(exec) = options.exec {
            let exec = exec.into_list();
            let mut execs = Vec::with_capacity(exec.len());
            for (i, options) in exec.into_iter().enumerate() {
                let rule = super::ExecItemRule::new(options)
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("create match-or-rule: exec[{}] failed: {}", i, err).into()
                    })?;
                execs.push(rule);
            }
            Some(execs)
        } else {
            None
        };
        let else_exec = if let Some(else_exec) = options.else_exec {
            let else_exec = else_exec.into_list();
            let mut else_execs = Vec::with_capacity(else_exec.len());
            for (i, options) in else_exec.into_iter().enumerate() {
                let rule = super::ExecItemRule::new(options)
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("create match-or-rule: else-exec[{}] failed: {}", i, err).into()
                    })?;
                else_execs.push(rule);
            }
            Some(else_execs)
        } else {
            None
        };
        Ok(Self {
            logical: Logical::Or,
            matchers,
            exec: execs,
            else_exec,
        })
    }

    async fn check_wrapper(
        &self,
        label: &str,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        for (i, rule) in self.matchers.iter().enumerate() {
            if let Err(e) = rule.check(manager).await {
                return Err(format!(
                    "check match-{}-rule: match-{}[{}] failed: {}",
                    label, label, i, e
                )
                .into());
            }
        }
        if let Some(execs) = &self.exec {
            for (i, rule) in execs.iter().enumerate() {
                if let Err(e) = rule.check(manager).await {
                    return Err(
                        format!("check match-{}-rule: exec[{}] failed: {}", label, i, e).into(),
                    );
                }
            }
        }
        if let Some(else_execs) = &self.else_exec {
            for (i, rule) in else_execs.iter().enumerate() {
                if let Err(e) = rule.check(manager).await {
                    return Err(format!(
                        "check match-{}-rule: else-exec[{}] failed: {}",
                        label, i, e
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub(super) async fn check(
        &self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self.logical {
            Logical::And => self.check_wrapper("and", manager).await,
            Logical::Or => self.check_wrapper("or", manager).await,
        }
    }

    async fn run_wrapper(
        &self,
        label: &str,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        let mut matched_num = 0;
        for (i, rule) in self.matchers.iter().enumerate() {
            debug!(
                logger,
                { tracker = ctx.log_tracker() },
                "match match-{}-rule: match-{}[{}]",
                label,
                label,
                i
            );
            match rule.r#match(logger, ctx).await {
                Ok(b) => match self.logical {
                    Logical::And => {
                        if b {
                            matched_num += 1;
                            continue;
                        }
                        matched_num = 0;
                        debug!(
                            logger,
                            { tracker = ctx.log_tracker() },
                            "match match-{}-rule: match-{}[{}] not matched, stop",
                            label,
                            label,
                            i
                        );
                        break;
                    }
                    Logical::Or => {
                        if b {
                            matched_num += self.matchers.len();
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "match match-{}-rule: match-{}[{}] not matched, stop",
                                label,
                                label,
                                i
                            );
                            break;
                        }
                    }
                },
                Err(e) => {
                    error!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "match match-{}-rule: match-{}[{}] failed: {}",
                        label,
                        label,
                        i,
                        e
                    );
                    return Err(e);
                }
            }
        }
        if matched_num == self.matchers.len() {
            if let Some(execs) = &self.exec {
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "match-{}-rule: run exec rule",
                    label
                );
                for (i, rule) in execs.iter().enumerate() {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "execute match-{}-rule: exec[{}]",
                        label,
                        i
                    );
                    match rule.execute(logger, ctx).await {
                        Ok(r) => {
                            if let adapter::ReturnMode::Continue = r {
                                continue;
                            }
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "execute match-{}-rule: exec[{}], return: {:?}",
                                label,
                                i,
                                r
                            );
                            return Ok(r);
                        }
                        Err(e) => {
                            error!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "execute match-{}-rule: exec[{}] failed: {}",
                                label,
                                i,
                                e
                            );
                            return Err(e);
                        }
                    }
                }
            } else {
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "match-{}-rule: exec rule not found, continue",
                    label
                );
                return Ok(adapter::ReturnMode::Continue);
            }
        } else {
            if let Some(else_execs) = &self.else_exec {
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "match-{}-rule: run else-exec rule",
                    label
                );
                for (i, rule) in else_execs.iter().enumerate() {
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "execute match-{}-rule: else-exec[{}]",
                        label,
                        i
                    );
                    match rule.execute(logger, ctx).await {
                        Ok(r) => {
                            if let adapter::ReturnMode::Continue = r {
                                continue;
                            }
                            debug!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "execute match-{}-rule: else-exec[{}], return: {:?}",
                                label,
                                i,
                                r
                            );
                            return Ok(r);
                        }
                        Err(e) => {
                            error!(
                                logger,
                                { tracker = ctx.log_tracker() },
                                "execute match-{}-rule: else-exec[{}] failed: {}",
                                label,
                                i,
                                e
                            );
                            return Err(e);
                        }
                    }
                }
            } else {
                debug!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "match-{}-rule: else-exec rule not found, continue",
                    label
                );
                return Ok(adapter::ReturnMode::Continue);
            }
        }
        Ok(adapter::ReturnMode::Continue)
    }

    pub(super) async fn run(
        &self,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        match self.logical {
            Logical::And => self.run_wrapper("and", logger, ctx).await,
            Logical::Or => self.run_wrapper("or", logger, ctx).await,
        }
    }
}

pub(super) struct ExecRule {
    exec: Vec<super::ExecItemRule>,
}

impl ExecRule {
    pub(super) fn new(
        options: option::ExecRuleOptions,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let exec = options.exec.into_list();
        let mut execs = Vec::with_capacity(exec.len());
        for (i, options) in exec.into_iter().enumerate() {
            let rule = super::ExecItemRule::new(options)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("create exec-rule: exec[{}] failed: {}", i, err).into()
                })?;
            execs.push(rule);
        }
        Ok(Self { exec: execs })
    }

    pub(super) async fn check(
        &self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        for (i, rule) in self.exec.iter().enumerate() {
            if let Err(e) = rule.check(manager).await {
                return Err(format!("check exec-rule: exec[{}] failed: {}", i, e).into());
            }
        }
        Ok(())
    }

    pub(super) async fn run(
        &self,
        logger: &Arc<Box<dyn log::Logger>>,
        ctx: &mut adapter::Context,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        for (i, rule) in self.exec.iter().enumerate() {
            debug!(
                logger,
                { tracker = ctx.log_tracker() },
                "execute exec-rule: exec[{}]",
                i
            );
            match rule.execute(logger, ctx).await {
                Ok(r) => {
                    if let adapter::ReturnMode::Continue = r {
                        continue;
                    }
                    debug!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "execute exec-rule: exec[{}], return: {:?}",
                        i,
                        r
                    );
                    return Ok(r);
                }
                Err(e) => {
                    error!(
                        logger,
                        { tracker = ctx.log_tracker() },
                        "execute exec-rule: exec[{}] failed: {}",
                        i,
                        e
                    );
                    return Err(e);
                }
            }
        }
        Ok(adapter::ReturnMode::Continue)
    }
}

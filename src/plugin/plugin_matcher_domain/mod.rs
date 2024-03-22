use std::{collections::HashMap, error::Error, str::FromStr, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::{
    fs,
    io::{self, AsyncBufReadExt},
    sync::RwLock,
};

use crate::{adapter, common, debug, log};

pub(crate) const TYPE: &str = "domain";

#[derive(Serialize, Deserialize)]
struct Options {
    #[serde(default)]
    rule: Option<common::Listable<common::Domain>>,
    #[serde(default)]
    file: Option<common::Listable<String>>,
}

pub(crate) struct Domain {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    inner_domain_matcher: Option<common::DomainMatcher>,
    files: Arc<Option<Vec<String>>>,
    file_domain_matcher: Arc<RwLock<Option<common::DomainMatcher>>>,
}

impl Domain {
    pub(crate) fn new(
        _: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: serde_yaml::Value,
    ) -> Result<Box<dyn adapter::MatcherPlugin>, Box<dyn Error + Send + Sync>> {
        let options =
            Options::deserialize(options).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize options: {}", err).into()
            })?;
        let logger = Arc::new(logger);
        if options.rule.is_none() && options.file.is_none() {
            return Err("missing rule or file".into());
        }
        let inner_domain_matcher = options
            .rule
            .map(|domain| common::DomainMatcher::new(domain.into_list()));

        let s = Self {
            tag,
            logger,
            inner_domain_matcher,
            files: Arc::new(options.file.map(|v| v.into_list())),
            file_domain_matcher: Arc::new(RwLock::new(None)),
        };

        Ok(Box::new(s))
    }

    async fn load_files<S: AsRef<str>>(
        logger: &Arc<Box<dyn log::Logger>>,
        files: &[S],
    ) -> Result<common::DomainMatcher, Box<dyn Error + Send + Sync>> {
        let mut rule_map = HashMap::new();
        for file in files {
            let mut lines = match fs::File::open(file.as_ref()).await {
                Ok(f) => io::BufReader::new(f).lines(),
                Err(e) => {
                    return Err(
                        format!("failed to open file: {}, err: {}", file.as_ref(), e).into(),
                    );
                }
            };
            while let Ok(Some(line)) = lines.next_line().await {
                let s = line.trim();
                if s.is_empty() || s.starts_with('#') {
                    continue;
                }
                let s = s.split_once('#').map(|(s, _)| s.trim_end()).unwrap_or(s);
                if rule_map.contains_key(s) {
                    continue;
                }
                let rule = match common::Domain::from_str(s) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(
                            logger,
                            "failed to parse domain rule: {}, file: {}, err: {}",
                            s,
                            file.as_ref(),
                            e
                        );
                        continue;
                    }
                };
                rule_map.insert(s.to_string(), rule);
            }
        }
        Ok(common::DomainMatcher::new(rule_map.into_values().collect()))
    }
}

#[async_trait::async_trait]
impl adapter::Common for Domain {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(files) = self.files.as_ref() {
            let domain_matcher = Self::load_files(&self.logger, files).await?;
            self.file_domain_matcher
                .write()
                .await
                .replace(domain_matcher);
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::MatcherPlugin for Domain {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(
        &self,
        _: serde_yaml::Value,
    ) -> Result<u16, Box<dyn Error + Send + Sync>> {
        Ok(0)
    }

    async fn r#match(
        &self,
        ctx: &mut adapter::Context,
        _: u16,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let domain = ctx.request().query().unwrap().name().to_string();
        let domain = domain.trim_end_matches('.');
        let inner_domain_matcher = &self.inner_domain_matcher;
        let file_domain_matcher = self.file_domain_matcher.read().await;
        if let Some(inner_domain_matcher) = inner_domain_matcher {
            if inner_domain_matcher.find(&domain) {
                debug!(
                    self.logger,
                    { tracker = ctx.log_tracker() },
                    "domain matched: {}",
                    domain
                );
                return Ok(true);
            }
        }
        if let Some(file_domain_matcher) = file_domain_matcher.as_ref() {
            if file_domain_matcher.find(&domain) {
                debug!(
                    self.logger,
                    { tracker = ctx.log_tracker() },
                    "domain matched: {}",
                    domain
                );
                return Ok(true);
            }
        }
        debug!(
            self.logger,
            { tracker = ctx.log_tracker() },
            "no domain matched",
        );
        Ok(false)
    }

    #[cfg(feature = "api")]
    fn api_router(&self) -> Option<axum::Router> {
        Some(api::APIHandler::new(self).api_router())
    }
}

#[cfg(feature = "api")]
mod api {
    use std::sync::Arc;

    use axum::response::IntoResponse;
    use tokio::sync::RwLock;

    use crate::{common, debug, error, log};

    pub(crate) struct APIHandler {
        logger: Arc<Box<dyn log::Logger>>,
        files: Arc<Option<Vec<String>>>,
        file_domain_matcher: Arc<RwLock<Option<common::DomainMatcher>>>,
    }

    impl APIHandler {
        pub(super) fn new(d: &super::Domain) -> Arc<Self> {
            Arc::new(Self {
                logger: d.logger.clone(),
                files: d.files.clone(),
                file_domain_matcher: d.file_domain_matcher.clone(),
            })
        }

        pub(super) fn api_router(self: Arc<Self>) -> axum::Router {
            axum::Router::new()
                .route("/reload", axum::routing::post(Self::reload))
                .with_state(self)
        }

        // POST /reload
        async fn reload(
            ctx: common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl IntoResponse {
            if let Some(files) = ctx.state.files.as_ref() {
                match super::Domain::load_files(&ctx.state.logger, files).await {
                    Ok(domain_matcher) => {
                        ctx.state
                            .file_domain_matcher
                            .write()
                            .await
                            .replace(domain_matcher);
                        debug!(ctx.state.logger, "reload success");
                    }
                    Err(e) => {
                        error!(ctx.state.logger, "failed to reload: {}", e);
                        return http::StatusCode::INTERNAL_SERVER_ERROR;
                    }
                }
            }
            http::StatusCode::NO_CONTENT
        }
    }
}

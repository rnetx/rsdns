use std::{
    collections::HashMap,
    fs, io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{adapter, error, info, listener, log, option, plugin, upstream, warn, workflow};

#[derive(Clone)]
pub struct Manager {
    manager_logger: Arc<Box<dyn log::Logger>>,
    //
    upstreams: Arc<
        RwLock<(
            Vec<Arc<Box<dyn adapter::Upstream>>>,
            HashMap<String, Arc<Box<dyn adapter::Upstream>>>,
        )>,
    >,
    matcher_plugins: Arc<
        RwLock<(
            Vec<Arc<Box<dyn adapter::MatcherPlugin>>>,
            HashMap<String, Arc<Box<dyn adapter::MatcherPlugin>>>,
        )>,
    >,
    executor_plugins: Arc<
        RwLock<(
            Vec<Arc<Box<dyn adapter::ExecutorPlugin>>>,
            HashMap<String, Arc<Box<dyn adapter::ExecutorPlugin>>>,
        )>,
    >,
    workflows: Arc<
        RwLock<(
            Vec<Arc<Box<dyn adapter::Workflow>>>,
            HashMap<String, Arc<Box<dyn adapter::Workflow>>>,
        )>,
    >,
    listeners: Arc<RwLock<Vec<Arc<Box<dyn adapter::Listener>>>>>,
    //
    #[cfg(feature = "api")]
    api_server: Arc<Mutex<Option<super::APIServer>>>,
    //
    state_map: Arc<state::TypeMap![Send + Sync]>,
    //
    is_running: Arc<AtomicBool>,
    started_notify_token: CancellationToken,
    is_closing: Arc<AtomicBool>,
    failed_message: Arc<Mutex<Option<String>>>,
    failed_call: CancellationToken,
}

impl Manager {
    pub async fn prepare(options: option::Options) -> anyhow::Result<Self> {
        let root_logger = Arc::new(if options.log.disabled {
            log::NopLogger.into_box()
        } else {
            let mut color_enabled = false;
            let output = match options.log.output.as_str() {
                "" | "stdout" => {
                    color_enabled = true;
                    Box::new(io::stdout()) as Box<dyn io::Write + Send + Sync>
                }
                "stderr" => {
                    color_enabled = true;
                    Box::new(io::stderr()) as Box<dyn io::Write + Send + Sync>
                }
                _ => {
                    let f = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&options.log.output)
                        .map_err(|err| anyhow::anyhow!("failed to open log file: {}", err))?;
                    Box::new(f) as Box<dyn io::Write + Send + Sync>
                }
            };
            if options.log.disable_color {
                color_enabled = false;
            }
            log::BasicLogger::new(
                options.log.disable_timestamp,
                options.log.level,
                color_enabled,
                output,
            )
            .into_box()
        });
        let manager = Self {
            manager_logger: Arc::new(
                log::TagLogger::new(root_logger.clone(), "manager".to_owned())
                    .with_color(colored::Color::BrightRed)
                    .into_box(),
            ),
            upstreams: Arc::new(RwLock::new((Vec::new(), HashMap::new()))),
            matcher_plugins: Arc::new(RwLock::new((Vec::new(), HashMap::new()))),
            executor_plugins: Arc::new(RwLock::new((Vec::new(), HashMap::new()))),
            workflows: Arc::new(RwLock::new((Vec::new(), HashMap::new()))),
            listeners: Arc::new(RwLock::new(Vec::new())),

            #[cfg(feature = "api")]
            api_server: Arc::new(Mutex::new(None)),

            state_map: Arc::new(<state::TypeMap![Send + Sync]>::new()),
            is_running: Arc::new(AtomicBool::new(false)),
            started_notify_token: CancellationToken::new(),
            is_closing: Arc::new(AtomicBool::new(false)),
            failed_message: Arc::new(Mutex::new(None)),
            failed_call: CancellationToken::new(),
        };
        // Create Upstream
        {
            if options.upstreams.is_empty() {
                return Err(anyhow::anyhow!("missing upstream"));
            }
            let mut locker = manager.upstreams.write().await;
            for (i, o) in options.upstreams.into_iter().enumerate() {
                if o.tag.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create upstream[{}] failed: missing tag",
                        i
                    ));
                }
                if locker.1.contains_key(&o.tag) {
                    return Err(anyhow::anyhow!(
                        "create upstream[{}] failed: duplicate tag",
                        i
                    ));
                }
                let tag = o.tag.clone();
                let logger = log::TagLogger::new(root_logger.clone(), format!("upstream/{}", tag))
                    .with_color(colored::Color::BrightGreen);
                let u = Arc::new(
                    upstream::new_upstream(
                        manager.clone_abstract_arc_box(),
                        logger.into_box(),
                        tag.clone(),
                        o,
                    )
                    .map_err(|err| {
                        anyhow::anyhow!("create upstream[{}](upstream[{}]) failed: {}", i, tag, err)
                    })?,
                );
                locker.0.push(u.clone());
                locker.1.insert(u.tag().to_string(), u);
            }
            super::upstream_topological_sort(&mut locker.0)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            locker.0.shrink_to_fit();
            locker.1.shrink_to_fit();
        }
        // Create Matcher Plugin
        {
            let mut locker = manager.matcher_plugins.write().await;
            for (i, o) in options.matcher_plugins.into_iter().enumerate() {
                if o.tag.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create matcher-plugin[{}] failed: missing tag",
                        i
                    ));
                }
                if locker.1.contains_key(&o.tag) {
                    return Err(anyhow::anyhow!(
                        "create matcher-plugin[{}] failed: duplicate tag",
                        i
                    ));
                }
                let tag = o.tag.clone();
                if o.r#type.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create matcher-plugin[{}](matcher-plugin[{}]) failed: missing type",
                        i,
                        tag
                    ));
                }
                let logger =
                    log::TagLogger::new(root_logger.clone(), format!("matcher-plugin/{}", tag))
                        .with_color(colored::Color::BrightBlue);
                let p = Arc::new(
                    plugin::new_matcher_plugin(
                        manager.clone_abstract_arc_box(),
                        logger.into_box(),
                        o.tag,
                        o.r#type,
                        o.options.unwrap_or(serde_yaml::Value::Null),
                    )
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "create matcher-plugin[{}](matcher-plugin[{}]) failed: {}",
                            i,
                            tag,
                            err
                        )
                    })?,
                );
                locker.0.push(p.clone());
                locker.1.insert(p.tag().to_string(), p);
            }
            locker.0.shrink_to_fit();
            locker.1.shrink_to_fit();
        }
        // Create Executor Plugin
        {
            let mut locker = manager.executor_plugins.write().await;
            for (i, o) in options.executor_plugins.into_iter().enumerate() {
                if o.tag.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create executor-plugin[{}] failed: missing tag",
                        i
                    ));
                }
                if locker.1.contains_key(&o.tag) {
                    return Err(anyhow::anyhow!(
                        "create executor-plugin[{}] failed: duplicate tag",
                        i
                    ));
                }
                let tag = o.tag.clone();
                if o.r#type.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create executor-plugin[{}](executor-plugin[{}]) failed: missing type",
                        i,
                        tag
                    ));
                }
                let logger =
                    log::TagLogger::new(root_logger.clone(), format!("executor-plugin/{}", tag))
                        .with_color(colored::Color::BrightBlue);
                let p = Arc::new(
                    plugin::new_executor_plugin(
                        manager.clone_abstract_arc_box(),
                        logger.into_box(),
                        o.tag,
                        o.r#type,
                        o.options.unwrap_or(serde_yaml::Value::Null),
                    )
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "create executor-plugin[{}](executor-plugin[{}]) failed: {}",
                            i,
                            tag,
                            err
                        )
                    })?,
                );
                locker.0.push(p.clone());
                locker.1.insert(p.tag().to_string(), p);
            }
            locker.0.shrink_to_fit();
            locker.1.shrink_to_fit();
        }
        // Create Workflow
        {
            if options.workflows.is_empty() {
                return Err(anyhow::anyhow!("missing workflow"));
            }
            let mut locker = manager.workflows.write().await;
            for (i, o) in options.workflows.into_iter().enumerate() {
                if o.tag.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create workflow[{}] failed: missing tag",
                        i
                    ));
                }
                if locker.1.contains_key(&o.tag) {
                    return Err(anyhow::anyhow!(
                        "create workflow[{}] failed: duplicate tag",
                        i
                    ));
                }
                let tag = o.tag.clone();
                let logger = log::TagLogger::new(root_logger.clone(), format!("workflow/{}", tag))
                    .with_color(colored::Color::BrightMagenta);
                let w = Arc::new(
                    workflow::Workflow::new(
                        manager.clone_abstract_arc_box(),
                        logger.into_box(),
                        tag.clone(),
                        o,
                    )
                    .map(|w| Box::new(w) as Box<dyn adapter::Workflow>)
                    .map_err(|err| {
                        anyhow::anyhow!("create workflow[{}](workflow[{}]) failed: {}", i, tag, err)
                    })?,
                );
                locker.0.push(w.clone());
                locker.1.insert(w.tag().to_string(), w);
            }
            locker.0.shrink_to_fit();
            locker.1.shrink_to_fit();
        }
        // Create Listener
        {
            if options.listeners.is_empty() {
                return Err(anyhow::anyhow!("missing listener"));
            }
            let mut l = manager.listeners.write().await;
            for (i, o) in options.listeners.into_iter().enumerate() {
                if o.tag.is_empty() {
                    return Err(anyhow::anyhow!(
                        "create listener[{}] failed: missing tag",
                        i
                    ));
                }
                let tag = o.tag.clone();
                let logger = log::TagLogger::new(root_logger.clone(), format!("listener/{}", tag))
                    .with_color(colored::Color::BrightYellow);
                let li = Arc::new(
                    listener::new_listener(
                        manager.clone_abstract_arc_box(),
                        logger.into_box(),
                        tag.clone(),
                        o,
                    )
                    .map_err(|err| {
                        anyhow::anyhow!("create listener[{}](listener[{}]) failed: {}", i, tag, err)
                    })?,
                );
                l.push(li);
            }
            l.shrink_to_fit();
        }

        #[cfg(feature = "api")]
        {
            // API Server
            if let Some(api_options) = options.api {
                let logger = log::TagLogger::new(root_logger.clone(), format!("api-server"))
                    .with_color(colored::Color::BrightCyan);
                let api_server = super::APIServer::new(
                    manager.clone_abstract_arc_box(),
                    logger.into_box(),
                    api_options,
                );
                manager.api_server.lock().await.replace(api_server);
            }
        }

        //
        Ok(manager)
    }

    fn clone_abstract_arc_box(&self) -> Arc<Box<dyn adapter::Manager>> {
        Arc::new(Box::new(self.clone()) as Box<dyn adapter::Manager>)
    }

    async fn start(&self) -> anyhow::Result<()> {
        #[cfg(feature = "api")]
        {
            // Prepare Upstream
            if self.api_server.lock().await.is_some() {
                self.state_map
                    .set(upstream::UpstreamStatisticDataMap::new());
            }
        }

        // Start Upstream
        {
            let locker = self.upstreams.read().await;
            for u in locker.0.iter() {
                info!(self.manager_logger, "upstream[{}] starting...", u.tag());
                if let Err(e) = u.start().await {
                    error!(
                        self.manager_logger,
                        "upstream[{}] start failed: {}",
                        u.tag(),
                        e
                    );
                    return Err(e);
                }
                info!(self.manager_logger, "upstream[{}] started", u.tag());
            }
        }
        // Start Matcher Plugin
        {
            let locker = self.matcher_plugins.read().await;
            for p in locker.0.iter() {
                info!(
                    self.manager_logger,
                    "matcher-plugin[{}] starting...",
                    p.tag()
                );
                if let Err(e) = p.start().await {
                    error!(
                        self.manager_logger,
                        "matcher-plugin[{}] start failed: {}",
                        p.tag(),
                        e
                    );
                    return Err(e);
                }
                info!(self.manager_logger, "matcher-plugin[{}] started", p.tag());
            }
        }
        // Start Executor Plugin
        {
            let locker = self.executor_plugins.read().await;
            for p in locker.0.iter() {
                info!(
                    self.manager_logger,
                    "executor-plugin[{}] starting...",
                    p.tag()
                );
                if let Err(e) = p.start().await {
                    error!(
                        self.manager_logger,
                        "executor-plugin[{}] start failed: {}",
                        p.tag(),
                        e
                    );
                    return Err(e);
                }
                info!(self.manager_logger, "executor-plugin[{}] started", p.tag());
            }
        }
        // Check Workflow
        {
            let locker = self.workflows.read().await;
            for w in locker.0.iter() {
                info!(self.manager_logger, "workflow[{}] checking...", w.tag());
                if let Err(e) = w.check().await {
                    error!(
                        self.manager_logger,
                        "workflow[{}] check failed: {}",
                        w.tag(),
                        e
                    );
                    return Err(e);
                }
                info!(self.manager_logger, "workflow[{}] checked", w.tag());
            }
        }
        // Start Listener
        {
            let l = self.listeners.read().await;
            for li in l.iter() {
                info!(self.manager_logger, "listener[{}] starting...", li.tag());
                if let Err(e) = li.start().await {
                    error!(
                        self.manager_logger,
                        "listener[{}] start failed: {}",
                        li.tag(),
                        e
                    );
                    return Err(e);
                }
                info!(self.manager_logger, "listener[{}] started", li.tag());
            }
        }

        #[cfg(feature = "api")]
        {
            // Start API Server
            if let Some(api_server) = self.api_server.lock().await.as_ref() {
                api_server.start().await.map_err(|err| {
                    error!(self.manager_logger, "api-server start failed: {}", err);
                    err
                })?;
            }
        }

        Ok(())
    }

    async fn close(&self) {
        #[cfg(feature = "api")]
        {
            // Close API Server
            if let Some(api_server) = self.api_server.lock().await.take() {
                info!(self.manager_logger, "api-server closing...");
                api_server.close().await;
                info!(self.manager_logger, "api-server closed");
            }
        }

        // Close Listener
        {
            let l = self.listeners.read().await;
            for li in l.iter() {
                info!(self.manager_logger, "listener[{}] closing...", li.tag());
                if let Err(e) = li.close().await {
                    error!(
                        self.manager_logger,
                        "listener[{}] close failed: {}",
                        li.tag(),
                        e
                    );
                }
                info!(self.manager_logger, "listener[{}] closed", li.tag());
            }
        }
        // Close Executor Plugin
        {
            let locker = self.executor_plugins.read().await;
            for p in locker.0.iter() {
                info!(
                    self.manager_logger,
                    "executor-plugin[{}] closing...",
                    p.tag()
                );
                if let Err(e) = p.close().await {
                    error!(
                        self.manager_logger,
                        "executor-plugin[{}] close failed: {}",
                        p.tag(),
                        e
                    );
                }
                info!(self.manager_logger, "executor-plugin[{}] closed", p.tag());
            }
        }
        // Close Matcher Plugin
        {
            let locker = self.matcher_plugins.read().await;
            for p in locker.0.iter() {
                info!(
                    self.manager_logger,
                    "matcher-plugin[{}] closing...",
                    p.tag()
                );
                if let Err(e) = p.close().await {
                    error!(
                        self.manager_logger,
                        "matcher-plugin[{}] close failed: {}",
                        p.tag(),
                        e
                    );
                }
                info!(self.manager_logger, "matcher-plugin[{}] closed", p.tag());
            }
        }
        // Close Upstream
        {
            let locker = self.upstreams.read().await;
            for u in locker.0.iter() {
                info!(self.manager_logger, "upstream[{}] closing...", u.tag());
                if let Err(e) = u.close().await {
                    error!(
                        self.manager_logger,
                        "upstream[{}] close failed: {}",
                        u.tag(),
                        e
                    );
                }
                info!(self.manager_logger, "upstream[{}] closed", u.tag());
            }
        }
    }

    pub async fn run(&self, cancel_token: CancellationToken) -> anyhow::Result<()> {
        if self.is_running.swap(true, Ordering::Relaxed) {
            return Err(anyhow::anyhow!("manager is already running"));
        }
        info!(self.manager_logger, "starting...");
        if let Err(e) = self.start().await {
            self.is_running.store(false, Ordering::Relaxed);
            return Err(e);
        }
        // API Server
        // Wait
        info!(self.manager_logger, "started");
        let mut failed_msg = None;
        self.started_notify_token.cancel();
        tokio::select! {
            _ = self.failed_call.cancelled() => {
                failed_msg = self.failed_message.lock().await.clone();
            }
            _ = cancel_token.cancelled() => {
                self.is_closing.store(true, Ordering::Relaxed);
                warn!(self.manager_logger, "request to close...");
            }
        }
        info!(self.manager_logger, "closing...");
        self.close().await;
        info!(self.manager_logger, "closed");
        let res = match failed_msg {
            Some(msg) => Err(anyhow::anyhow!("{}", msg)),
            None => Ok(()),
        };
        self.is_closing.store(false, Ordering::Relaxed);
        res
    }

    pub fn started_notify_token(&self) -> CancellationToken {
        self.started_notify_token.clone()
    }
}

#[async_trait::async_trait]
impl adapter::Manager for Manager {
    async fn fail_to_close(&self, msg: String) {
        if self.is_closing.load(Ordering::Relaxed) {
            return;
        }
        if !self.failed_call.is_cancelled() {
            self.failed_message.lock().await.replace(msg);
            self.failed_call.cancel();
        }
    }

    async fn list_upstream(&self) -> Vec<Arc<Box<dyn adapter::Upstream>>> {
        self.upstreams.read().await.0.clone()
    }

    async fn get_upstream(&self, tag: &str) -> Option<Arc<Box<dyn adapter::Upstream>>> {
        self.upstreams.read().await.1.get(tag).cloned()
    }

    async fn list_workflow(&self) -> Vec<Arc<Box<dyn adapter::Workflow>>> {
        self.workflows.read().await.0.clone()
    }

    async fn get_workflow(&self, tag: &str) -> Option<Arc<Box<dyn adapter::Workflow>>> {
        self.workflows.read().await.1.get(tag).cloned()
    }

    async fn list_matcher_plugin(&self) -> Vec<Arc<Box<dyn adapter::MatcherPlugin>>> {
        self.matcher_plugins.read().await.0.clone()
    }

    async fn get_matcher_plugin(&self, tag: &str) -> Option<Arc<Box<dyn adapter::MatcherPlugin>>> {
        self.matcher_plugins.read().await.1.get(tag).cloned()
    }

    async fn list_executor_plugin(&self) -> Vec<Arc<Box<dyn adapter::ExecutorPlugin>>> {
        self.executor_plugins.read().await.0.clone()
    }

    async fn get_executor_plugin(
        &self,
        tag: &str,
    ) -> Option<Arc<Box<dyn adapter::ExecutorPlugin>>> {
        self.executor_plugins.read().await.1.get(tag).cloned()
    }

    fn get_state_map(&self) -> &state::TypeMap![Send + Sync] {
        &self.state_map
    }

    fn api_enabled(&self) -> bool {
        false
    }
}

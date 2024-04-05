use std::{collections::HashMap, net::IpAddr, path::PathBuf, sync::Arc};

use rand::Rng;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{adapter, debug, log};

pub(crate) const TYPE: &str = "maxminddb";

#[derive(Deserialize)]
struct Options {
    path: String,
    mode: Mode,
}

#[derive(Deserialize, Clone, Copy)]
enum Mode {
    #[serde(rename = "sing-box")]
    SingBox,
    #[serde(rename = "clash-meta")]
    ClashMeta,
    #[serde(rename = "geoip2-country")]
    GeoLite2Country,
}

pub(crate) struct MaxmindDB {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    path: PathBuf,
    mode: Mode,
    args_map: RwLock<HashMap<u16, WorkflowArgs>>,
    reader: Arc<RwLock<Option<maxminddb::Reader<Vec<u8>>>>>,
}

#[serde_with::serde_as]
#[derive(Deserialize, Clone)]
struct WorkflowArgs {
    #[serde(default)]
    mode: WorkflowMode,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    tag: Vec<String>,
}

#[derive(Deserialize, Clone, Copy)]
enum WorkflowMode {
    #[serde(rename = "match-response-ip")]
    MatchResponseIP,
    #[serde(rename = "match-client-ip")]
    MatchClientIP,
}

impl Default for WorkflowMode {
    fn default() -> Self {
        Self::MatchResponseIP
    }
}

impl MaxmindDB {
    pub(crate) fn new(
        _: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: serde_yaml::Value,
    ) -> anyhow::Result<Box<dyn adapter::MatcherPlugin>> {
        let options = Options::deserialize(options)
            .map_err(|err| anyhow::anyhow!("failed to deserialize options: {}", err))?;
        let logger = Arc::new(logger);

        let s = Self {
            tag,
            logger,
            path: PathBuf::from(options.path),
            mode: options.mode,
            args_map: RwLock::new(HashMap::new()),
            reader: Arc::new(RwLock::new(None)),
        };

        Ok(Box::new(s))
    }

    fn match_tag<S: AsRef<str>>(
        reader: &maxminddb::Reader<Vec<u8>>,
        mode: Mode,
        ip: IpAddr,
        tags: &[S],
    ) -> Option<String> {
        match mode {
            Mode::SingBox => {
                if let Ok(s) = reader.lookup::<String>(ip) {
                    for tag in tags {
                        if tag.as_ref() == s {
                            return Some(s);
                        }
                    }
                }
            }
            Mode::ClashMeta => {
                if let Ok(s) = reader.lookup::<Vec<String>>(ip) {
                    for tag in tags {
                        if s.iter().any(|t| t.as_str() == tag.as_ref()) {
                            return Some(tag.as_ref().to_string());
                        }
                    }
                }
            }
            Mode::GeoLite2Country => {
                if let Ok(c) = reader.lookup::<maxminddb::geoip2::Country>(ip) {
                    let code = if let Some(Some(c)) = c.country.map(|c| c.iso_code) {
                        c
                    } else if let Some(Some(c)) = c.registered_country.map(|c| c.iso_code) {
                        c
                    } else if let Some(Some(c)) = c.represented_country.map(|c| c.iso_code) {
                        c
                    } else if let Some(Some(c)) = c.continent.map(|c| c.code) {
                        c
                    } else {
                        ""
                    };
                    if code.len() > 0 {
                        if tags.iter().any(|t| t.as_ref() == code) {
                            return Some(code.to_string());
                        }
                    }
                }
            }
        }
        None
    }
}

#[async_trait::async_trait]
impl adapter::Common for MaxmindDB {
    async fn start(&self) -> anyhow::Result<()> {
        let reader = maxminddb::Reader::open_readfile(&self.path).map_err(|err| {
            anyhow::anyhow!(
                "failed to open maxminddb file: {}, err: {}",
                self.path.to_string_lossy(),
                err
            )
        })?;
        match (reader.metadata.database_type.as_str(), &self.mode) {
            ("sing-geoip", Mode::SingBox) => {}
            ("Meta-geoip0", Mode::ClashMeta) => {}
            ("GeoLite2Country", Mode::GeoLite2Country) => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "unsupported maxminddb database type: {}",
                    reader.metadata.database_type
                ));
            }
        }
        self.reader.write().await.replace(reader);
        Ok(())
    }

    async fn close(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::MatcherPlugin for MaxmindDB {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(&self, args: serde_yaml::Value) -> anyhow::Result<u16> {
        let mut args = WorkflowArgs::deserialize(args)
            .map_err(|err| anyhow::anyhow!("failed to deserialize args: {}", err))?;
        let mut new_tags = Vec::with_capacity(args.tag.len());
        for tag in args.tag {
            if !tag.contains(',') {
                new_tags.push(tag);
            } else {
                tag.split(',')
                    .for_each(|t| new_tags.push(t.trim().to_string()));
            }
        }
        args.tag = new_tags.into();
        let mut args_map = self.args_map.write().await;
        let mut rng = rand::thread_rng();
        loop {
            let args_id = rng.gen::<u16>();
            if !args_map.contains_key(&args_id) {
                args_map.insert(args_id, args);
                return Ok(args_id);
            }
        }
    }

    async fn r#match(&self, ctx: &mut adapter::Context, args_id: u16) -> anyhow::Result<bool> {
        let args = self.args_map.read().await.get(&args_id).cloned().unwrap();
        match args.mode {
            WorkflowMode::MatchResponseIP => {
                if let Some(response) = ctx.response() {
                    let reader_locker = self.reader.read().await;
                    let reader = reader_locker.as_ref().unwrap();
                    for answer in response.answers() {
                        if let Some(data) = answer.data() {
                            if let Some(a) = data.as_a() {
                                if let Some(s) = Self::match_tag(
                                    reader,
                                    self.mode,
                                    IpAddr::V4(a.0.clone()),
                                    &args.tag,
                                ) {
                                    debug!(
                                        self.logger,
                                        { tracker = ctx.log_tracker() },
                                        "[match-response-ip] match-ip => {}, tag: {}",
                                        a.0,
                                        s
                                    );
                                    return Ok(true);
                                }
                            }

                            if let Some(aaaa) = data.as_aaaa() {
                                if let Some(s) = Self::match_tag(
                                    reader,
                                    self.mode,
                                    IpAddr::V6(aaaa.0.clone()),
                                    &args.tag,
                                ) {
                                    debug!(
                                        self.logger,
                                        { tracker = ctx.log_tracker() },
                                        "[match-response-ip] match-ip => {}, tag: {}",
                                        aaaa.0,
                                        s
                                    );
                                    return Ok(true);
                                }
                            }
                        }
                    }
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "[match-response-ip] no match ip"
                    );
                } else {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "[match-response-ip] no response, skip match ip"
                    );
                }
            }
            WorkflowMode::MatchClientIP => {
                let client_ip = ctx.client_ip();
                let reader_locker = self.reader.read().await;
                let reader = reader_locker.as_ref().unwrap();
                if let Some(s) = Self::match_tag(reader, self.mode, client_ip.clone(), &args.tag) {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "[match-client-ip] match-ip => {}; tag: {}",
                        client_ip,
                        s
                    );
                    return Ok(true);
                } else {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "[match-client-ip] no match ip"
                    );
                }
            }
        }
        Ok(false)
    }

    #[cfg(feature = "api")]
    fn api_router(&self) -> Option<axum::Router> {
        Some(api::APIHandler::new(self).api_router())
    }
}

#[cfg(feature = "api")]
mod api {
    use std::{path::PathBuf, sync::Arc};

    use axum::response::IntoResponse;
    use tokio::sync::RwLock;

    use crate::{common, debug, error, log};

    pub(crate) struct APIHandler {
        logger: Arc<Box<dyn log::Logger>>,
        path: PathBuf,
        mode: super::Mode,
        reader: Arc<RwLock<Option<maxminddb::Reader<Vec<u8>>>>>,
    }

    impl APIHandler {
        pub(super) fn new(m: &super::MaxmindDB) -> Arc<Self> {
            Arc::new(Self {
                logger: m.logger.clone(),
                path: m.path.clone(),
                mode: m.mode,
                reader: m.reader.clone(),
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
            match maxminddb::Reader::open_readfile(&ctx.state.path) {
                Ok(reader) => {
                    match (reader.metadata.database_type.as_str(), &ctx.state.mode) {
                        ("sing-geoip", super::Mode::SingBox) => {}
                        ("Meta-geoip0", super::Mode::ClashMeta) => {}
                        ("GeoLite2Country", super::Mode::GeoLite2Country) => {}
                        _ => {
                            error!(
                                ctx.state.logger,
                                "unsupported maxminddb database type: {}",
                                reader.metadata.database_type
                            );
                            return http::StatusCode::INTERNAL_SERVER_ERROR;
                        }
                    }
                    ctx.state.reader.write().await.replace(reader);
                    debug!(
                        ctx.state.logger,
                        "reloaded maxminddb file: {}",
                        ctx.state.path.to_string_lossy()
                    );
                    http::StatusCode::NO_CONTENT
                }
                Err(e) => {
                    error!(
                        ctx.state.logger,
                        "failed to open maxminddb file: {}, err: {}",
                        ctx.state.path.to_string_lossy(),
                        e
                    );
                    http::StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
    }
}

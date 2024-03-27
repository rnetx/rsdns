use std::{collections::HashMap, error::Error, net::IpAddr, str::FromStr, sync::Arc};

use rand::Rng;
use serde::Deserialize;
use tokio::{
    fs,
    io::{self, AsyncBufReadExt},
    sync::RwLock,
};

use crate::{adapter, common, debug, log};

pub(crate) const TYPE: &str = "ip";

#[derive(Deserialize)]
struct Options {
    #[serde(default)]
    rule: common::SingleOrList<String>,
    #[serde(default)]
    file: common::SingleOrList<String>,
}

#[derive(Deserialize, Clone, Default)]
struct WorkflowArgs {
    #[serde(default)]
    mode: WorkflowMode,
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

pub(crate) struct IP {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    inner_rule_reader: Option<maxminddb::Reader<Vec<u8>>>,
    files: Arc<Option<Vec<String>>>,
    file_rule_reader: Arc<RwLock<Option<maxminddb::Reader<Vec<u8>>>>>,
    args_map: RwLock<HashMap<u16, WorkflowArgs>>,
}

impl IP {
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
        if options.rule.len() == 0 && options.file.len() == 0 {
            return Err("missing rule or file".into());
        }
        let inner_rule_reader = if options.rule.len() > 0 {
            let mut ip_map = HashMap::with_capacity(options.rule.len());
            for ip in options.rule.into_list() {
                match ip.parse::<common::IPRange>() {
                    Ok(v) => {
                        ip_map.insert(v, ());
                    }
                    Err(e) => {
                        return Err(format!("failed to parse ip rule: {}, err: {}", ip, e).into());
                    }
                }
            }
            Some(Self::range_to_maxminddb(ip_map.into_keys().collect()))
        } else {
            None
        };

        let s = Self {
            tag,
            logger,
            inner_rule_reader,
            files: Arc::new({
                if options.file.len() > 0 {
                    Some(options.file.into_list())
                } else {
                    None
                }
            }),
            file_rule_reader: Arc::new(RwLock::new(None)),
            args_map: RwLock::new(HashMap::new()),
        };

        Ok(Box::new(s))
    }

    async fn load_files<S: AsRef<str>>(
        logger: &Arc<Box<dyn log::Logger>>,
        files: &[S],
    ) -> Result<maxminddb::Reader<Vec<u8>>, Box<dyn Error + Send + Sync>> {
        let mut ip_map = HashMap::new();
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
                let rule = match common::IPRange::from_str(s) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(
                            logger,
                            "failed to parse ip rule: {}, file: {}, err: {}",
                            s,
                            file.as_ref(),
                            e
                        );
                        continue;
                    }
                };
                ip_map.insert(rule, ());
            }
        }
        Ok(Self::range_to_maxminddb(ip_map.into_keys().collect()))
    }

    fn range_to_maxminddb(range: Vec<common::IPRange>) -> maxminddb::Reader<Vec<u8>> {
        use common::{IPRange, IPv4Range, IPv6Range};
        use maxminddb_writer::{paths::IpAddrWithMask, Database};

        let mut db = Database::default();
        let data = db
            .insert_value(0u8)
            .expect("maxminddb-writer: insert value failed");
        for r in range {
            for r in r.range_to_ip_or_cidr() {
                match r {
                    IPRange::V4(IPv4Range::Single(ip)) => {
                        db.insert_node(IpAddrWithMask::from(ip), data);
                    }
                    IPRange::V4(IPv4Range::CIDR(cidr)) => {
                        db.insert_node(
                            IpAddrWithMask::new(cidr.addr().into(), cidr.prefix_len()),
                            data,
                        );
                    }
                    IPRange::V6(IPv6Range::Single(ip)) => {
                        db.insert_node(IpAddrWithMask::from(ip), data);
                    }
                    IPRange::V6(IPv6Range::CIDR(cidr)) => {
                        db.insert_node(
                            IpAddrWithMask::new(cidr.addr().into(), cidr.prefix_len()),
                            data,
                        );
                    }
                    _ => {}
                }
            }
        }
        let mut buffer = Vec::new();
        db.write_to(&mut buffer)
            .expect("maxminddb-writer: write to buffer failed");
        drop(db);
        buffer.shrink_to_fit();
        let reader =
            maxminddb::Reader::from_source(buffer).expect("maxminddb: read from buffer failed");
        reader
    }
}

#[async_trait::async_trait]
impl adapter::Common for IP {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(files) = self.files.as_ref() {
            let rules = Self::load_files(&self.logger, files).await?;
            self.file_rule_reader.write().await.replace(rules);
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::MatcherPlugin for IP {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(
        &self,
        mut args: serde_yaml::Value,
    ) -> Result<u16, Box<dyn Error + Send + Sync>> {
        if args.as_mapping().map(|m| m.len() == 0).unwrap_or(false) {
            args = serde_yaml::Value::Null;
        }
        let args = if args.is_null() {
            WorkflowArgs::default()
        } else if args.is_string() {
            WorkflowMode::deserialize(args)
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to deserialize args: {}", err).into()
                })
                .map(|mode| WorkflowArgs { mode })?
        } else {
            WorkflowArgs::deserialize(args).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize args: {}", err).into()
            })?
        };
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

    async fn r#match(
        &self,
        ctx: &mut adapter::Context,
        args_id: u16,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let args = self.args_map.read().await.get(&args_id).cloned().unwrap();
        match args.mode {
            WorkflowMode::MatchResponseIP => {
                if let Some(response) = ctx.response() {
                    let file_rule_reader = self.file_rule_reader.read().await;
                    let inner_rule_reader = &self.inner_rule_reader;
                    for answer in response.answers() {
                        if let Some(data) = answer.data() {
                            if let Some(a) = data.as_a() {
                                if let Some(inner_rule_reader) = inner_rule_reader {
                                    if inner_rule_reader.lookup::<u8>(IpAddr::V4(a.0)).is_ok() {
                                        debug!(
                                            self.logger,
                                            { tracker = ctx.log_tracker() },
                                            "[match-response-ip] match ip: {}",
                                            a.0
                                        );
                                        return Ok(true);
                                    }
                                }
                                if let Some(file_rule_reader) = file_rule_reader.as_ref() {
                                    if file_rule_reader.lookup::<u8>(IpAddr::V4(a.0)).is_ok() {
                                        debug!(
                                            self.logger,
                                            { tracker = ctx.log_tracker() },
                                            "[match-response-ip] match ip: {}",
                                            a.0
                                        );
                                        return Ok(true);
                                    }
                                }
                            }
                            if let Some(aaaa) = data.as_aaaa() {
                                if let Some(inner_rule_reader) = inner_rule_reader {
                                    if inner_rule_reader.lookup::<u8>(IpAddr::V6(aaaa.0)).is_ok() {
                                        debug!(
                                            self.logger,
                                            { tracker = ctx.log_tracker() },
                                            "[match-response-ip] match ip: {}",
                                            aaaa.0
                                        );
                                        return Ok(true);
                                    }
                                }
                                if let Some(file_rule_reader) = file_rule_reader.as_ref() {
                                    if file_rule_reader.lookup::<u8>(IpAddr::V6(aaaa.0)).is_ok() {
                                        debug!(
                                            self.logger,
                                            { tracker = ctx.log_tracker() },
                                            "[match-response-ip] match ip: {}",
                                            aaaa.0
                                        );
                                        return Ok(true);
                                    }
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
                let file_rule_reader = self.file_rule_reader.read().await;
                let inner_rule_reader = &self.inner_rule_reader;
                if let Some(inner_rule_reader) = inner_rule_reader {
                    if inner_rule_reader.lookup::<u8>(*client_ip).is_ok() {
                        debug!(
                            self.logger,
                            { tracker = ctx.log_tracker() },
                            "[match-client-ip] match ip: {}",
                            client_ip
                        );
                        return Ok(true);
                    }
                }
                if let Some(file_rule_reader) = file_rule_reader.as_ref() {
                    if file_rule_reader.lookup::<u8>(*client_ip).is_ok() {
                        debug!(
                            self.logger,
                            { tracker = ctx.log_tracker() },
                            "[match-client-ip] match ip: {}",
                            client_ip
                        );
                        return Ok(true);
                    }
                }
                debug!(
                    self.logger,
                    { tracker = ctx.log_tracker() },
                    "[match-client-ip] no match ip"
                );
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
    use std::sync::Arc;

    use axum::response::IntoResponse;
    use tokio::sync::RwLock;

    use crate::{common, debug, error, log};

    pub(crate) struct APIHandler {
        logger: Arc<Box<dyn log::Logger>>,
        files: Arc<Option<Vec<String>>>,
        file_rule_reader: Arc<RwLock<Option<maxminddb::Reader<Vec<u8>>>>>,
    }

    impl APIHandler {
        pub(super) fn new(i: &super::IP) -> Arc<Self> {
            Arc::new(Self {
                logger: i.logger.clone(),
                files: i.files.clone(),
                file_rule_reader: i.file_rule_reader.clone(),
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
                match super::IP::load_files(&ctx.state.logger, files).await {
                    Ok(rules) => {
                        ctx.state.file_rule_reader.write().await.replace(rules);
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

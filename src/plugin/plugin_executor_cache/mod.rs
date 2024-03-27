use std::{collections::HashMap, error::Error, sync::Arc};

use rand::Rng;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{adapter, debug, log};

pub(crate) const TYPE: &str = "cache";

cfg_if::cfg_if! {
    if #[cfg(feature = "plugin-executor-cache-redis")] {
        #[derive( Deserialize)]
        #[serde(tag = "mode")]
        enum Options {
            #[serde(rename = "memory")]
            Memory(memory_cache::Options),

            #[serde(rename = "redis")]
            Redis(redis_cache::Options),
        }
    } else {
        #[derive( Deserialize)]
        #[serde(tag = "mode")]
        enum Options {
            #[serde(rename = "memory")]
            Memory(memory_cache::Options),
        }
    }
}

pub(crate) struct Cache {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    cache: CacheWrapper,
    args_map: RwLock<HashMap<u16, WorkflowArgs>>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(tag = "mode")]
enum WorkflowArgs {
    #[serde(rename = "restore")]
    Restore {
        #[serde(rename = "disable-return-all")]
        #[serde(default)]
        disable_return_all: bool,
    },
    #[serde(rename = "store")]
    Store,
}

enum CacheWrapper {
    Memory(memory_cache::MemoryCache),

    #[cfg(feature = "plugin-executor-cache-redis")]
    Redis(redis_cache::RedisCache),
}

impl Cache {
    pub(crate) fn new(
        _: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: serde_yaml::Value,
    ) -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>> {
        let options =
            Options::deserialize(options).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize options: {}", err).into()
            })?;
        let logger = Arc::new(logger);

        let s = match options {
            Options::Memory(options) => {
                CacheWrapper::Memory(memory_cache::MemoryCache::new(logger.clone(), options))
            }

            #[cfg(feature = "plugin-executor-cache-redis")]
            Options::Redis(options) => {
                CacheWrapper::Redis(redis_cache::RedisCache::new(logger.clone(), options)?)
            }
        };

        Ok(Box::new(Self {
            tag,
            logger,
            cache: s,
            args_map: RwLock::new(HashMap::new()),
        }))
    }
}

#[async_trait::async_trait]
impl adapter::Common for Cache {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        match &self.cache {
            CacheWrapper::Memory(s) => {
                s.start().await?;
            }

            #[cfg(feature = "plugin-executor-cache-redis")]
            CacheWrapper::Redis(s) => {
                s.start().await?;
            }
        }

        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        match &self.cache {
            CacheWrapper::Memory(s) => {
                s.close().await;
            }

            #[cfg(feature = "plugin-executor-cache-redis")]
            CacheWrapper::Redis(_) => {}
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::ExecutorPlugin for Cache {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(
        &self,
        args: serde_yaml::Value,
    ) -> Result<u16, Box<dyn Error + Send + Sync>> {
        let args =
            WorkflowArgs::deserialize(args).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize args: {}", err).into()
            })?;
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

    async fn execute(
        &self,
        ctx: &mut adapter::Context,
        args_id: u16,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        let args = self.args_map.read().await.get(&args_id).cloned().unwrap();

        match args {
            WorkflowArgs::Store => {
                if let Some(mut response) = ctx.response().cloned() {
                    if let Ok(expired_duration) = util::pre_store(&mut response) {
                        match &self.cache {
                            CacheWrapper::Memory(c) => {
                                c.store_response(
                                    ctx.request().query().cloned().unwrap(),
                                    response,
                                    expired_duration,
                                )
                                .await;
                            }

                            #[cfg(feature = "plugin-executor-cache-redis")]
                            CacheWrapper::Redis(c) => {
                                if let Err(e) = c
                                    .store_response(
                                        ctx.request().query().cloned().unwrap(),
                                        response,
                                        expired_duration,
                                    )
                                    .await
                                {
                                    crate::error!(
                                        self.logger,
                                        { tracker = ctx.log_tracker() },
                                        "failed to store response to cache: {}",
                                        e
                                    );
                                }
                            }
                        }

                        debug!(
                            self.logger,
                            { tracker = ctx.log_tracker() },
                            "response stored to cache"
                        );
                    }
                } else {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "response not found, skip store to cache"
                    );
                }
            }
            WorkflowArgs::Restore { disable_return_all } => {
                let query = match ctx.request().query() {
                    Some(v) => v,
                    None => {
                        debug!(
                            self.logger,
                            { tracker = ctx.log_tracker() },
                            "query not found, skip restore from cache"
                        );
                        return Ok(adapter::ReturnMode::Continue);
                    }
                };

                let response = match &self.cache {
                    CacheWrapper::Memory(c) => c.restore_response(query).await,

                    #[cfg(feature = "plugin-executor-cache-redis")]
                    CacheWrapper::Redis(c) => match c.restore_response(query).await {
                        Ok(v) => v,
                        Err(e) => {
                            crate::error!(
                                self.logger,
                                { tracker = ctx.log_tracker() },
                                "failed to restore response from cache: {}",
                                e
                            );
                            return Ok(adapter::ReturnMode::Continue);
                        }
                    },
                };

                if let Some(mut response) = response {
                    if let Ok(_) = util::post_restore(ctx.request(), &mut response) {
                        ctx.replace_response(response);
                        if !disable_return_all {
                            debug!(
                                self.logger,
                                { tracker = ctx.log_tracker() },
                                "response restored from cache, return all"
                            );
                            return Ok(adapter::ReturnMode::ReturnAll);
                        } else {
                            debug!(
                                self.logger,
                                { tracker = ctx.log_tracker() },
                                "response restored from cache"
                            );
                        }
                    }
                } else {
                    debug!(
                        self.logger,
                        { tracker = ctx.log_tracker() },
                        "response not found in cache"
                    );
                }
            }
        }

        Ok(adapter::ReturnMode::Continue)
    }

    #[cfg(feature = "api")]
    fn api_router(&self) -> Option<axum::Router> {
        match &self.cache {
            CacheWrapper::Memory(c) => Some(memory_cache::APIHandler::from(c).api_router()),

            #[cfg(feature = "plugin-executor-cache-redis")]
            CacheWrapper::Redis(c) => Some(redis_cache::APIHandler::from(c).api_router()),
        }
    }
}

mod util {
    use std::{str::FromStr, time::Duration};

    use hickory_proto::{
        op::{Message, Query},
        rr::{DNSClass, Name, RecordType},
    };

    pub(super) fn get_key(query: &Query) -> String {
        format!(
            "{};{};{}",
            query.name(),
            query.query_class(),
            query.query_type()
        )
    }

    pub(super) fn parse_key(s: &str) -> Option<Query> {
        let parts = s.split(';').collect::<Vec<_>>();
        if parts.len() == 3 {
            let name_str = parts[0];
            let query_class_str = parts[1];
            let query_type_str = parts[2];

            let name = Name::from_str(name_str).ok();
            let query_class = DNSClass::from_str(query_class_str).ok();
            let query_type = RecordType::from_str(query_type_str).ok();

            if let (Some(name), Some(query_class), Some(query_type)) =
                (name, query_class, query_type)
            {
                let mut query = Query::new();
                query.set_name(name);
                query.set_query_class(query_class);
                query.set_query_type(query_type);
                return Some(query);
            }
        }
        None
    }

    pub(super) fn pre_store(response: &mut Message) -> Result<Duration, ()> {
        let mut ttl = 0;
        response.answers().iter().for_each(|answer| {
            if ttl == 0 || answer.ttl() < ttl {
                ttl = answer.ttl();
            }
        });
        Ok(Duration::from_secs(ttl as u64))
    }

    pub(super) fn post_restore(request: &Message, response: &mut Message) -> Result<(), ()> {
        response.set_id(request.id());
        Ok(())
    }
}

mod memory_cache {
    use std::{collections::HashMap, error::Error, sync::Arc, time::Duration};

    use chrono::{DateTime, Local};
    use hickory_proto::op::{Message, Query};
    use serde::Deserialize;
    use tokio::{
        fs::{self, File},
        io::{self, AsyncWriteExt},
        sync::{Mutex, Notify, RwLock},
    };

    use crate::{common, debug, error, log};

    #[derive(Deserialize)]
    pub(crate) struct Options {
        #[serde(rename = "max-size")]
        #[serde(default)]
        max_size: Option<usize>,
        #[serde(rename = "dump-file")]
        #[serde(default)]
        dump_file: Option<String>,
        #[serde(rename = "dump-interval")]
        #[serde(default)]
        #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
        dump_interval: Option<Duration>,
    }

    pub(crate) struct MemoryCache {
        logger: Arc<Box<dyn log::Logger>>,
        cache_map: Arc<RwLock<HashMap<Query, (Message, DateTime<Local>)>>>,
        dump_file: Option<String>,
        dump_interval: Option<Duration>,
        dump_notify: Option<Arc<Notify>>,
        canceller: Mutex<Option<common::Canceller>>,
    }

    impl MemoryCache {
        pub(super) fn new(logger: Arc<Box<dyn log::Logger>>, options: Options) -> Self {
            let cache_map = if let Some(size) = options.max_size {
                HashMap::with_capacity(size)
            } else {
                HashMap::new()
            };
            let dump_notify = if options.dump_file.is_some() {
                Some(Arc::new(Notify::new()))
            } else {
                None
            };
            Self {
                logger,
                cache_map: Arc::new(RwLock::new(cache_map)),
                dump_file: options.dump_file,
                dump_interval: options.dump_interval,
                dump_notify,
                canceller: Mutex::new(None),
            }
        }

        async fn write_cache_map(
            cache_map: &HashMap<Query, (Message, DateTime<Local>)>,
            dump_file: &mut File,
        ) -> Result<(), Box<dyn Error + Send + Sync>> {
            let mut data_map = HashMap::with_capacity(cache_map.len());
            for (query, (msg, _)) in cache_map {
                if let Ok(data) = msg.to_vec() {
                    let value =
                        base64::Engine::encode(&base64::prelude::BASE64_STANDARD_NO_PAD, data);
                    let key = super::util::get_key(query);
                    data_map.insert(key, value);
                }
            }
            let data = serde_json::json!(data_map).to_string();
            dump_file.set_len(0).await?;
            dump_file.write_all(data.as_bytes()).await?;
            dump_file.flush().await?;
            Ok(())
        }

        async fn read_cache_map(
            dump_file: &mut File,
        ) -> Result<HashMap<Query, (Message, DateTime<Local>)>, Box<dyn Error + Send + Sync>>
        {
            let mut buf = vec![];
            let mut buf_writer = io::BufWriter::new(&mut buf);
            io::copy(dump_file, &mut buf_writer).await?;
            drop(buf_writer);
            let data_map: HashMap<String, String> = serde_json::from_slice(&buf)?;
            let mut cache_map = HashMap::new();
            for (key, value) in data_map {
                if let Some(query) = super::util::parse_key(&key) {
                    if let Ok(buf) =
                        base64::Engine::decode(&base64::prelude::BASE64_STANDARD_NO_PAD, value)
                    {
                        if let Ok(msg) = Message::from_vec(&buf) {
                            let mut ttl = 0;
                            msg.answers().iter().for_each(|answer| {
                                if ttl == 0 || ttl > answer.ttl() {
                                    ttl = answer.ttl();
                                }
                            });
                            let t = Local::now() + Duration::from_secs(ttl as u64);
                            cache_map.insert(query, (msg, t));
                        }
                    }
                }
            }
            Ok(cache_map)
        }

        async fn dump_handle(
            logger: Arc<Box<dyn log::Logger>>,
            cache_map: Arc<RwLock<HashMap<Query, (Message, DateTime<Local>)>>>,
            mut dump_file: fs::File,
            dump_interval: Option<Duration>,
            dump_notify: Arc<Notify>,
            canceller_guard: common::CancellerGuard,
        ) {
            if let Some(dump_interval) = dump_interval {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(dump_interval) => {
                            let res = {
                                let cache_map = cache_map.read().await;
                                Self::write_cache_map(&cache_map, &mut dump_file).await
                            };
                            match res {
                                Ok(_) => {
                                    debug!(logger, "cache dumped");
                                }
                                Err(e) => {
                                    error!(logger, "failed to dump cache: {}", e);
                                }
                            }
                        }
                        _ = dump_notify.notified() => {
                            let res = {
                                let cache_map = cache_map.read().await;
                                Self::write_cache_map(&cache_map, &mut dump_file).await
                            };
                            match res {
                                Ok(_) => {
                                    debug!(logger, "cache dumped");
                                }
                                Err(e) => {
                                    error!(logger, "failed to dump cache: {}", e);
                                }
                            }
                        }
                        _ = canceller_guard.cancelled() => {
                            break;
                        }
                    }
                }
            } else {
                loop {
                    tokio::select! {
                        _ = dump_notify.notified() => {
                            let res = {
                                let cache_map = cache_map.read().await;
                                Self::write_cache_map(&cache_map, &mut dump_file).await
                            };
                            match res {
                                Ok(_) => {
                                    debug!(logger, "cache dumped");
                                }
                                Err(e) => {
                                    error!(logger, "failed to dump cache: {}", e);
                                }
                            }
                        }
                        _ = canceller_guard.cancelled() => {
                            break;
                        }
                    }
                }
            }
        }

        async fn check_handle(
            cache_map: Arc<RwLock<HashMap<Query, (Message, DateTime<Local>)>>>,
            canceller_guard: common::CancellerGuard,
        ) {
            const DEFAULT_CLEAN_INTERVAL: Duration = Duration::from_secs(60); // 1min

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(DEFAULT_CLEAN_INTERVAL) => {
                        let mut cache_map = cache_map.write().await;
                        let now = Local::now();
                        cache_map.retain(|_, (_, t)| *t > now);
                    }
                    _ = canceller_guard.cancelled() => {
                        break;
                    }
                }
            }
        }

        pub(super) async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
            let (canceller, canceller_guard) = common::new_canceller();
            if let Some(dump_file) = &self.dump_file {
                let mut dump_file = fs::OpenOptions::new()
                    .append(false)
                    .create(true)
                    .open(dump_file)
                    .await
                    .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                        format!("failed to open dump file: {}", err).into()
                    })?;
                if let Ok(cache_map) = Self::read_cache_map(&mut dump_file).await {
                    *self.cache_map.write().await = cache_map;
                }
                let canceller_guard = canceller_guard.clone();
                let cache_map = self.cache_map.clone();
                let logger = self.logger.clone();
                let dump_interval = self.dump_interval;
                let dump_notify = self.dump_notify.clone().unwrap();
                tokio::spawn(async move {
                    Self::dump_handle(
                        logger,
                        cache_map,
                        dump_file,
                        dump_interval,
                        dump_notify,
                        canceller_guard,
                    )
                    .await
                });
            }
            let cache_map = self.cache_map.clone();
            tokio::spawn(async move { Self::check_handle(cache_map, canceller_guard).await });
            self.canceller.lock().await.replace(canceller);
            Ok(())
        }

        pub(super) async fn close(&self) {
            if let Some(mut canceller) = self.canceller.lock().await.take() {
                canceller.cancel_and_wait().await;
            }
        }

        pub(super) async fn restore_response(&self, query: &Query) -> Option<Message> {
            let cache_map = self.cache_map.read().await;
            cache_map.get(query).cloned().map(|(response, _)| response)
        }

        pub(super) async fn store_response(
            &self,
            query: Query,
            response: Message,
            expired_duration: Duration,
        ) {
            {
                let mut cache_map = self.cache_map.write().await;
                cache_map.insert(query, (response, Local::now() + expired_duration));
            }
            if let Some(dump_notify) = &self.dump_notify {
                dump_notify.notify_waiters();
            }
        }
    }

    #[cfg(feature = "api")]
    pub(crate) struct APIHandler {
        logger: Arc<Box<dyn log::Logger>>,
        cache_map: Arc<RwLock<HashMap<Query, (Message, DateTime<Local>)>>>,
        dump_notify: Option<Arc<Notify>>,
    }

    #[cfg(feature = "api")]
    impl APIHandler {
        pub(super) fn from(c: &MemoryCache) -> Arc<Self> {
            Arc::new(Self {
                logger: c.logger.clone(),
                cache_map: c.cache_map.clone(),
                dump_notify: c.dump_notify.clone(),
            })
        }

        pub(super) fn api_router(self: Arc<Self>) -> axum::Router {
            axum::Router::new()
                .route("/flush", axum::routing::delete(Self::flush))
                .route("/size", axum::routing::get(Self::cache_size))
                .route("/dump", axum::routing::post(Self::dump))
                .with_state(self)
        }

        // DELETE /flush
        async fn flush(
            ctx: crate::common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl axum::response::IntoResponse {
            ctx.state.cache_map.write().await.clear();
            crate::debug!(ctx.state.logger, "memory cache flushed");
            http::StatusCode::NO_CONTENT
        }

        // GET /size
        async fn cache_size(
            ctx: crate::common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl axum::response::IntoResponse {
            let size = ctx.state.cache_map.read().await.len();
            crate::common::GenericResponse::new(http::StatusCode::OK, size)
        }

        // POST /dump
        async fn dump(
            ctx: crate::common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl axum::response::IntoResponse {
            if let Some(notify) = &ctx.state.dump_notify {
                notify.notify_waiters();
                http::StatusCode::NO_CONTENT
            } else {
                http::StatusCode::NOT_FOUND
            }
        }
    }
}

#[cfg(feature = "plugin-executor-cache-redis")]
mod redis_cache {
    use std::{
        error::Error,
        net::{IpAddr, SocketAddr},
        sync::Arc,
        time::Duration,
    };

    use hickory_proto::op::{Message, Query};
    use redis::AsyncCommands;
    use serde::Deserialize;
    use tokio::sync::RwLock;

    use crate::log;

    #[derive(Deserialize)]
    pub(crate) struct Options {
        server: String,
        #[serde(default)]
        db: Option<u64>,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
    }

    pub(crate) struct RedisCache {
        logger: Arc<Box<dyn log::Logger>>,
        client: redis::Client,
        connection_manager: Arc<RwLock<Option<redis::aio::ConnectionManager>>>,
    }

    impl RedisCache {
        pub(super) fn new(
            logger: Arc<Box<dyn log::Logger>>,
            options: Options,
        ) -> Result<Self, Box<dyn Error + Send + Sync>> {
            let mut u = String::default();
            {
                let addr = if let Ok(addr) = options.server.parse::<SocketAddr>() {
                    addr.to_string()
                } else if let Ok(addr) = options.server.parse::<IpAddr>() {
                    addr.to_string()
                } else {
                    "".to_owned()
                };
                if addr.len() > 0 {
                    u.push_str("redis://");
                    match (options.username, options.password) {
                        (Some(username), Some(password)) => {
                            u.push_str(&username);
                            u.push(':');
                            u.push_str(&password);
                            u.push('@');
                        }
                        (Some(_), None) => {
                            return Err("missing password".into());
                        }
                        (None, Some(_)) => {
                            return Err("missing username".into());
                        }
                        (None, None) => {}
                    }
                    u.push_str(&addr);
                    if let Some(db) = options.db {
                        u.push('/');
                        u.push_str(&db.to_string());
                    }
                } else {
                    u.push_str("redis+unix://");
                    u.push_str(&options.server);
                    let mut pq = 0;
                    if let Some(db) = options.db {
                        if pq == 0 {
                            u.push('?');
                        }
                        u.push_str("db=");
                        u.push_str(&db.to_string());
                        pq += 1;
                    }
                    match (options.username, options.password) {
                        (Some(username), Some(password)) => {
                            if pq == 0 {
                                u.push('?');
                            } else if pq > 0 {
                                u.push('&');
                            }
                            u.push_str("username=");
                            u.push_str(&username);
                            u.push('&');
                            u.push_str("password=");
                            u.push_str(&password);
                        }
                        (Some(_), None) => {
                            return Err("missing password".into());
                        }
                        (None, Some(_)) => {
                            return Err("missing username".into());
                        }
                        (None, None) => {}
                    }
                }
            }
            let client = redis::Client::open(u).unwrap();
            let connection_manager = Arc::new(RwLock::new(None));
            Ok(Self {
                logger,
                client,
                connection_manager,
            })
        }

        pub(super) async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
            let client = self.client.clone();
            let connection_manager = redis::aio::ConnectionManager::new(client)
                .await
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to create connection manager: {}", err).into()
                })?;
            self.connection_manager
                .write()
                .await
                .replace(connection_manager);
            Ok(())
        }

        pub(super) async fn restore_response(
            &self,
            query: &Query,
        ) -> Result<Option<Message>, Box<dyn Error + Send + Sync>> {
            let key = super::util::get_key(query);
            let mut connection_manager = self.connection_manager.read().await.clone().unwrap();
            let value = connection_manager
                .get::<'_, _, Option<String>>(key)
                .await
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to get response from redis: {}", err).into()
                })?;
            match value {
                Some(value) => {
                    let buf =
                        base64::Engine::decode(&base64::prelude::BASE64_STANDARD_NO_PAD, value)
                            .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                            format!("failed to deserialize response from redis: {}", err).into()
                        })?;
                    let msg = Message::from_vec(&buf).map_err::<Box<dyn Error + Send + Sync>, _>(
                        |err| format!("failed to parse response from redis: {}", err).into(),
                    )?;
                    Ok(Some(msg))
                }
                None => Ok(None),
            }
        }

        pub(super) async fn store_response(
            &self,
            query: Query,
            response: Message,
            expired_duration: Duration,
        ) -> Result<(), Box<dyn Error + Send + Sync>> {
            let key = super::util::get_key(&query);
            let data = response
                .to_vec()
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to serialize response to redis: {}", err).into()
                })?;
            let value = base64::Engine::encode(&base64::prelude::BASE64_STANDARD_NO_PAD, data);
            let mut connection_manager = self.connection_manager.read().await.clone().unwrap();
            connection_manager
                .set_ex(key, value, expired_duration.as_secs())
                .await
                .map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                    format!("failed to set response to redis: {}", err).into()
                })?;
            Ok(())
        }
    }

    #[cfg(feature = "api")]
    pub(crate) struct APIHandler {
        logger: Arc<Box<dyn log::Logger>>,
        connection_manager: Arc<RwLock<Option<redis::aio::ConnectionManager>>>,
    }

    #[cfg(feature = "api")]
    impl APIHandler {
        pub(super) fn from(c: &RedisCache) -> Arc<Self> {
            Arc::new(Self {
                logger: c.logger.clone(),
                connection_manager: c.connection_manager.clone(),
            })
        }

        pub(super) fn api_router(self: Arc<Self>) -> axum::Router {
            axum::Router::new()
                .route("/flush", axum::routing::delete(Self::flush))
                .route("/size", axum::routing::get(Self::cache_size))
                .with_state(self)
        }

        // DELETE /flush
        async fn flush(
            ctx: crate::common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl axum::response::IntoResponse {
            let mut connection_manager = ctx.state.connection_manager.read().await.clone().unwrap();
            match redis::cmd("FLUSHALL")
                .query_async::<_, ()>(&mut connection_manager)
                .await
            {
                Ok(_) => {
                    crate::debug!(ctx.state.logger, "redis cache flushed");
                    http::StatusCode::NO_CONTENT
                }
                Err(e) => {
                    crate::error!(ctx.state.logger, "failed to flush redis: {}", e);
                    http::StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }

        // GET /size
        async fn cache_size(
            ctx: crate::common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl axum::response::IntoResponse {
            let mut connection_manager = ctx.state.connection_manager.read().await.clone().unwrap();
            match redis::cmd("DBSIZE")
                .query_async::<_, usize>(&mut connection_manager)
                .await
            {
                Ok(size) => axum::response::IntoResponse::into_response(
                    crate::common::GenericResponse::new(http::StatusCode::OK, size),
                ),
                Err(e) => {
                    crate::error!(ctx.state.logger, "failed to get redis db size: {}", e);
                    axum::response::IntoResponse::into_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                    )
                }
            }
        }
    }
}

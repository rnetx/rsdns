use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};

use axum::{
    body::Body,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::common;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct UpstreamStatisticEncodeData {
    pub(crate) tag: String,
    #[serde(rename = "type")]
    pub(crate) r#type: String,
    pub(crate) total: u64,
    pub(crate) success: u64,
}

struct UpstreamStatisticData {
    tag: String,
    r#type: &'static str,
    total: AtomicU64,
    success: AtomicU64,
}

pub(crate) struct UpstreamStatisticDataMap {
    map: Arc<RwLock<HashMap<String, UpstreamStatisticData>>>,
    notify: Arc<Notify>,
}

impl UpstreamStatisticDataMap {
    pub(crate) fn new() -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    fn init_data(&self, tag: &str, r#type: &'static str, is_success: bool) -> bool {
        let mut map = self.map.write().unwrap();
        if !map.contains_key(tag) {
            map.insert(
                tag.to_string(),
                UpstreamStatisticData {
                    tag: tag.to_string(),
                    r#type,
                    total: AtomicU64::new(1),
                    success: AtomicU64::new(if is_success { 1 } else { 0 }),
                },
            );
            self.notify.notify_waiters();
            true
        } else {
            false
        }
    }

    pub(super) fn add_record(&self, tag: &str, r#type: &'static str, is_success: bool) {
        if self.init_data(tag, r#type, is_success) {
            return;
        }
        let map = self.map.read().unwrap();
        let data = map.get(tag).unwrap();
        data.total.fetch_add(1, Ordering::Relaxed);
        if is_success {
            data.success.fetch_add(1, Ordering::Relaxed);
        }
        self.notify.notify_waiters();
    }

    pub(crate) fn api_handler(&self) -> Arc<UpstreamAPIHandler> {
        Arc::new(UpstreamAPIHandler {
            map: self.map.clone(),
        })
    }
}

pub(crate) struct UpstreamAPIHandler {
    map: Arc<RwLock<HashMap<String, UpstreamStatisticData>>>,
}

impl UpstreamAPIHandler {
    pub(crate) fn router(self: &Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route("/upstream", get(list_upstreams))
            .route("/upstream/:tag", get(get_upstream))
            .with_state(self.clone())
    }
}

struct GenericResponse<T: Serialize> {
    data: T,
}

impl<T: Serialize> IntoResponse for GenericResponse<T> {
    fn into_response(self) -> Response<Body> {
        let response_content = serde_json::json!({ "data": self.data }).to_string();
        let mut response = Response::new(Body::from(response_content));
        *response.status_mut() = http::StatusCode::OK;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        response
    }
}

// GET /upstream => list all upstreams
async fn list_upstreams(
    ctx: common::GenericStateRequestContext<Arc<UpstreamAPIHandler>, ()>,
) -> impl IntoResponse {
    let upstreams = {
        let map = ctx.state.map.read().unwrap();
        map.values()
            .map(|data| UpstreamStatisticEncodeData {
                tag: data.tag.clone(),
                r#type: data.r#type.to_string(),
                total: data.total.load(Ordering::Relaxed),
                success: data.success.load(Ordering::Relaxed),
            })
            .collect::<Vec<_>>()
    };
    GenericResponse { data: upstreams }
}

// GET /upstream/:tag => get upstream by tag
async fn get_upstream(
    ctx: common::GenericStateRequestContext<Arc<UpstreamAPIHandler>, String>,
) -> impl IntoResponse {
    let data = {
        let map = ctx.state.map.read().unwrap();
        if let Some(path_params) = &ctx.path_params {
            map.get(&path_params.0)
                .map(|data| UpstreamStatisticEncodeData {
                    tag: data.tag.clone(),
                    r#type: data.r#type.to_string(),
                    total: data.total.load(Ordering::Relaxed),
                    success: data.success.load(Ordering::Relaxed),
                })
        } else {
            None
        }
    };
    GenericResponse { data }
}

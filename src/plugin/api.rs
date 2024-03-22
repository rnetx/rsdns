use std::{collections::HashMap, marker::PhantomData, sync::Arc};

use axum::response::IntoResponse;
use tower_service::Service;

use crate::{adapter, common};

pub(crate) struct PluginAPIHandler<T> {
    tags: Vec<String>,
    routers: HashMap<String, axum::Router>,
    _phantom: PhantomData<T>,
}

impl PluginAPIHandler<Arc<Box<dyn adapter::MatcherPlugin>>> {
    pub(crate) fn new_matcher_plugin(list: Vec<Arc<Box<dyn adapter::MatcherPlugin>>>) -> Arc<Self> {
        let mut tags = Vec::with_capacity(list.len());
        let mut routers = HashMap::with_capacity(list.len());
        for p in list {
            if let Some(router) = p.api_router() {
                tags.push(p.tag().to_string());
                routers.insert(p.tag().to_string(), router);
            }
        }
        Arc::new(Self {
            tags,
            routers,
            _phantom: PhantomData,
        })
    }

    pub(crate) fn api_router(self: Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route("/plugin/matcher", axum::routing::get(Self::list_plugin))
            .route(
                "/plugin/matcher/*tag",
                axum::routing::any(Self::route_plugin),
            )
            .with_state(self)
    }

    // GET /plugin/matcher
    async fn list_plugin(
        ctx: common::GenericStateRequestContext<Arc<Self>, ()>,
    ) -> impl IntoResponse {
        common::GenericResponse::new(http::StatusCode::OK, ctx.state.tags.clone())
    }

    // Any /plugin/matcher/*tag
    async fn route_plugin(
        ctx: common::GenericStateRequestContext<Arc<Self>, String>,
    ) -> impl IntoResponse {
        let tag = match ctx.path_params {
            Some(v) => v.0,
            None => {
                return common::GenericResponse::new(http::StatusCode::BAD_REQUEST, "missing tag")
                    .into_response()
            }
        };
        let router = match ctx.state.routers.get(&tag) {
            Some(v) => v,
            None => {
                return common::GenericResponse::new(
                    http::StatusCode::NOT_FOUND,
                    "plugin not found",
                )
                .into_response()
            }
        };
        match router.clone().into_service().call(ctx.req).await {
            Ok(v) => v,
            Err(_) => common::GenericResponse::new_empty(http::StatusCode::INTERNAL_SERVER_ERROR)
                .into_response(),
        }
    }
}

impl PluginAPIHandler<Arc<Box<dyn adapter::ExecutorPlugin>>> {
    pub(crate) fn new_executor_plugin(
        list: Vec<Arc<Box<dyn adapter::ExecutorPlugin>>>,
    ) -> Arc<Self> {
        let mut tags = Vec::with_capacity(list.len());
        let mut routers = HashMap::with_capacity(list.len());
        for p in list {
            if let Some(router) = p.api_router() {
                tags.push(p.tag().to_string());
                routers.insert(p.tag().to_string(), router);
            }
        }
        Arc::new(Self {
            tags,
            routers,
            _phantom: PhantomData,
        })
    }

    pub(crate) fn api_router(self: Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route("/plugin/executor", axum::routing::get(Self::list_plugin))
            .route(
                "/plugin/executor/*tag",
                axum::routing::any(Self::route_plugin),
            )
            .with_state(self)
    }

    // GET /plugin/executor
    async fn list_plugin(
        ctx: common::GenericStateRequestContext<Arc<Self>, ()>,
    ) -> impl IntoResponse {
        common::GenericResponse::new(http::StatusCode::OK, ctx.state.tags.clone())
    }

    // Any /plugin/executor/*tag
    async fn route_plugin(
        ctx: common::GenericStateRequestContext<Arc<Self>, String>,
    ) -> impl IntoResponse {
        let tag = match ctx.path_params {
            Some(v) => v.0,
            None => {
                return common::GenericResponse::new(http::StatusCode::BAD_REQUEST, "missing tag")
                    .into_response()
            }
        };
        let router = match ctx.state.routers.get(&tag) {
            Some(v) => v,
            None => {
                return common::GenericResponse::new(
                    http::StatusCode::NOT_FOUND,
                    "plugin not found",
                )
                .into_response()
            }
        };
        match router.clone().into_service().call(ctx.req).await {
            Ok(v) => v,
            Err(_) => common::GenericResponse::new_empty(http::StatusCode::INTERNAL_SERVER_ERROR)
                .into_response(),
        }
    }
}

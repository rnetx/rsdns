use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::Request, response::Response};
use futures_util::Future;
use tokio::{net::TcpListener, sync::Mutex};
use tower_http::auth::{AsyncAuthorizeRequest, AsyncRequireAuthorizationLayer};

use crate::{adapter, common, info, log, option, plugin, upstream};

pub(super) struct APIServer {
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Arc<Box<dyn log::Logger>>,
    listen: SocketAddr,
    secret: Option<String>,
    canceller: Mutex<Option<common::Canceller>>,
}

impl APIServer {
    pub(super) fn new(
        manager: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        options: option::APIServerOptions,
    ) -> Self {
        Self {
            manager,
            logger: Arc::new(logger),
            listen: options.listen,
            secret: options.secret,
            canceller: Mutex::new(None),
        }
    }

    async fn get_router(&self) -> axum::Router {
        let mut router = axum::Router::new();

        // Version
        {
            router = router.route(
                "/api/v1/version",
                axum::routing::get(|| async {
                    let mut data = HashMap::with_capacity(2);
                    data.insert("version", crate::build_info::PKG_VERSION);
                    data.insert("git_commit_id", crate::build_info::SHORT_COMMIT);
                    common::GenericResponse::new(http::StatusCode::OK, data)
                }),
            );
        }

        // Upstream
        if let Some(map) = self
            .manager
            .get_state_map()
            .try_get::<upstream::UpstreamStatisticDataMap>()
        {
            router = router.nest("/api/v1", map.api_handler().router());
        }

        // Matcher Plugin
        {
            router = router.nest(
                "/api/v1",
                plugin::PluginAPIHandler::new_matcher_plugin(
                    self.manager.list_matcher_plugin().await,
                )
                .api_router(),
            );
        }

        // Executor Plugin
        {
            router = router.nest(
                "/api/v1",
                plugin::PluginAPIHandler::new_executor_plugin(
                    self.manager.list_executor_plugin().await,
                )
                .api_router(),
            );
        }

        router = router.fallback(axum::routing::any(
            || async move { http::StatusCode::NOT_FOUND },
        ));

        // Auth
        if let Some(secret) = &self.secret {
            router = router.layer(AsyncRequireAuthorizationLayer::new(AuthMiddleware {
                secret: secret.clone(),
            }))
        }

        router
    }

    pub(super) async fn start(&self) -> anyhow::Result<()> {
        let tcp_listener = TcpListener::bind(&self.listen).await?;
        info!(self.logger, "API server listen on {}", self.listen);
        let router = self.get_router().await;
        let (canceller, canceller_guard) = common::new_canceller();
        tokio::spawn(async move {
            axum::serve(tcp_listener, router.into_make_service())
                .with_graceful_shutdown(canceller_guard.into_cancelled_owned())
                .await
        });
        self.canceller.lock().await.replace(canceller);
        Ok(())
    }

    pub(super) async fn close(&self) {
        if let Some(mut canceller) = self.canceller.lock().await.take() {
            canceller.cancel_and_wait().await;
        }
    }
}

#[derive(Clone)]
pub(crate) struct AuthMiddleware {
    secret: String,
}

impl AsyncAuthorizeRequest<Body> for AuthMiddleware {
    type RequestBody = Body;
    type ResponseBody = Body;
    type Future = std::pin::Pin<
        Box<
            dyn Future<
                    Output = Result<Request<Self::RequestBody>, http::Response<Self::ResponseBody>>,
                > + Send,
        >,
    >;

    fn authorize(&mut self, request: http::Request<Body>) -> Self::Future {
        let secret = self.secret.clone();
        Box::pin(async move {
            if let Some(v) = request.headers().get(http::header::AUTHORIZATION) {
                if let Ok(s) = String::from_utf8(v.as_bytes().to_vec()) {
                    if s.trim_start_matches("Bearer ") == secret.as_str() {
                        return Ok(request);
                    }
                }
            }
            let mut response = Response::new(Body::empty());
            *response.status_mut() = http::StatusCode::UNAUTHORIZED;
            Err(response)
        })
    }
}

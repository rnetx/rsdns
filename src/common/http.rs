use axum::{
    extract::{FromRequest, Path, Request},
    response::{IntoResponse, Response},
    RequestExt,
};

pub(crate) struct GenericStateRequestContext<S, P> {
    pub(crate) state: S,
    pub(crate) req: Request,
    pub(crate) path_params: Option<Path<P>>,
}

#[async_trait::async_trait]
impl<S, P> FromRequest<S> for GenericStateRequestContext<S, P>
where
    S: Clone + Send + Sync + 'static,
    P: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = ();

    async fn from_request(mut req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let path_params = req.extract_parts::<Path<P>>().await.ok();
        Ok(Self {
            state: state.clone(),
            req,
            path_params,
        })
    }
}

pub(crate) struct GenericResponse<T = ()> {
    pub(crate) status: http::StatusCode,
    pub(crate) data: T,
}

impl From<http::StatusCode> for GenericResponse {
    fn from(status: http::StatusCode) -> Self {
        Self { status, data: () }
    }
}

impl<T> GenericResponse<T> {
    pub(crate) fn new(status: http::StatusCode, data: T) -> Self {
        Self { status, data }
    }
}

impl GenericResponse<serde_yaml::Value> {
    pub(crate) fn new_empty(status: http::StatusCode) -> Self {
        Self {
            status,
            data: serde_yaml::Value::Null,
        }
    }
}

impl<T: serde::Serialize> IntoResponse for GenericResponse<T> {
    fn into_response(self) -> Response {
        let mut response = Response::new(
            serde_json::json!({
                "data": self.data,
            })
            .to_string()
            .into(),
        );
        *response.status_mut() = self.status;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        response
    }
}

pub mod api;
pub mod listener;
pub mod log;
pub mod plugin;
pub mod upstream;
pub mod workflow;

pub use api::*;
pub use listener::*;
pub use log::*;
pub use plugin::*;
pub use upstream::*;
pub use workflow::*;

use serde::Deserialize;

#[serde_with::serde_as]
#[derive(Default, Debug, Clone, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub log: LogOptions,
    #[serde(default)]
    pub api: Option<APIServerOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    pub upstreams: Vec<UpstreamOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    pub listeners: Vec<ListenerOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(rename = "matcher-plugins")]
    #[serde(default)]
    pub matcher_plugins: Vec<PluginOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(rename = "executor-plugins")]
    #[serde(default)]
    pub executor_plugins: Vec<PluginOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    pub workflows: Vec<WorkflowOptions>,
}

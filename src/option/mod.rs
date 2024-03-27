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

use crate::common;

#[derive(Default, Debug, Clone, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub log: LogOptions,
    #[serde(default)]
    pub api: Option<APIServerOptions>,
    #[serde(default)]
    pub upstreams: common::SingleOrList<UpstreamOptions>,
    #[serde(default)]
    pub listeners: common::SingleOrList<ListenerOptions>,
    #[serde(rename = "matcher-plugins")]
    #[serde(default)]
    pub matcher_plugins: common::SingleOrList<PluginOptions>,
    #[serde(rename = "executor-plugins")]
    #[serde(default)]
    pub executor_plugins: common::SingleOrList<PluginOptions>,
    #[serde(default)]
    pub workflows: common::SingleOrList<WorkflowOptions>,
}

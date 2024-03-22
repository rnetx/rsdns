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

use serde::{Deserialize, Serialize};

use crate::common;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub log: LogOptions,
    #[serde(default)]
    pub api: Option<APIServerOptions>,
    #[serde(default)]
    pub upstreams: common::Listable<UpstreamOptions>,
    #[serde(default)]
    pub listeners: common::Listable<ListenerOptions>,
    #[serde(rename = "matcher-plugins")]
    #[serde(default)]
    pub matcher_plugins: common::Listable<PluginOptions>,
    #[serde(rename = "executor-plugins")]
    #[serde(default)]
    pub executor_plugins: common::Listable<PluginOptions>,
    #[serde(default)]
    pub workflows: common::Listable<WorkflowOptions>,
}

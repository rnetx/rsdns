mod plugin;

#[cfg(feature = "api")]
mod api;

pub use plugin::*;

#[cfg(feature = "api")]
pub(crate) use api::*;

// Plugin

#[cfg(feature = "plugin-matcher-domain")]
mod plugin_matcher_domain;

#[cfg(feature = "plugin-matcher-ip")]
mod plugin_matcher_ip;

#[cfg(feature = "plugin-matcher-maxminddb")]
mod plugin_matcher_maxminddb;

#[cfg(feature = "plugin-matcher-script")]
mod plugin_matcher_script;

#[cfg(feature = "plugin-executor-cache")]
mod plugin_executor_cache;

#[cfg(feature = "plugin-executor-prefer")]
mod plugin_executor_prefer;

#[cfg(all(
    feature = "plugin-executor-ipset",
    any(target_os = "linux", target_os = "android")
))]
mod plugin_executor_ipset;

#[cfg(feature = "plugin-executor-script")]
mod plugin_executor_script;

use serde::Deserialize;

use crate::{common, log};

#[derive(Default, Debug, Clone, Deserialize)]
pub struct LogOptions {
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    #[serde(deserialize_with = "common::deserialize_with_from_str")]
    pub level: log::Level,
    #[serde(default)]
    #[serde(rename = "disable-timestamp")]
    pub disable_timestamp: bool,
    #[serde(default)]
    #[serde(rename = "disable-color")]
    pub disable_color: bool,
    #[serde(default)]
    pub output: String,
}

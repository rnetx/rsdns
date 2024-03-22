use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct LogOptions {
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    #[serde(rename = "disable-timestamp")]
    pub disable_timestamp: bool,
    #[serde(default)]
    pub output: String,
}

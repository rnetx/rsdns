use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOptions {
    pub tag: String,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(flatten)]
    #[serde(default)]
    pub options: serde_yaml::Value,
}

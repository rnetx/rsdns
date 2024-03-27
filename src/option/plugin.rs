use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginOptions {
    pub tag: String,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(flatten)]
    #[serde(default)]
    pub options: Option<serde_yaml::Value>,
}

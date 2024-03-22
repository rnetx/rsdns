use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::common;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOptions {
    pub tag: String,
    pub rules: common::Listable<WorkflowRuleOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowRuleOptions {
    MatchAnd(MatchAndRuleOptions),
    MatchOr(MatchOrRuleOptions),
    Exec(ExecRuleOptions),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchAndRuleOptions {
    #[serde(rename = "match-and")]
    pub match_and: common::Listable<super::MatchItemRuleOptions>,
    #[serde(default)]
    pub exec: Option<common::Listable<super::ExecItemRuleOptions>>,
    #[serde(default)]
    #[serde(rename = "else-exec")]
    pub else_exec: Option<common::Listable<super::ExecItemRuleOptions>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchOrRuleOptions {
    #[serde(rename = "match-or")]
    pub match_or: common::Listable<super::MatchItemRuleOptions>,
    #[serde(default)]
    pub exec: Option<common::Listable<super::ExecItemRuleOptions>>,
    #[serde(default)]
    #[serde(rename = "else-exec")]
    pub else_exec: Option<common::Listable<super::ExecItemRuleOptions>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRuleOptions {
    pub exec: common::Listable<super::ExecItemRuleOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MatchItemRuleOptions {
    #[serde(rename = "listener")]
    Listener(common::Listable<String>),
    #[serde(rename = "!listener")]
    InvertListener(common::Listable<String>),

    #[serde(rename = "client-ip")]
    ClientIP(common::Listable<String>),
    #[serde(rename = "!client-ip")]
    InvertClientIP(common::Listable<String>),

    #[serde(rename = "qtype")]
    QType(common::Listable<serde_yaml::Value>),
    #[serde(rename = "!qtype")]
    InvertQType(common::Listable<serde_yaml::Value>),

    #[serde(rename = "qname")]
    QName(common::Listable<String>),
    #[serde(rename = "!qname")]
    InvertQName(common::Listable<String>),

    #[serde(rename = "has-resp-msg")]
    HasRespMsg(bool),
    #[serde(rename = "!has-resp-msg")]
    InvertHasRespMsg(bool),

    #[serde(rename = "resp-ip")]
    RespIP(common::Listable<String>),
    #[serde(rename = "!resp-ip")]
    InvertRespIP(common::Listable<String>),

    #[serde(rename = "mark")]
    Mark(common::Listable<i16>),
    #[serde(rename = "!mark")]
    InvertMark(common::Listable<i16>),

    #[serde(rename = "metadata")]
    Metadata(serde_yaml::Mapping), // Must Match All
    #[serde(rename = "!metadata")]
    InvertMetadata(serde_yaml::Mapping),

    #[serde(rename = "env")]
    Env(serde_yaml::Mapping), // Must Match Any
    #[serde(rename = "!env")]
    InvertEnv(serde_yaml::Mapping),

    #[serde(rename = "plugin")]
    Plugin(WorkflowPluginOptions),
    #[serde(rename = "!plugin")]
    InvertPlugin(WorkflowPluginOptions),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecItemRuleOptions {
    #[serde(rename = "mark")]
    Mark(i16),
    #[serde(rename = "metadata")]
    Metadata(serde_yaml::Mapping),
    #[serde(rename = "set-ttl")]
    SetTTL(u32),
    #[serde(rename = "set-resp-ip")]
    SetRespIP(common::Listable<IpAddr>),
    #[serde(rename = "plugin")]
    Plugin(WorkflowPluginOptions),
    #[serde(rename = "upstream")]
    Upstream(String),
    #[serde(rename = "jump-to")]
    JumpTo(common::Listable<String>),
    #[serde(rename = "go-to")]
    GoTo(String),
    #[serde(rename = "clean-resp")]
    CleanResp(serde_yaml::Value),
    #[serde(rename = "return")]
    Return(String),
}

// Plugin

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPluginOptions {
    pub tag: String,
    #[serde(default)]
    pub args: Option<serde_yaml::Value>,
}

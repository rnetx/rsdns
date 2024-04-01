use std::{collections::HashMap, net::IpAddr};

use serde::Deserialize;

use crate::common;

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowOptions {
    pub tag: String,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    pub rules: Vec<WorkflowRuleOptions>,
}

#[derive(Debug, Clone)]
pub enum WorkflowRuleOptions {
    MatchAnd(MatchAndRuleOptions),
    MatchOr(MatchOrRuleOptions),
    Exec(ExecRuleOptions),
}

impl<'de> serde::Deserialize<'de> for WorkflowRuleOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = WorkflowRuleOptions;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("WorkflowRuleOptions")
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let map: serde_yaml::Mapping = serde::de::Deserialize::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;

                let contain_match_and = map.contains_key("match-and");
                let contain_match_or = map.contains_key("match-or");
                let contain_exec = map.contains_key("exec");
                let contain_else_exec = map.contains_key("else-exec");

                match (
                    contain_match_and,
                    contain_match_or,
                    contain_exec,
                    contain_else_exec,
                ) {
                    (true, false, true, false) | (true, false, _, true) => {
                        // match-and
                        MatchAndRuleOptions::deserialize(serde_yaml::Value::Mapping(map))
                            .map(|m| WorkflowRuleOptions::MatchAnd(m))
                            .map_err(|e| serde::de::Error::custom(format!("{}", e)))
                    }
                    (false, true, true, false) | (false, true, _, true) => {
                        // match-or
                        MatchOrRuleOptions::deserialize(serde_yaml::Value::Mapping(map))
                            .map(|m| WorkflowRuleOptions::MatchOr(m))
                            .map_err(|e| serde::de::Error::custom(format!("{}", e)))
                    }
                    (false, false, true, false) => {
                        // exec
                        ExecRuleOptions::deserialize(serde_yaml::Value::Mapping(map))
                            .map(|m| WorkflowRuleOptions::Exec(m))
                            .map_err(|e| serde::de::Error::custom(format!("{}", e)))
                    }
                    _ => Err(serde::de::Error::custom("invalid WorkflowRuleOptions")),
                }
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct MatchAndRuleOptions {
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(rename = "match-and")]
    pub match_and: Vec<MatchItemRuleOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    pub exec: Vec<ExecItemRuleOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    #[serde(rename = "else-exec")]
    pub else_exec: Vec<ExecItemRuleOptions>,
}

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct MatchOrRuleOptions {
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(rename = "match-or")]
    pub match_or: Vec<MatchItemRuleOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    pub exec: Vec<ExecItemRuleOptions>,
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    #[serde(default)]
    #[serde(rename = "else-exec")]
    pub else_exec: Vec<ExecItemRuleOptions>,
}

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct ExecRuleOptions {
    #[serde_as(deserialize_as = "serde_with::OneOrMany<_>")]
    pub exec: Vec<ExecItemRuleOptions>,
}

#[derive(Debug, Clone)]
pub struct MatchItemRuleOptions {
    pub invert: bool,
    pub options: MatchItemWrapperRuleOptions,
}

impl<'de> serde::Deserialize<'de> for MatchItemRuleOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = MatchItemRuleOptions;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("MatchItemRuleOptions")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut key = match map.next_key::<String>()? {
                    Some(key) => key,
                    None => {
                        return Err(serde::de::Error::custom("invalid MatchItemRuleOptions"));
                    }
                };
                let invert = if key.starts_with('~') {
                    key = key[1..].to_string();
                    true
                } else {
                    false
                };
                let v =
                    match key.as_str() {
                        "listener" => MatchItemWrapperRuleOptions::Listener(
                            map.next_value::<common::SingleOrList<String>>()?.into(),
                        ),
                        "client-ip" => MatchItemWrapperRuleOptions::ClientIP(
                            map.next_value::<common::SingleOrList<String>>()?.into(),
                        ),
                        "qtype" => MatchItemWrapperRuleOptions::QType(
                            map.next_value::<common::SingleOrList<serde_yaml::Value>>()?
                                .into(),
                        ),
                        "qname" => MatchItemWrapperRuleOptions::QName(
                            map.next_value::<common::SingleOrList<String>>()?.into(),
                        ),
                        "has-resp-msg" => {
                            MatchItemWrapperRuleOptions::HasRespMsg(map.next_value::<bool>()?)
                        }
                        "resp-ip" => MatchItemWrapperRuleOptions::RespIP(
                            map.next_value::<common::SingleOrList<String>>()?.into(),
                        ),
                        "mark" => MatchItemWrapperRuleOptions::Mark(
                            map.next_value::<common::SingleOrList<i16>>()?.into(),
                        ),
                        "metadata" => MatchItemWrapperRuleOptions::Metadata(
                            map.next_value::<HashMap<String, String>>()?,
                        ),
                        "env" => MatchItemWrapperRuleOptions::Env(
                            map.next_value::<HashMap<String, String>>()?,
                        ),
                        "plugin" => MatchItemWrapperRuleOptions::Plugin(
                            map.next_value::<WorkflowPluginOptions>()?,
                        ),
                        _ => {
                            return Err(serde::de::Error::custom("invalid MatchItemRuleOptions"));
                        }
                    };
                Ok(MatchItemRuleOptions { invert, options: v })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Debug, Clone)]
pub enum MatchItemWrapperRuleOptions {
    Listener(common::SingleOrList<String>),
    ClientIP(common::SingleOrList<String>),
    QType(common::SingleOrList<serde_yaml::Value>),
    QName(common::SingleOrList<String>),
    HasRespMsg(bool),
    RespIP(common::SingleOrList<String>),
    Mark(common::SingleOrList<i16>),
    Metadata(HashMap<String, String>), // Must Match All
    Env(HashMap<String, String>),      // Must Match Any
    Plugin(WorkflowPluginOptions),
}

#[derive(Debug, Clone, Deserialize)]
pub enum ExecItemRuleOptions {
    #[serde(rename = "mark")]
    Mark(i16),
    #[serde(rename = "metadata")]
    Metadata(HashMap<String, String>),
    #[serde(rename = "set-ttl")]
    SetTTL(u32),
    #[serde(rename = "set-resp-ip")]
    SetRespIP(common::SingleOrList<IpAddr>),
    #[serde(rename = "plugin")]
    Plugin(WorkflowPluginOptions),
    #[serde(rename = "upstream")]
    Upstream(String),
    #[serde(rename = "jump-to")]
    JumpTo(common::SingleOrList<String>),
    #[serde(rename = "go-to")]
    GoTo(String),
    #[serde(rename = "clean-resp")]
    CleanResp(serde_yaml::Value),
    #[serde(rename = "return")]
    Return(String),
}

// Plugin

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowPluginOptions {
    pub tag: String,
    #[serde(default)]
    pub args: Option<serde_yaml::Value>,
}

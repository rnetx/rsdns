use std::{collections::HashMap, error::Error, ops::Deref, sync::Arc};

use crate::{adapter, log};

struct MatcherPluginCreator(
    Box<
        dyn Fn(
                Arc<Box<dyn adapter::Manager>>,
                Box<dyn log::Logger>,
                String,
                serde_yaml::Value,
            ) -> Result<Box<dyn adapter::MatcherPlugin>, Box<dyn Error + Send + Sync>>
            + Sync,
    >,
);

impl<F> From<F> for MatcherPluginCreator
where
    F: Fn(
            Arc<Box<dyn adapter::Manager>>,
            Box<dyn log::Logger>,
            String,
            serde_yaml::Value,
        ) -> Result<Box<dyn adapter::MatcherPlugin>, Box<dyn Error + Send + Sync>>
        + Sync
        + 'static,
{
    fn from(value: F) -> Self {
        Self(Box::new(value))
    }
}

impl Deref for MatcherPluginCreator {
    type Target = dyn Fn(
        Arc<Box<dyn adapter::Manager>>,
        Box<dyn log::Logger>,
        String,
        serde_yaml::Value,
    )
        -> Result<Box<dyn adapter::MatcherPlugin>, Box<dyn Error + Send + Sync>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct ExecutorPluginCreator(
    Box<
        dyn Fn(
                Arc<Box<dyn adapter::Manager>>,
                Box<dyn log::Logger>,
                String,
                serde_yaml::Value,
            )
                -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>>
            + Sync,
    >,
);

impl<F> From<F> for ExecutorPluginCreator
where
    F: Fn(
            Arc<Box<dyn adapter::Manager>>,
            Box<dyn log::Logger>,
            String,
            serde_yaml::Value,
        ) -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>>
        + Sync
        + 'static,
{
    fn from(value: F) -> Self {
        Self(Box::new(value))
    }
}

impl Deref for ExecutorPluginCreator {
    type Target = dyn Fn(
        Arc<Box<dyn adapter::Manager>>,
        Box<dyn log::Logger>,
        String,
        serde_yaml::Value,
    )
        -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct PluginMap<T>(HashMap<String, T>);

impl<T> Default for PluginMap<T> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<T> PluginMap<T> {
    fn register<S: Into<String>, C: Into<T>>(&mut self, r#type: S, creator: C) {
        self.0.insert(r#type.into(), creator.into());
    }

    fn get<S: AsRef<str>>(&self, r#type: S) -> Option<&T> {
        self.0.get(r#type.as_ref())
    }

    fn keys(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

lazy_static::lazy_static! {
  static ref MATCHER_CREATOR: PluginMap<MatcherPluginCreator> = {
    let mut m = PluginMap::default();

    #[cfg(feature = "plugin-matcher-domain")]
    {
        use super::plugin_matcher_domain::{Domain, TYPE};
        m.register(TYPE, Domain::new);
    }

    #[cfg(feature = "plugin-matcher-ip")]
    {
        use super::plugin_matcher_ip::{IP, TYPE};
        m.register(TYPE, IP::new);
    }

    #[cfg(feature = "plugin-matcher-maxminddb")]
    {
        use super::plugin_matcher_maxminddb::{MaxmindDB, TYPE};
        m.register(TYPE, MaxmindDB::new);
    }

    #[cfg(feature = "plugin-matcher-script")]
    {
        use super::plugin_matcher_script::{Script, TYPE};
        m.register(TYPE, Script::new);
    }

    m
  };

  static ref EXECUTOR_CREATOR: PluginMap<ExecutorPluginCreator> = {
    let mut m = PluginMap::default();

    #[cfg(feature = "plugin-executor-cache")]
    {
        use super::plugin_executor_cache::{Cache, TYPE};
        m.register(TYPE, Cache::new);
    }

    #[cfg(all(
        feature = "plugin-executor-ipset",
        any(target_os = "linux", target_os = "android")
    ))]
    {
        use super::plugin_executor_ipset::{IPSet, TYPE};
        m.register(TYPE, IPSet::new);
    }

    #[cfg(feature = "plugin-executor-prefer")]
    {
        use super::plugin_executor_prefer::{Prefer, TYPE};
        m.register(TYPE, Prefer::new);
    }

    #[cfg(feature = "plugin-executor-script")]
    {
        use super::plugin_executor_script::{Script, TYPE};
        m.register(TYPE, Script::new);
    }

    m
  };
}

pub(crate) fn new_matcher_plugin(
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Box<dyn log::Logger>,
    tag: String,
    r#type: String,
    options: serde_yaml::Value,
) -> Result<Box<dyn adapter::MatcherPlugin>, Box<dyn Error + Send + Sync>> {
    let creator = MATCHER_CREATOR
        .get(&r#type)
        .ok_or::<Box<dyn Error + Send + Sync>>(
            format!("matcher-plugin type [{}] not found", r#type).into(),
        )?;
    creator(manager, logger, tag, options)
}

pub(crate) fn new_executor_plugin(
    manager: Arc<Box<dyn adapter::Manager>>,
    logger: Box<dyn log::Logger>,
    tag: String,
    r#type: String,
    options: serde_yaml::Value,
) -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>> {
    let creator = EXECUTOR_CREATOR
        .get(&r#type)
        .ok_or::<Box<dyn Error + Send + Sync>>(
            format!("executor-plugin type [{}] not found", r#type).into(),
        )?;
    creator(manager, logger, tag, options)
}

pub fn support_matcher_plugins() -> Vec<String> {
    MATCHER_CREATOR.keys()
}

pub fn support_executor_plugins() -> Vec<String> {
    EXECUTOR_CREATOR.keys()
}

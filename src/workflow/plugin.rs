use std::sync::Arc;

use crate::{adapter, option};

pub(super) struct MatchPlugin {
    pub(super) plugin_tag: Option<String>,
    pub(super) args: Option<serde_yaml::Value>,
    //
    pub(super) plugin: Option<Arc<Box<dyn adapter::MatcherPlugin>>>,
    pub(super) args_id: Option<u16>,
}

impl From<option::WorkflowPluginOptions> for MatchPlugin {
    fn from(value: option::WorkflowPluginOptions) -> Self {
        Self {
            plugin_tag: Some(value.tag),
            args: value.args,
            plugin: None,
            args_id: None,
        }
    }
}

pub(super) struct ExecPlugin {
    pub(super) plugin_tag: Option<String>,
    pub(super) args: Option<serde_yaml::Value>,
    //
    pub(super) plugin: Option<Arc<Box<dyn adapter::ExecutorPlugin>>>,
    pub(super) args_id: Option<u16>,
}

impl From<option::WorkflowPluginOptions> for ExecPlugin {
    fn from(value: option::WorkflowPluginOptions) -> Self {
        Self {
            plugin_tag: Some(value.tag),
            args: value.args,
            plugin: None,
            args_id: None,
        }
    }
}

impl MatchPlugin {
    pub(super) async fn prepare(
        &mut self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> anyhow::Result<()> {
        let tag = self.plugin_tag.take().unwrap();
        let plugin = manager
            .get_matcher_plugin(&tag)
            .await
            .ok_or(anyhow::anyhow!("matcher-plugin [{}] not found", tag))?;
        let args = self.args.take().unwrap_or(serde_yaml::Value::default());
        let args_id = plugin.prepare_workflow_args(args).await.map_err(|err| {
            anyhow::anyhow!("matcher-plugin [{}] check args failed: {}", tag, err)
        })?;
        self.plugin.replace(plugin);
        self.args_id.replace(args_id);
        Ok(())
    }

    pub(super) fn get(&self) -> (Arc<Box<dyn adapter::MatcherPlugin>>, u16) {
        let plugin = self.plugin.clone().unwrap();
        let args_id = self.args_id.unwrap();
        (plugin, args_id)
    }
}

impl ExecPlugin {
    pub(super) async fn prepare(
        &mut self,
        manager: &Arc<Box<dyn adapter::Manager>>,
    ) -> anyhow::Result<()> {
        let tag = self.plugin_tag.take().unwrap();
        let plugin = manager
            .get_executor_plugin(&tag)
            .await
            .ok_or(anyhow::anyhow!("executor-plugin [{}] not found", tag))?;
        let args = self.args.take().unwrap_or(serde_yaml::Value::default());
        let args_id = plugin.prepare_workflow_args(args).await.map_err(|err| {
            anyhow::anyhow!("executor-plugin [{}] check args failed: {}", tag, err)
        })?;
        self.plugin.replace(plugin);
        self.args_id.replace(args_id);
        Ok(())
    }

    pub(super) fn get(&self) -> (Arc<Box<dyn adapter::ExecutorPlugin>>, u16) {
        let plugin = self.plugin.clone().unwrap();
        let args_id = self.args_id.unwrap();
        (plugin, args_id)
    }
}

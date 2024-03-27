use std::{collections::HashMap, error::Error, process::Stdio, sync::Arc};

use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, RwLock},
};

use crate::{adapter, common, error, log};

pub(crate) const TYPE: &str = "script";

#[derive(Deserialize)]
struct Options {
    command: String,
    #[serde(default)]
    args: Option<common::SingleOrList<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(deserialize_with = "common::deserialize_with_option_from_str")]
    #[serde(rename = "stdout-level")]
    stdout_level: Option<log::Level>,
    #[serde(deserialize_with = "common::deserialize_with_option_from_str")]
    #[serde(rename = "stderr-level")]
    stderr_level: Option<log::Level>,
}

pub(crate) struct Script {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    command: String,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    stdout_level: Option<log::Level>,
    stderr_level: Option<log::Level>,
    canceller: Arc<RwLock<Option<(common::Canceller, common::CancellerGuard)>>>,
}

impl Script {
    pub(crate) fn new(
        _: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: serde_yaml::Value,
    ) -> Result<Box<dyn adapter::ExecutorPlugin>, Box<dyn Error + Send + Sync>> {
        let options =
            Options::deserialize(options).map_err::<Box<dyn Error + Send + Sync>, _>(|err| {
                format!("failed to deserialize options: {}", err).into()
            })?;
        let logger = Arc::new(logger);
        let s = Self {
            tag,
            logger,
            command: options.command,
            args: options.args.map(|v| v.into_list()),
            env: options.env,
            stdout_level: options.stdout_level,
            stderr_level: options.stderr_level,
            canceller: Arc::new(RwLock::new(None)),
        };

        Ok(Box::new(s))
    }

    fn build_command(&self, ctx: &adapter::Context) -> Command {
        let mut cmd = Command::new(&self.command);
        if let Some(args) = &self.args {
            cmd.args(args);
        }
        if let Some(env) = &self.env {
            cmd.envs(env);
        }
        {
            cmd.env("LISTENER", ctx.listener());
            cmd.env("CLIENT_IP", ctx.client_ip().to_string());
            cmd.env("MARK", ctx.mark().to_string());
            if let Some(query) = ctx.request().query() {
                cmd.env("QNAME", format!("{}", query.name()).trim_end_matches('.'));
                cmd.env("QTYPE", query.query_type().to_string());
                cmd.env("QCLASS", query.query_class().to_string());
            }
            if let Some(response) = ctx.response() {
                let mut i = 0usize;
                for answer in response.answers() {
                    if let Some(data) = answer.data() {
                        if let Some(a) = data.as_a() {
                            cmd.env(format!("RESP_IP_{}", i + 1), a.0.to_string());
                            i = i + 1;
                        }
                        if let Some(aaaa) = data.as_aaaa() {
                            cmd.env(format!("RESP_IP_{}", i + 1), aaaa.0.to_string());
                            i = i + 1;
                        }
                    }
                }
                cmd.env("RESP_IP_COUNT", i.to_string());
            }
            for (k, v) in ctx.metadata() {
                cmd.env(k, v);
            }
        }
        cmd.kill_on_drop(true);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd
    }

    async fn run_command(
        logger: Arc<Box<dyn log::Logger>>,
        log_tracker: Arc<log::Tracker>,
        stdout_level: Option<log::Level>,
        stderr_level: Option<log::Level>,
        mut cmd: Command,
        canceller_guard: common::CancellerGuard,
    ) {
        let mut child = match cmd.spawn() {
            Ok(v) => v,
            Err(e) => {
                error!(
                    logger,
                    { tracker = log_tracker },
                    "failed to run command: {}",
                    e
                );
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut tag = false;
        let (sender, mut receiver) = mpsc::channel(16);
        enum Output {
            Stdout(String),
            Stderr(String),
        }
        if let Some(stdout) = stdout {
            let sender = sender.clone();
            let canceller_guard = canceller_guard.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                loop {
                    tokio::select! {
                        res = lines.next_line() => {
                            if let Ok(res) = res {
                                if let Some(line) = res {
                                    tokio::select! {
                                        res = sender.send(Output::Stdout(line)) => {
                                          if res.is_err() {
                                            break;
                                          }
                                        }
                                        _ = canceller_guard.cancelled() => {
                                          break;
                                        }
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        _ = canceller_guard.cancelled() => {
                            break;
                        }
                    }
                }
            });
            tag = true;
        }
        if let Some(stderr) = stderr {
            let sender = sender.clone();
            let canceller_guard = canceller_guard.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                loop {
                    tokio::select! {
                        res = lines.next_line() => {
                            if let Ok(res) = res {
                                if let Some(line) = res {
                                    tokio::select! {
                                        res = sender.send(Output::Stderr(line)) => {
                                          if res.is_err() {
                                            break;
                                          }
                                        }
                                        _ = canceller_guard.cancelled() => {
                                          break;
                                        }
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        _ = canceller_guard.cancelled() => {
                            break;
                        }
                    }
                }
            });
            tag = true;
        }
        drop(sender);
        let handle_output = |out: Output| {
            let (msg, label, level) = match out {
                Output::Stdout(msg) => (msg, "stdout", stdout_level),
                Output::Stderr(msg) => (msg, "stderr", stderr_level),
            };
            if let Some(level) = level {
                logger.log_with_tracker(level, &log_tracker, format_args!("{}: {}", label, msg));
            }
        };
        if tag {
            loop {
                tokio::select! {
                  res = receiver.recv() => {
                    if let Some(out) = res {
                      handle_output(out);
                    }
                  }
                  _ = child.wait() => {
                    break;
                  }
                  _ = canceller_guard.cancelled() => {
                    break;
                  }
                }
            }
        } else {
            drop(receiver);
            loop {
                tokio::select! {
                  _ = child.wait() => {
                    break;
                  }
                  _ = canceller_guard.cancelled() => {
                    break;
                  }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for Script {
    async fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let (canceller, canceller_guard) = common::new_canceller();
        *self.canceller.write().await = Some((canceller, canceller_guard));
        Ok(())
    }

    async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some((mut canceller, canceller_guard)) = self.canceller.write().await.take() {
            drop(canceller_guard);
            canceller.cancel_and_wait().await;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::ExecutorPlugin for Script {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(
        &self,
        _: serde_yaml::Value,
    ) -> Result<u16, Box<dyn Error + Send + Sync>> {
        Ok(0)
    }

    async fn execute(
        &self,
        ctx: &mut adapter::Context,
        _: u16,
    ) -> Result<adapter::ReturnMode, Box<dyn Error + Send + Sync>> {
        let cmd = self.build_command(ctx);
        let canceller_guard = self.canceller.read().await.as_ref().unwrap().1.clone();
        let stdout_level = self.stdout_level;
        let stderr_level = self.stderr_level;
        let logger = self.logger.clone();
        let log_tracker = ctx.log_tracker().clone();
        tokio::spawn(async move {
            Self::run_command(
                logger,
                log_tracker,
                stdout_level,
                stderr_level,
                cmd,
                canceller_guard,
            )
            .await;
        });
        Ok(adapter::ReturnMode::Continue)
    }
}

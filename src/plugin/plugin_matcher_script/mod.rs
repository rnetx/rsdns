use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, Mutex},
};
use tokio_util::sync::CancellationToken;

use crate::{adapter, common, debug, error, log};

pub(crate) const TYPE: &str = "script";

const DEFAULT_RUN_INTERVAL: Duration = Duration::from_secs(60);

#[serde_with::serde_as]
#[derive(Deserialize)]
struct Options {
    command: String,
    #[serde(default)]
    #[serde_as(deserialize_as = "Option<serde_with::OneOrMany<_>>")]
    args: Option<Vec<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(deserialize_with = "common::deserialize_with_option_from_str")]
    #[serde(rename = "stdout-level")]
    stdout_level: Option<log::Level>,
    #[serde(deserialize_with = "common::deserialize_with_option_from_str")]
    #[serde(rename = "stderr-level")]
    stderr_level: Option<log::Level>,
    #[serde(default)]
    #[serde(deserialize_with = "duration_str::deserialize_option_duration")]
    #[serde(rename = "run-interval")]
    run_interval: Option<Duration>,
}

#[derive(Clone)]
pub(crate) struct Script {
    tag: String,
    logger: Arc<Box<dyn log::Logger>>,
    command: String,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    stdout_level: Option<log::Level>,
    stderr_level: Option<log::Level>,
    run_interval: Option<Duration>,
    match_flag: Arc<AtomicBool>,
    canceller: Arc<Mutex<Option<common::Canceller>>>,
}

impl Script {
    pub(crate) fn new(
        _: Arc<Box<dyn adapter::Manager>>,
        logger: Box<dyn log::Logger>,
        tag: String,
        options: serde_yaml::Value,
    ) -> anyhow::Result<Box<dyn adapter::MatcherPlugin>> {
        let options = Options::deserialize(options)
            .map_err(|err| anyhow::anyhow!("failed to deserialize options: {}", err))?;
        let logger = Arc::new(logger);
        let s = Self {
            tag,
            logger,
            command: options.command,
            args: options.args,
            env: options.env,
            stdout_level: options.stdout_level,
            stderr_level: options.stderr_level,
            run_interval: options.run_interval,
            match_flag: Arc::new(AtomicBool::new(false)),
            canceller: Arc::new(Mutex::new(None)),
        };

        Ok(Box::new(s))
    }

    fn build_command(&self) -> Command {
        let mut cmd = Command::new(&self.command);
        if let Some(args) = &self.args {
            cmd.args(args);
        }
        if let Some(env) = &self.env {
            cmd.envs(env);
        }
        cmd.kill_on_drop(true);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd
    }

    async fn handle_command(
        logger: Arc<Box<dyn log::Logger>>,
        token: CancellationToken,
        match_flag: Arc<AtomicBool>,
        mut cmd: Command,
        stdout_level: Option<log::Level>,
        stderr_level: Option<log::Level>,
    ) {
        let mut child = match cmd.spawn() {
            Ok(v) => v,
            Err(e) => {
                error!(logger, "failed to run command, error: {}", e);
                return;
            }
        };
        enum Output {
            Stdout(String),
            Stderr(String),
        }
        let (sender, mut receiver) = mpsc::channel(16);
        let mut tag = false;
        if let Some(out) = child.stdout.take() {
            let token_handle = token.clone();
            let sender_handle = sender.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(out).lines();
                loop {
                    tokio::select! {
                        res = lines.next_line() => {
                            if let Ok(res) = res {
                                if let Some(line) = res {
                                    tokio::select! {
                                        res = sender_handle.send(Output::Stdout(line)) => {
                                          if res.is_err() {
                                            break;
                                          }
                                        }
                                        _ = token_handle.cancelled() => {
                                          break;
                                        }
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        _ = token_handle.cancelled() => {
                            break;
                        }
                    }
                }
            });
            tag = true;
        }
        if let Some(out) = child.stderr.take() {
            let token_handle = token.clone();
            let sender_handle = sender.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(out).lines();
                loop {
                    tokio::select! {
                        res = lines.next_line() => {
                            if let Ok(res) = res {
                                if let Some(line) = res {
                                    tokio::select! {
                                        res = sender_handle.send(Output::Stderr(line)) => {
                                          if res.is_err() {
                                            break;
                                          }
                                        }
                                        _ = token_handle.cancelled() => {
                                          break;
                                        }
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        _ = token_handle.cancelled() => {
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
            if msg.contains("rsdns-script:matched") {
                match_flag.store(true, Ordering::Relaxed);
            }
            if let Some(level) = level {
                logger.log(level, format_args!("{}: {}", label, msg));
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
                    token.cancel();
                    break;
                  }
                  _ = token.cancelled() => {
                    child.start_kill().ok();
                    break;
                  }
                }
            }
        } else {
            drop(receiver);
            loop {
                tokio::select! {
                  _ = child.wait() => {
                    token.cancel();
                    break;
                  }
                  _ = token.cancelled() => {
                    child.start_kill().ok();
                    break;
                  }
                }
            }
        }
    }

    async fn handle(self, canceller_guard: common::CancellerGuard) {
        let run_interval = self.run_interval.unwrap_or(DEFAULT_RUN_INTERVAL);
        let token = CancellationToken::new();
        loop {
            tokio::select! {
              _ = tokio::time::sleep(run_interval) => {
                let cmd = self.build_command();
                let token_handle = token.clone();
                let match_flag = self.match_flag.clone();
                let stdout_level = self.stdout_level;
                let stderr_level = self.stderr_level;
                let logger = self.logger.clone();
                tokio::select! {
                  _ = Self::handle_command(logger, token_handle, match_flag, cmd, stdout_level, stderr_level) => {}
                  _ = canceller_guard.cancelled() => {
                    token.cancel();
                    break;
                  }
                }
              }
              _ = canceller_guard.cancelled() => {
                token.cancel();
                break;
              }
            }
        }
    }
}

#[async_trait::async_trait]
impl adapter::Common for Script {
    async fn start(&self) -> anyhow::Result<()> {
        let s = self.clone();
        let (canceller, canceller_guard) = common::new_canceller();
        tokio::spawn(s.handle(canceller_guard));
        *self.canceller.lock().await = Some(canceller);
        Ok(())
    }

    async fn close(&self) -> anyhow::Result<()> {
        if let Some(mut canceller) = self.canceller.lock().await.take() {
            canceller.cancel_and_wait().await;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl adapter::MatcherPlugin for Script {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn r#type(&self) -> &'static str {
        TYPE
    }

    async fn prepare_workflow_args(&self, _: serde_yaml::Value) -> anyhow::Result<u16> {
        Ok(0)
    }

    async fn r#match(&self, _: &mut adapter::Context, _: u16) -> anyhow::Result<bool> {
        let result = self.match_flag.load(Ordering::Relaxed);
        if result {
            debug!(self.logger, "matched");
        } else {
            debug!(self.logger, "no matched");
        }
        Ok(result)
    }

    #[cfg(feature = "api")]
    fn api_router(&self) -> Option<axum::Router> {
        Some(api::APIHandler::new(self).api_router())
    }
}

#[cfg(feature = "api")]
mod api {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use axum::response::IntoResponse;

    use crate::common;

    pub(crate) struct APIHandler {
        match_flag: Arc<AtomicBool>,
    }

    impl APIHandler {
        pub(super) fn new(s: &super::Script) -> Arc<Self> {
            Arc::new(Self {
                match_flag: s.match_flag.clone(),
            })
        }

        pub(super) fn api_router(self: Arc<Self>) -> axum::Router {
            axum::Router::new()
                .route("/", axum::routing::get(Self::get_result))
                .with_state(self)
        }

        // GET /
        async fn get_result(
            ctx: common::GenericStateRequestContext<Arc<Self>, ()>,
        ) -> impl IntoResponse {
            let result = ctx.state.match_flag.load(Ordering::Relaxed);
            common::GenericResponse::new(http::StatusCode::OK, result)
        }
    }
}

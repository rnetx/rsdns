use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use bytes::Buf;
use hickory_proto::op::Message;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

use crate::{error, log};

const DEFAULT_MAX_TASK: usize = 64;
const DEFAULT_RECV_BUFFER_SIZE: usize = 65535;

pub(super) struct PipelineStream<S: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    task_sender: mpsc::Sender<(u16, Vec<u8>, oneshot::Sender<Message>)>,
    token: CancellationToken,
    n: Arc<AtomicUsize>,
    _marker: PhantomData<S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + Sync> Clone for PipelineStream<S> {
    fn clone(&self) -> Self {
        self.n.fetch_add(1, Ordering::Relaxed);
        Self {
            task_sender: self.task_sender.clone(),
            token: self.token.clone(),
            n: self.n.clone(),
            _marker: PhantomData,
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static> PipelineStream<S> {
    pub(super) async fn new(logger: Arc<Box<dyn log::Logger>>, stream: S) -> Self {
        let (task_sender, task_receiver) = mpsc::channel(DEFAULT_MAX_TASK);
        let token = CancellationToken::new();
        let token_cloned = token.clone();
        tokio::spawn(async move {
            Self::stream_handle(logger, stream, token_cloned, task_receiver).await;
        });
        Self {
            task_sender,
            token,
            n: Arc::new(AtomicUsize::new(1)),
            _marker: PhantomData,
        }
    }

    async fn stream_handle(
        logger: Arc<Box<dyn log::Logger>>,
        mut stream: S,
        token: CancellationToken,
        mut task_receiver: mpsc::Receiver<(u16, Vec<u8>, oneshot::Sender<Message>)>,
    ) {
        let mut task_map = HashMap::new();
        let mut buffer = bytes::BytesMut::with_capacity(DEFAULT_RECV_BUFFER_SIZE + 2);
        loop {
            tokio::select! {
              _ = token.cancelled() => {
                token.cancel();
                return;
              }
              res = task_receiver.recv() => {
                if let Some((id, data, sender)) = res {
                  let data_length_bytes = (data.len() as u16).to_be_bytes();
                  let mut data_chain = bytes::Buf::chain(&data_length_bytes[..], data.as_slice());
                  tokio::select! {
                    res = stream.write_all_buf(&mut data_chain) => {
                      if res.is_err() {
                        token.cancel();
                        return;
                      }
                    }
                    _ = token.cancelled() => {
                      token.cancel();
                      return;
                    }
                  }
                  tokio::select! {
                    res = stream.flush() => {
                      if res.is_err() {
                        token.cancel();
                        return;
                      }
                    }
                    _ = token.cancelled() => {
                      token.cancel();
                      return;
                    }
                  }
                  task_map.insert(id, sender);
                }
              }
              res = stream.read_buf(&mut buffer) => {
                match res {
                  Ok(length) => {
                    if length == 0 {
                      token.cancel();
                      return;
                    }
                    let length = buffer.get_u16();
                    let data = buffer.copy_to_bytes(length as usize);
                    if let Ok(message) = Message::from_vec(&data) {
                      let id = message.id();
                      if let Some(sender) = task_map.remove(&id) {
                        let _ = sender.send(message);
                      }
                    };
                  }
                  Err(e) => {
                    error!(logger, "stream read failed: {}", e);
                    token.cancel();
                    return;
                  }
                }
              }
            }
        }
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    pub(super) async fn exchange(&self, request: &Message) -> anyhow::Result<Message> {
        if self.is_cancelled() {
            return Err(anyhow::anyhow!("cancelled"));
        }
        let data = request
            .to_vec()
            .map_err(|err| anyhow::anyhow!("serialize request failed: {}", err))?;
        let (sender, receiver) = oneshot::channel();
        let token = self.token.clone();
        tokio::select! {
          _ = token.cancelled() => {
            return Err(anyhow::anyhow!("cancelled"));
          }
          res = self.task_sender.send((request.id(), data, sender)) => {
            if res.is_err() {
              return Err(anyhow::anyhow!("task send failed"));
            }
          }
        }
        tokio::select! {
          _ = token.cancelled() => {
            return Err(anyhow::anyhow!("cancelled"));
          }
          res = receiver => {
            match res {
              Ok(message) => Ok(message),
              Err(_) => Err(anyhow::anyhow!("message receive failed"))
            }
          }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + Sync> Drop for PipelineStream<S> {
    fn drop(&mut self) {
        if self.n.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.token.cancel();
        }
    }
}

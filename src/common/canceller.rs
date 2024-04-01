use std::ops::Deref;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) struct Canceller {
    token: CancellationToken,
    receiver: mpsc::Receiver<()>,
}

#[derive(Clone)]
pub(crate) struct CancellerGuard {
    token: CancellationToken,
    _sender: mpsc::Sender<()>,
}

impl Canceller {
    pub(crate) async fn cancel_and_wait(&mut self) {
        self.token.cancel();
        self.receiver.recv().await;
    }
}

impl CancellerGuard {
    pub(crate) async fn into_cancelled_owned(self) {
        self.token.cancelled_owned().await;
    }

    pub(crate) fn clone_token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Deref for CancellerGuard {
    type Target = CancellationToken;

    fn deref(&self) -> &Self::Target {
        &self.token
    }
}

pub(crate) fn new_canceller() -> (Canceller, CancellerGuard) {
    let (sender, receiver) = mpsc::channel(1);
    let token = CancellationToken::new();
    let guard = CancellerGuard {
        token: token.clone(),
        _sender: sender,
    };
    let canceller = Canceller { token, receiver };
    (canceller, guard)
}

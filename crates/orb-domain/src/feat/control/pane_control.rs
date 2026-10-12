//! The control server's line to the screen loop: hands it a request and waits
//! for the loop's answer, so changes to what the screen owns happen on the
//! loop's thread.

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use crate::feat::control::change::Change;
use crate::feat::control::protocol::{ApiError, ErrorCode};

/// What an agent asks the screen loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneRequest {
    Change(Change),
}

/// The loop's answer.
pub type PaneReply = Result<(), ApiError>;

/// How long a request waits for the loop.
const LOOP_WAIT: Duration = Duration::from_secs(5);

/// Hands requests to the screen loop and waits for its answer. Cloning shares
/// the loop.
#[derive(Clone)]
pub struct PaneControl {
    send: Arc<dyn Fn(PaneRequest, Sender<PaneReply>) + Send + Sync>,
}

impl PaneControl {
    /// A handle that passes each request, with where to answer it, to `send`.
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(PaneRequest, Sender<PaneReply>) + Send + Sync + 'static,
    {
        Self {
            send: Arc::new(send),
        }
    }

    /// Asks the loop to carry out `request` and waits for its answer.
    ///
    /// # Errors
    ///
    /// Returns the loop's refusal, or `failed` when the loop is gone or takes
    /// longer than five seconds.
    pub fn ask(&self, request: PaneRequest) -> PaneReply {
        let (tx, rx) = mpsc::channel();
        (self.send)(request, tx);
        rx.recv_timeout(LOOP_WAIT).unwrap_or_else(|_| {
            Err(ApiError::new(
                ErrorCode::Failed,
                "orb's screen didn't answer",
            ))
        })
    }
}

impl std::fmt::Debug for PaneControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaneControl")
    }
}

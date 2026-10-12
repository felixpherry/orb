//! The loop's side of the control socket: applies an agent's request on the
//! loop's thread and answers it.

use std::sync::mpsc::Sender;

use orb_domain::feat::control::change;
use orb_domain::feat::control::pane_control::{PaneReply, PaneRequest};

use super::App;

impl App {
    /// Applies an agent's request and answers it. A refusal goes back to the
    /// caller only: nothing reaches the message box, and no key sequence or
    /// toast is dismissed.
    pub(super) fn control(&mut self, request: PaneRequest, reply: &Sender<PaneReply>) {
        let answer = match request {
            PaneRequest::Change(change) => {
                let applied = change::apply(&mut self.state.write(), &change);
                applied.map(|commands| {
                    for command in &commands {
                        self.execute(command);
                    }
                })
            }
        };
        let _ = reply.send(answer);
    }
}

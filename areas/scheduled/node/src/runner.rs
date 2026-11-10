//! Delivery to this node's sessions through Send's guarded path: the text
//! is pasted as one paste, Enter waits for it to settle, and Enter is
//! pressed only if the pane still runs the process the text went to
//! (`parvi_term::engine::delivery`). The prompt never reaches a shell or an
//! argv.
//!
//! The pane is CHROTE's, chosen by `Sessions::active_pane`. A send not under way by its deadline is
//! withdrawn unsent; one under way is waited for a little longer and, if
//! tmux still has not answered, reported unconfirmed, never as a failure
//! that might yet paste.

use std::sync::Arc;
use std::time::Duration;

use parvi_node_kit::BoxFuture;
use parvi_scheduled_contract::Target;
use parvi_term::Sessions;
use parvi_term::engine::{Delivering, SessionInfo};

use crate::service::Runner;

/// How long a send that has started is waited for past its deadline: its
/// paste settles for 1.2 s before Enter.
const STARTED_GRACE: Duration = Duration::from_secs(30);

pub struct SessionsRunner {
    sessions: Arc<dyn Sessions>,
    /// The node's own Unix user: a target naming it is on the node's own
    /// tmux server.
    own: String,
}

impl SessionsRunner {
    pub fn new(sessions: Arc<dyn Sessions>) -> SessionsRunner {
        SessionsRunner {
            sessions,
            own: own_user(),
        }
    }

    /// The session a target names: on its user's server, by name.
    fn find(&self, target: &Target) -> Result<SessionInfo, String> {
        let sessions = &self.sessions;
        let user = target.unix_user.as_str();
        let server = if user.is_empty() || user == self.own {
            None
        } else if sessions.users().iter().any(|u| u == user) {
            Some(user)
        } else {
            return Err(format!(
                "unix user {user:?} is not allowed: this node follows no tmux server for them"
            ));
        };
        sessions
            .list()
            .into_iter()
            .find(|s| s.name == target.session_name && s.user.as_deref() == server)
            .ok_or_else(|| format!("no session {} on this node", target.label()))
    }
}

impl Runner for SessionsRunner {
    fn validate<'a>(&'a self, target: &'a Target) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.find(target).map(|_| ()) })
    }

    fn send<'a>(
        &'a self,
        target: &'a Target,
        prompt: &'a str,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let start = tokio::time::Instant::now();
            let queued = async {
                let session = self.find(target)?;
                let pane = self
                    .sessions
                    .active_pane(&session.id)
                    .await
                    .map_err(|e| format!("{}: {e:#}", target.label()))?;
                let delivering = self
                    .sessions
                    .deliver(pane, prompt.to_string(), true)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                Ok::<_, String>((pane, delivering))
            };
            // Nothing is queued until `deliver` returns, so giving up
            // before then sends nothing.
            let (pane, mut delivering) = tokio::time::timeout(deadline, queued)
                .await
                .map_err(|_| format!("no session answered within {} s", deadline.as_secs()))??;
            let left = deadline.saturating_sub(start.elapsed());
            let local = self.sessions.local_pane(pane);
            settle(&mut delivering, left, STARTED_GRACE)
                .await
                .map_err(|e| format!("pane %{local}: {e}"))?;
            Ok(format!("%{local}"))
        })
    }
}

/// A queued send, settled within `deadline`: one whose turn has not come
/// by then is withdrawn unsent; one under way gets `grace` more, after
/// which it is unconfirmed (CHROTE's word) rather than failed.
async fn settle<D>(d: &mut D, deadline: Duration, grace: Duration) -> Result<(), String>
where
    D: Future<Output = anyhow::Result<()>> + Unpin + Withdraw,
{
    match tokio::time::timeout(deadline, &mut *d).await {
        Ok(done) => return done.map_err(|e| format!("{e:#}")),
        Err(_) if d.withdraw() => {
            return Err(format!(
                "its turn did not come within {} s; withdrawn unsent",
                deadline.as_secs_f32()
            ));
        }
        Err(_) => {}
    }
    match tokio::time::timeout(grace, &mut *d).await {
        Ok(done) => done.map_err(|e| format!("{e:#}")),
        Err(_) => Err(format!(
            "delivery is unconfirmed: the paste began but tmux had not answered after {} s",
            (deadline + grace).as_secs()
        )),
    }
}

/// A queued send that can be withdrawn before its turn.
trait Withdraw {
    /// True if it will never be sent; false if it is under way.
    fn withdraw(&self) -> bool;
}

impl Withdraw for Delivering {
    fn withdraw(&self) -> bool {
        Delivering::withdraw(self)
    }
}

#[cfg(unix)]
pub(crate) fn own_user() -> String {
    // SAFETY: getpwuid returns a pointer into static storage or null; the
    // name is copied out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(not(unix))]
pub(crate) fn own_user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    /// A send that never answers, started or not as told.
    struct Stuck {
        started: bool,
        withdrawn: AtomicBool,
    }

    impl Future for Stuck {
        type Output = anyhow::Result<()>;
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<anyhow::Result<()>> {
            Poll::Pending
        }
    }

    impl Withdraw for Stuck {
        fn withdraw(&self) -> bool {
            self.withdrawn.store(!self.started, Ordering::SeqCst);
            !self.started
        }
    }

    /// Review 13: a send timed out while queued was reported failed and
    /// could paste later.
    #[tokio::test]
    async fn a_late_send_is_withdrawn_or_said_to_be_unconfirmed() {
        let mut waiting = Stuck {
            started: false,
            withdrawn: AtomicBool::new(false),
        };
        let (deadline, grace) = (Duration::from_millis(40), Duration::from_millis(80));
        let e = settle(&mut waiting, deadline, grace).await.unwrap_err();
        assert!(e.contains("withdrawn unsent"), "{e}");
        assert!(waiting.withdrawn.load(Ordering::SeqCst));

        let mut under_way = Stuck {
            started: true,
            withdrawn: AtomicBool::new(false),
        };
        let before = tokio::time::Instant::now();
        let e = settle(&mut under_way, deadline, grace).await.unwrap_err();
        assert!(e.contains("unconfirmed"), "{e}");
        assert!(before.elapsed() >= deadline + grace, "waited out its grace");
    }
}

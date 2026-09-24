//! Process ownership shared by the two CLI-backed ACP adapters.
use std::{process::ExitStatus, sync::Mutex};

use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use super::AcpError;

/// Children come exclusively from the runner's group-owning launcher. Keeping
/// the group guard with the Child covers dropped prompt/shutdown futures too.
struct OwnedChild(Child);

impl OwnedChild {
    fn signal_group(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(pid) = self.0.id().filter(|pid| *pid > 1) {
            if unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), AcpError> {
        let signal = self.signal_group();
        let kill = self.0.start_kill();
        let wait = self.0.wait().await;
        let errors: Vec<_> = [signal.err(), kill.err(), wait.err()]
            .into_iter()
            .flatten()
            .map(|error| error.to_string())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AcpError::Transport(format!(
                "stop adapter process: {}",
                errors.join("; ")
            )))
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.signal_group();
        // The launcher also enables Child::kill_on_drop for portable cleanup.
    }
}

#[derive(Default)]
struct State {
    child: Option<OwnedChild>,
    cancel: CancellationToken,
    active: bool,
}

#[derive(Default)]
pub(super) struct AdapterProcess(Mutex<State>);

/// The prompt owns this guard across every await, including stdin writes and
/// event delivery. Dropping a prompt cannot leave its child in a longer-lived
/// adapter. A rejected overlapping turn never owns or clears the active one.
pub(super) struct AdapterTurn<'a> {
    process: &'a AdapterProcess,
    cancel: CancellationToken,
    owns_turn: bool,
}

impl AdapterTurn<'_> {
    pub(super) fn check_active(&self) -> Result<(), AcpError> {
        if self.owns_turn {
            Ok(())
        } else {
            Err(AcpError::Transport(
                "adapter already has an active prompt".into(),
            ))
        }
    }
}

impl std::ops::Deref for AdapterTurn<'_> {
    type Target = CancellationToken;

    fn deref(&self) -> &Self::Target {
        &self.cancel
    }
}

impl Drop for AdapterTurn<'_> {
    fn drop(&mut self) {
        if self.owns_turn {
            self.cancel.cancel();
            let child = {
                let mut state = self.process.0.lock().unwrap();
                state.active = false;
                state.child.take()
            };
            drop(child);
        }
    }
}

impl AdapterProcess {
    pub(super) fn begin_turn(&self) -> AdapterTurn<'_> {
        let mut state = self.0.lock().unwrap();
        if state.active {
            let cancel = CancellationToken::new();
            cancel.cancel();
            return AdapterTurn {
                process: self,
                cancel,
                owns_turn: false,
            };
        }
        state.active = true;
        state.cancel = CancellationToken::new();
        AdapterTurn {
            process: self,
            cancel: state.cancel.clone(),
            owns_turn: true,
        }
    }

    /// A cancel that arrives while resolving/spawning the executable remains
    /// authoritative when the child is finally registered, before any write.
    pub(super) async fn install(
        &self,
        child: Child,
        cancel: &CancellationToken,
    ) -> Result<(), AcpError> {
        let mut child = OwnedChild(child);
        {
            let mut state = self.0.lock().unwrap();
            if !cancel.is_cancelled() {
                state.child = Some(child);
                return Ok(());
            }
        }
        child.stop().await?;
        Err(AcpError::Transport(
            "adapter turn was cancelled during startup".into(),
        ))
    }

    pub(super) async fn wait(&self, cancel: &CancellationToken) -> Result<ExitStatus, AcpError> {
        // Never hold the state lock while waiting: a process can close stdout
        // before exiting, and cancel must still be able to signal its token.
        let child = self.0.lock().unwrap().child.take();
        let mut child =
            child.ok_or_else(|| AcpError::Transport("adapter turn was cancelled".into()))?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                child.stop().await?;
                Err(AcpError::Transport("adapter turn was cancelled".into()))
            }
            status = child.0.wait() => status.map_err(|error| AcpError::Transport(format!("wait for adapter: {error}"))),
        }
    }

    pub(super) async fn cancel(&self) -> Result<(), AcpError> {
        let child = {
            let mut state = self.0.lock().unwrap();
            state.cancel.cancel();
            state.child.take()
        };
        if let Some(mut child) = child {
            child.stop().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fixture(dir: &std::path::Path) -> Child {
        crate::agents::runner::try_spawn(
            "sh",
            None,
            &["-c".into(), "exec sleep 30".into()],
            dir,
            "",
            None,
            crate::agents::runner::SpawnIo::Adapter,
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn dropping_a_turn_releases_its_registered_process() {
        let dir = tempfile::tempdir().unwrap();
        let process = AdapterProcess::default();
        let turn = process.begin_turn();
        process.install(fixture(dir.path()), &turn).await.unwrap();

        drop(turn);

        assert!(
            process.0.lock().unwrap().child.is_none(),
            "an abandoned prompt must not retain its running child in the adapter"
        );
    }

    #[tokio::test]
    async fn overlapping_turn_is_rejected_without_replacing_the_active_cancellation() {
        let process = AdapterProcess::default();
        let active = process.begin_turn();
        let overlap = process.begin_turn();

        assert!(overlap.is_cancelled(), "one adapter cannot own two prompts");
        assert!(overlap.check_active().is_err());
        assert!(active.check_active().is_ok());
        drop(overlap);
        process.cancel().await.unwrap();
        assert!(
            active.is_cancelled(),
            "cancel must still reach the active turn"
        );
    }

    #[test]
    fn finishing_a_turn_permits_the_next_sequential_turn() {
        let process = AdapterProcess::default();
        let first = process.begin_turn();
        drop(first);
        let next = process.begin_turn();
        assert!(!next.is_cancelled());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn cancellation_before_registration_rejects_and_reaps_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let process = AdapterProcess::default();
        let token = process.begin_turn();
        process.cancel().await.unwrap();
        let result = process.install(fixture(dir.path()), &token).await;
        assert!(
            matches!(result, Err(AcpError::Transport(error)) if error.contains("cancelled during startup"))
        );
        assert!(process.0.lock().unwrap().child.is_none());
        process.cancel().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn cancellation_interrupts_a_wait_that_already_owns_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let process = AdapterProcess::default();
        let token = process.begin_turn();
        process.install(fixture(dir.path()), &token).await.unwrap();
        let waiting = process.wait(&token);
        tokio::pin!(waiting);
        // Poll once so wait owns the child before cancel runs. No clock race.
        assert!(futures::poll!(&mut waiting).is_pending());
        process.cancel().await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
            .await
            .unwrap();
        assert!(matches!(result, Err(AcpError::Transport(error)) if error.contains("cancelled")));
        process.cancel().await.unwrap();
    }
}

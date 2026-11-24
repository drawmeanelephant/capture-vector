//! The only place a process is created. Every child gets its own process
//! group (Unix) or Job object (Windows) so cancellation kills the whole
//! tree, never just the leader.

use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(unix)]
#[path = "proc/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "proc/windows.rs"]
mod platform;

/// Env vars holding live-publish credentials in CI. Children never
/// inherit them — secrets reach Boris over stdin only.
pub const SCRUBBED_ENV: &[&str] = &["CV_NOSTR_NSEC", "CV_BSKY_APP_PASSWORD", "CV_BSKY_HANDLE"];

/// How long a group gets to exit after the polite signal before it is
/// force-killed. Windows has no polite signal for a windowless child.
pub const STOP_GRACE: Duration = Duration::from_millis(1500);

const POLL: Duration = Duration::from_millis(10);

/// Cooperative cancellation for a running verb.
#[derive(Clone, Default, Debug)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Builds a Boris invocation with the crate-wide hygiene applied.
pub(crate) fn boris_command(bin: &Path, cwd: &Path) -> Command {
    // A relative path with a directory part would resolve against `cwd`
    // in the child; anchor it to ours. A bare name still searches PATH.
    let bin = if bin.is_relative() && bin.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        std::path::absolute(bin).unwrap_or_else(|_| bin.to_path_buf())
    } else {
        bin.to_path_buf()
    };
    let mut cmd = Command::new(&bin);
    cmd.current_dir(cwd);
    for k in SCRUBBED_ENV {
        cmd.env_remove(k);
    }
    cmd
}

/// A spawned child that owns its process tree.
pub(crate) struct Spawned {
    child: Child,
    group: platform::Group,
    killed: bool,
    status: Option<ExitStatus>,
}

impl Spawned {
    pub fn spawn(mut cmd: Command, stdin: Stdio) -> Result<Self> {
        cmd.stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::configure(&mut cmd);
        let program = std::path::PathBuf::from(cmd.get_program());
        let mut child = cmd
            .spawn()
            .map_err(|source| EngineError::Spawn { program, source })?;
        let group = match platform::Group::adopt(&child) {
            Ok(g) => g,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Io(e));
            }
        };
        Ok(Self {
            child,
            group,
            killed: false,
            status: None,
        })
    }

    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }
        Ok(self.status)
    }

    pub fn outcome(&self) -> Option<ExitOutcome> {
        self.status
            .map(|s| ExitOutcome::from_status(s, self.killed))
    }

    /// Stop the whole tree: polite signal, grace period, then force.
    /// Idempotent; safe after the leader already exited.
    pub fn kill_tree(&mut self, grace: Duration) {
        if self.status.is_some() && self.killed {
            return;
        }
        let leader_alive = matches!(self.try_wait(), Ok(None));
        if leader_alive {
            self.killed = true;
        }
        self.group.terminate();
        let deadline = Instant::now() + grace;
        while matches!(self.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        self.group.kill();
        if self.status.is_none() {
            self.status = self.child.wait().ok();
        }
    }

    /// Wait for exit, killing the tree if `cancel` fires first.
    pub fn wait_cancellable(&mut self, cancel: &CancelToken) -> Result<ExitOutcome> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(ExitOutcome::from_status(status, self.killed));
            }
            if cancel.is_cancelled() {
                self.kill_tree(Duration::ZERO);
                return Err(EngineError::Cancelled);
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if !matches!(self.try_wait(), Ok(Some(_))) {
            self.kill_tree(Duration::ZERO);
        }
    }
}

/// Drain a pipe to bytes on its own thread (both pipes must be drained
/// concurrently or a chatty child deadlocks on a full pipe buffer).
pub(crate) fn drain<R: Read + Send + 'static>(mut r: R) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

/// Captured result of a one-shot child.
pub(crate) struct Captured {
    pub outcome: ExitOutcome,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Spawn, optionally feed stdin, capture both pipes, honour cancel.
pub(crate) fn run_captured(
    cmd: Command,
    stdin: Option<&[u8]>,
    cancel: &CancelToken,
) -> Result<Captured> {
    let mut child = Spawned::spawn(
        cmd,
        if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        },
    )?;
    let out = drain(child.take_stdout().expect("stdout piped"));
    let err = drain(child.take_stderr().expect("stderr piped"));
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.take_stdin()) {
        use std::io::Write;
        // A child that exits without reading closes the pipe; that is
        // its verdict to report, not ours.
        let _ = pipe.write_all(bytes).and_then(|_| pipe.flush());
        drop(pipe);
    }
    let outcome = child.wait_cancellable(cancel);
    // Tree is dead on cancel, so pipes reach EOF and the drains finish.
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    Ok(Captured {
        outcome: outcome?,
        stdout,
        stderr,
    })
}

/// Test/probe helper: run a shell snippet in the platform shell.
#[cfg(test)]
pub(crate) fn shell_command(script: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(script);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_pipes_and_code() {
        let cmd = shell_command("echo out && echo err 1>&2 && exit 1");
        let c = run_captured(cmd, None, &CancelToken::new()).unwrap();
        assert_eq!(c.outcome, ExitOutcome::Exited { code: 1 });
        assert!(String::from_utf8_lossy(&c.stdout).contains("out"));
        assert!(String::from_utf8_lossy(&c.stderr).contains("err"));
    }

    #[test]
    fn cancel_kills_the_tree() {
        #[cfg(unix)]
        let script = "sleep 30 & sleep 30; wait";
        #[cfg(windows)]
        let script = "ping -n 30 127.0.0.1 >NUL";
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            c2.cancel();
        });
        let start = Instant::now();
        let r = run_captured(shell_command(script), None, &cancel);
        t.join().unwrap();
        assert!(matches!(r, Err(EngineError::Cancelled)));
        // Drains only finish once every holder of the pipes is dead —
        // finishing fast proves the grandchildren died too.
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn scrubbed_env_never_reaches_child() {
        let mut cmd = boris_command(
            Path::new(if cfg!(windows) { "cmd" } else { "sh" }),
            Path::new("."),
        );
        #[cfg(unix)]
        cmd.arg("-c").arg("env");
        #[cfg(windows)]
        cmd.arg("/C").arg("set");
        // Inject on the Command (not the test process) to stay race-free.
        cmd.env("CV_NOSTR_NSEC", "nsec1shouldnotleak");
        for k in SCRUBBED_ENV {
            cmd.env_remove(k);
        }
        let c = run_captured(cmd, None, &CancelToken::new()).unwrap();
        assert!(!String::from_utf8_lossy(&c.stdout).contains("nsec1shouldnotleak"));
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn relative_bin_is_anchored_before_the_child_cwd_changes() {
        let cmd = boris_command(Path::new("zig-out/bin/boris"), Path::new("/elsewhere"));
        assert_eq!(
            Path::new(cmd.get_program()),
            std::env::current_dir().unwrap().join("zig-out/bin/boris")
        );
        let bare = boris_command(Path::new("boris"), Path::new("/elsewhere"));
        assert_eq!(bare.get_program(), "boris");
    }
}

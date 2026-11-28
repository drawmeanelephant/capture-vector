//! The only place a process is created. Every child gets its own process
//! group (Unix) or Job object (Windows) so cancellation kills the whole
//! tree, never just the leader.

use crate::bounded::append;
use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
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
const PIPE_EXIT_GRACE: Duration = Duration::from_secs(5);
/// Retain complete one-shot output up to this many bytes per stream.
const CAPTURE_LIMIT: usize = 8 * 1024 * 1024;

pub use cv_core::engine::CancelToken;

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
    // The child's environment is an allowlist, not "inherit minus
    // credentials": any other secret in the process env (CI tokens,
    // cloud keys) must not flow into the engine.
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        if env_allowed(&k) {
            cmd.env(&k, v);
        }
    }
    cmd
}

/// Env names the engine child may see: process essentials, locale,
/// temp dirs, network egress (publish targets), and the engine's own
/// `BORIS_*` namespace. Compared case-insensitively (Windows env keys
/// vary in case); `LC_*` passes for locale.
const ENV_ALLOW: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "TEMP",
    "TMP",
    "TERM",
    "SYSTEMROOT",
    "COMSPEC",
    "PATHEXT",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

fn env_allowed(k: &std::ffi::OsStr) -> bool {
    let k = k.to_string_lossy();
    ENV_ALLOW.iter().any(|a| k.eq_ignore_ascii_case(a))
        || k.starts_with("LC_")
        || k.starts_with("BORIS_")
}

/// A spawned child that owns its process tree.
pub(crate) struct Spawned {
    child: Child,
    group: platform::Group,
    killed: bool,
    tree_stopped: bool,
    status: Option<ExitStatus>,
}

impl Spawned {
    pub fn spawn(cmd: Command, stdin: Stdio) -> Result<Self> {
        Self::spawn_with_stdout(cmd, stdin, Stdio::piped())
    }

    /// Watch and its capability probe read the protocol on stderr only.
    pub fn spawn_watch(cmd: Command) -> Result<Self> {
        Self::spawn_with_stdout(cmd, Stdio::null(), Stdio::null())
    }

    fn spawn_with_stdout(mut cmd: Command, stdin: Stdio, stdout: Stdio) -> Result<Self> {
        cmd.stdin(stdin).stdout(stdout).stderr(Stdio::piped());
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
        if let Err(e) = platform::resume(&child) {
            group.kill();
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineError::Io(e));
        }
        Ok(Self {
            child,
            group,
            killed: false,
            tree_stopped: false,
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
        if self.tree_stopped {
            return;
        }
        self.tree_stopped = true;
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
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if !matches!(self.try_wait(), Ok(Some(_))) {
            self.kill_tree(Duration::ZERO);
        }
    }
}

/// Read each pipe concurrently. On overflow discard the entire capture,
/// not just the suffix: a byte prefix is not a complete report or bundle.
fn capture<R: Read + Send + 'static>(
    mut r: R,
    stream: &'static str,
    limit: usize,
) -> JoinHandle<Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            let n = match r.read(&mut chunk) {
                Ok(0) => return Ok(captured),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            if !append(&mut captured, &chunk[..n], limit) {
                return Err(EngineError::OutputLimitExceeded { stream, limit });
            }
        }
    })
}

fn take_capture(thread: &mut Option<JoinHandle<Result<Vec<u8>>>>) -> Result<Option<Vec<u8>>> {
    if thread.as_ref().is_some_and(|t| t.is_finished()) {
        let bytes = thread
            .take()
            .expect("reader finished")
            .join()
            .map_err(|_| io::Error::other("child output reader failed"))??;
        Ok(Some(bytes))
    } else {
        Ok(None)
    }
}

/// Captured result of a one-shot child.
pub(crate) struct Captured {
    pub outcome: ExitOutcome,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn drain_captured(
    child: &mut Spawned,
    out: &mut Option<JoinHandle<Result<Vec<u8>>>>,
    err: &mut Option<JoinHandle<Result<Vec<u8>>>>,
    cancel: &CancelToken,
    input_finished: impl Fn() -> bool,
) -> Result<Captured> {
    let mut stdout = None;
    let mut stderr = None;
    let mut exited_at = None;
    loop {
        stdout = take_capture(out)?.or(stdout);
        stderr = take_capture(err)?.or(stderr);
        // Terminal exit is latched before cancellation is consulted:
        // a process observed dead must not have its outcome rewritten
        // to cancellation by a token arriving in the same poll window.
        let exited = child.try_wait()?.is_some();
        if !exited && cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        if exited {
            if out.is_none() && err.is_none() && input_finished() {
                return Ok(Captured {
                    outcome: child.outcome().expect("child exited"),
                    stdout: stdout.expect("stdout captured"),
                    stderr: stderr.expect("stderr captured"),
                });
            }
            // A descendant may still be flushing the pipes, but
            // must not hold them open indefinitely after exit.
            if exited_at.get_or_insert_with(Instant::now).elapsed() >= PIPE_EXIT_GRACE {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child pipes stayed open after exit",
                )
                .into());
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Spawn, optionally feed stdin, capture both pipes, honour cancel.
/// Return only complete captures; overflow fails the run even if the
/// child exited successfully or already wrote its output artifacts.
pub(crate) fn run_captured(
    cmd: Command,
    stdin: Option<&[u8]>,
    cancel: &CancelToken,
) -> Result<Captured> {
    run_captured_with_limit(cmd, stdin, cancel, CAPTURE_LIMIT)
}

fn run_captured_with_limit(
    cmd: Command,
    stdin: Option<&[u8]>,
    cancel: &CancelToken,
    limit: usize,
) -> Result<Captured> {
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let mut child = Spawned::spawn(
        cmd,
        if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        },
    )?;
    let mut out = Some(capture(
        child.take_stdout().expect("stdout piped"),
        "stdout",
        limit,
    ));
    let mut err = Some(capture(
        child.take_stderr().expect("stderr piped"),
        "stderr",
        limit,
    ));
    std::thread::scope(|scope| {
        // Keep the waiter responsive even if the child isn't reading its
        // stdin. Borrow the input so signing doesn't make another key copy.
        let input = stdin.zip(child.take_stdin()).map(|(bytes, mut pipe)| {
            scope.spawn(move || {
                // An early pipe close is the child's verdict to report.
                let _ = pipe.write_all(bytes).and_then(|_| pipe.flush());
            })
        });
        let result = drain_captured(&mut child, &mut out, &mut err, cancel, || {
            input.as_ref().is_none_or(|t| t.is_finished())
        });
        if result.is_err() {
            // Includes a capture error, even if the leader already quit.
            // Kill remaining pipe holders before waiting for the workers.
            child.kill_tree(Duration::ZERO);
        }
        for reader in [out, err].into_iter().flatten() {
            let _ = reader.join();
        }
        result
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
        use std::os::windows::process::CommandExt;

        let mut c = Command::new("cmd");
        // `cmd /C` has compatibility quote-stripping rules, while normal
        // `Command::arg` quoting is for argv-aware programs and is not the
        // raw command text `cmd` expects. `/S` plus one verbatim outer pair
        // makes the stripping deterministic and preserves quotes inside the
        // script (notably the quoted helper executable path). `/D` keeps an
        // AutoRun registry command out of the test, and `/V:OFF` makes `!`
        // in helper paths deterministic even on machines that enable delayed
        // expansion globally.
        c.arg("/D").arg("/V:OFF").arg("/S").arg("/C");
        c.raw_arg(format!("\"{script}\""));
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    #[test]
    fn captures_pipes_and_code() {
        let cmd = shell_command("echo out && echo err 1>&2 && exit 1");
        let c = run_captured(cmd, None, &CancelToken::new()).unwrap();
        assert_eq!(c.outcome, ExitOutcome::Exited { code: 1 });
        assert!(String::from_utf8_lossy(&c.stdout).contains("out"));
        assert!(String::from_utf8_lossy(&c.stderr).contains("err"));
    }

    #[test]
    fn captures_complete_output_at_the_limit() {
        // Get the platform's actual line endings (echo writes CRLF on Windows).
        let script = "echo short && echo short 1>&2";
        let cancel = CancelToken::new();
        let reference = run_captured(shell_command(script), None, &cancel).unwrap();
        let limit = reference.stdout.len().max(reference.stderr.len());
        let at_limit =
            run_captured_with_limit(shell_command(script), None, &cancel, limit).unwrap();
        assert_eq!(at_limit.stdout, reference.stdout);
        assert_eq!(at_limit.stderr, reference.stderr);
        assert_eq!(at_limit.outcome, ExitOutcome::Exited { code: 0 });
    }

    #[test]
    fn fails_without_returning_a_prefix_when_either_pipe_overflows() {
        for (script, expected_stream) in [
            ("echo complete report && echo ok 1>&2", "stdout"),
            ("echo ok && echo complete report 1>&2", "stderr"),
        ] {
            assert!(matches!(
                run_captured_with_limit(shell_command(script), None, &CancelToken::new(), 8),
                Err(EngineError::OutputLimitExceeded { stream, limit: 8 })
                    if stream == expected_stream
            ));
        }
    }

    #[test]
    fn capture_stops_reading_an_endless_stream_after_the_limit() {
        struct Endless(Arc<std::sync::atomic::AtomicUsize>);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                buf.fill(b'x');
                self.0.fetch_add(buf.len(), Ordering::SeqCst);
                Ok(buf.len())
            }
        }
        let pulled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = capture(Endless(pulled.clone()), "stdout", 1024)
            .join()
            .unwrap();
        assert!(matches!(
            result,
            Err(EngineError::OutputLimitExceeded {
                stream: "stdout",
                limit: 1024
            })
        ));
        // One read chunk past the limit is the most that is ever pulled.
        assert!(pulled.load(Ordering::SeqCst) <= 1024 + 8192);
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_disables_autorun_and_delayed_expansion() {
        let cmd = shell_command("exit 0");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.iter().take(4).map(String::as_str).collect::<Vec<_>>(),
            vec!["/D", "/V:OFF", "/S", "/C"]
        );
    }

    const ENDLESS_HELPER_ENV: &str = "CV_ENGINE_ENDLESS_OUTPUT";
    const ENDLESS_STREAMS: [&str; 2] = ["stdout", "stderr"];
    // Longer than the public test bound: if tree killing breaks, the helper's
    // own exit still makes the regression finite and the elapsed-time
    // assertion below fails instead of the suite hanging forever.
    const ENDLESS_HELPER_SELF_STOP: Duration = Duration::from_secs(45);

    fn write_endless<W: Write>(mut sink: W) {
        let chunk = [b'x'; 8192];
        let stop_at = Instant::now() + ENDLESS_HELPER_SELF_STOP;
        loop {
            if Instant::now() >= stop_at {
                return;
            }
            if sink.write_all(&chunk).and_then(|()| sink.flush()).is_err() {
                // A broken pipe must not exit immediately: the surviving
                // process keeps the sibling pipe open, so only killing the
                // process tree lets `run_captured` finish promptly.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    #[ignore = "process-mode helper for the output-limit test"]
    fn endless_output_process_mode_writes_until_killed() {
        let Some(stream) = std::env::var_os(ENDLESS_HELPER_ENV) else {
            return;
        };
        match stream.to_str() {
            Some("stdout") => write_endless(io::stdout().lock()),
            Some("stderr") => write_endless(io::stderr().lock()),
            _ => {}
        }
    }

    struct EndlessOutputHelper {
        path: std::path::PathBuf,
        #[cfg(windows)]
        _scratch: tempfile::TempDir,
    }

    #[cfg(unix)]
    fn endless_output_helper() -> EndlessOutputHelper {
        EndlessOutputHelper {
            path: std::env::current_exe().expect("test executable path"),
        }
    }

    #[cfg(windows)]
    fn endless_output_helper() -> EndlessOutputHelper {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("crate has a workspace root");
        let scratch_root = workspace.join("target/cv-tmp");
        std::fs::create_dir_all(&scratch_root).expect("create process-test scratch root");
        let scratch = tempfile::Builder::new()
            .prefix("endless-helper-")
            .tempdir_in(&scratch_root)
            .expect("create process-test scratch dir");
        let shape_dir = scratch.path().join("helper path with spaces 100% & bang!");
        std::fs::create_dir_all(&shape_dir).expect("create shell-hostile helper dir");
        let path = shape_dir.join("endless-output helper 100% & bang!.exe");
        let source = std::env::current_exe().expect("test executable path");
        if let Err(link_error) = std::fs::hard_link(&source, &path) {
            std::fs::copy(&source, &path).unwrap_or_else(|copy_error| {
                panic!(
                    "failed to hard-link ({link_error}) or copy ({copy_error}) the endless-output helper"
                )
            });
        }
        EndlessOutputHelper {
            path,
            _scratch: scratch,
        }
    }

    fn endless_command_with_helper(stream: &str, helper: &Path) -> Command {
        // Re-execute this native test binary through the platform shell. The
        // shell spawns the writer as a descendant, so the test still proves
        // that output-overflow cleanup kills the process tree rather than only
        // the leader. Unlike the old PowerShell writer, the measured cost is
        // Capture Vector's process boundary, not CLR startup or interpreter
        // teardown. An environment variable carries the executable path so
        // the test does not have to splice arbitrary path text into shell
        // source. On Windows the helper path deliberately contains spaces and
        // shell punctuation to prove that expansion and quoting keep the
        // invoked executable intact. On Unix the background+wait shape
        // matters: a one-command `sh -c` script may exec the writer in-place,
        // which collapses the process tree the regression is meant to exercise.
        #[cfg(unix)]
        let script = concat!(
            "\"$CV_PROC_HELPER_EXE\" ",
            "proc::tests::endless_output_process_mode_writes_until_killed ",
            "--exact --nocapture --ignored & ",
            "child=$!; wait \"$child\""
        );
        #[cfg(windows)]
        let script = concat!(
            "\"%CV_PROC_HELPER_EXE%\" ",
            "proc::tests::endless_output_process_mode_writes_until_killed ",
            "--exact --nocapture --ignored"
        );
        let mut cmd = shell_command(script);
        cmd.env("CV_PROC_HELPER_EXE", helper)
            .env(ENDLESS_HELPER_ENV, stream);
        cmd
    }

    #[cfg(unix)]
    fn direct_child_pids(pid: u32) -> Vec<u32> {
        let mut cmd = Command::new("ps");
        // Use repeated `-o` options rather than `pid=,ppid=`: procps
        // documents comma-separated empty headers as personality-dependent.
        cmd.args(["-e", "-o", "pid=", "-o", "ppid="]);
        let captured = run_captured(cmd, None, &CancelToken::new()).expect("ps runs");
        String::from_utf8_lossy(&captured.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let child = fields.next()?.parse::<u32>().ok()?;
                let parent = fields.next()?.parse::<u32>().ok()?;
                (parent == pid).then_some(child)
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn unix_endless_output_command_keeps_writer_below_shell_leader() {
        let helper = endless_output_helper();
        let mut child = Spawned::spawn(
            endless_command_with_helper("stdout", &helper.path),
            Stdio::null(),
        )
        .unwrap();
        let leader = child.child.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut children = Vec::new();
        while Instant::now() < deadline {
            children = direct_child_pids(leader);
            if !children.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        child.kill_tree(Duration::ZERO);
        assert!(
            !children.is_empty(),
            "shell leader {leader} execed the writer in-place; the regression no longer exercises descendant cleanup"
        );
    }

    #[test]
    fn endless_child_output_fails_at_the_limit_instead_of_hanging() {
        let helper = endless_output_helper();
        for expected_stream in ENDLESS_STREAMS {
            let start = Instant::now();
            let result = run_captured_with_limit(
                endless_command_with_helper(expected_stream, &helper.path),
                None,
                &CancelToken::new(),
                1024,
            );
            let got = match &result {
                Err(e) => e.to_string(),
                Ok(c) => format!(
                    "Ok(stdout={} bytes, stderr={} bytes, stdout prefix={:?}, stderr prefix={:?})",
                    c.stdout.len(),
                    c.stderr.len(),
                    &c.stdout[..c.stdout.len().min(120)],
                    &c.stderr[..c.stderr.len().min(120)]
                ),
            };
            assert!(
                matches!(
                    result.as_ref(),
                    Err(EngineError::OutputLimitExceeded { stream, limit: 1024 })
                        if *stream == expected_stream
                ),
                "got {got}"
            );
            // The writer is killed rather than left blocked on a full pipe.
            // The bound only exists to catch a process-tree cleanup failure;
            // with a native helper it is deliberately far outside normal
            // spawn/teardown variance rather than a benchmark of PowerShell.
            let elapsed = start.elapsed();
            assert!(
                elapsed < Duration::from_secs(30),
                "{expected_stream} cleanup took {elapsed:?}"
            );
        }
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
    fn cancellation_does_not_rewrite_terminal_exit_while_draining() {
        let cancel = CancelToken::new();
        // The leader exits normally while a finite background task still
        // holds the pipes. Once terminal exit is observed it remains the
        // process outcome; cancellation cannot rewrite it retroactively.
        // Windows: `start /b` leaves a grandchild holding the pipe handles.
        #[cfg(unix)]
        let script = "sleep 1 & exit 0";
        #[cfg(windows)]
        let script = "start /b ping -n 3 127.0.0.1 & exit 0";
        let mut child = Spawned::spawn(shell_command(script), Stdio::null()).unwrap();
        let mut out = Some(capture(
            child.take_stdout().expect("stdout piped"),
            "stdout",
            CAPTURE_LIMIT,
        ));
        let mut err = Some(capture(
            child.take_stderr().expect("stderr piped"),
            "stderr",
            CAPTURE_LIMIT,
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "leader did not exit");
            std::thread::sleep(POLL);
        }
        cancel.cancel();
        let result = drain_captured(&mut child, &mut out, &mut err, &cancel, || true);
        if result.is_err() {
            child.kill_tree(Duration::ZERO);
        }
        for reader in [out, err].into_iter().flatten() {
            let _ = reader.join();
        }
        let result = result.unwrap();
        assert!(matches!(result.outcome, ExitOutcome::Exited { code: 0 }));
    }

    #[test]
    fn rapid_cancellation_race_never_rewrites_an_observed_exit() {
        // The exact interleaving is scheduler-dependent, so repeat a fast
        // exit/cancel race. Either pre-exit cancellation wins legitimately,
        // or an observed zero exit must remain zero rather than becoming
        // cancellation retroactively. The deterministic draining regression
        // above covers the terminal-exit side even on machines where every
        // fast race is pre-cancelled.
        for _ in 0..40 {
            let cancel = CancelToken::new();
            let c2 = cancel.clone();
            let t = std::thread::spawn(move || c2.cancel());
            #[cfg(unix)]
            let script = "exit 0";
            #[cfg(windows)]
            let script = "exit 0";
            match run_captured(shell_command(script), None, &cancel) {
                Ok(c) => {
                    assert!(matches!(c.outcome, ExitOutcome::Exited { code: 0 }));
                }
                Err(EngineError::Cancelled) => {}
                Err(e) => panic!("unexpected capture error: {e}"),
            }
            t.join().unwrap();
        }
    }

    #[test]
    fn child_env_is_an_allowlist_not_inherit_minus_scrub() {
        use std::ffi::OsStr;
        for allowed in [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TMPDIR",
            "HTTP_PROXY",
            "SSL_CERT_FILE",
            "BORIS_HOME",
        ] {
            assert!(env_allowed(OsStr::new(allowed)), "{allowed}");
        }
        for denied in [
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "CV_NOSTR_NSEC",
            "CV_BSKY_APP_PASSWORD",
            "RANDOM_APP_KEY",
        ] {
            assert!(!env_allowed(OsStr::new(denied)), "{denied}");
        }
    }

    #[test]
    fn non_allowlisted_env_never_reaches_child() {
        // SAFETY: name is unique to this test; other tests don't read it.
        unsafe { std::env::set_var("CV_TEST_UNLISTED", "shouldnotleak") };
        let mut cmd = boris_command(
            Path::new(if cfg!(windows) { "cmd" } else { "sh" }),
            Path::new("."),
        );
        #[cfg(unix)]
        cmd.arg("-c").arg("env");
        #[cfg(windows)]
        cmd.arg("/C").arg("set");
        let c = run_captured(cmd, None, &CancelToken::new()).unwrap();
        assert!(!String::from_utf8_lossy(&c.stdout).contains("shouldnotleak"));
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

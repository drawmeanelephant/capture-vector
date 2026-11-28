# Process-boundary regression tests

The tests in `crates/cv-engine/src/proc.rs` check that cancellation and
output-overflow cleanup reach the whole child process tree. They are
test-only probes. They do not change the rule that `cv-engine` owns process
creation.

Run them with:

```sh
cargo test -p cv-engine --locked proc::tests::
```

## Coverage

- `unix_endless_output_command_keeps_writer_below_shell_leader` fails when
  the platform shell executes the endless writer in place. The output-limit
  regression must keep the writer below the shell leader, or it no longer
  exercises descendant cleanup.
- `endless_child_output_fails_at_the_limit_instead_of_hanging` overflows
  `stdout` and `stderr` in turn. The writer is a re-executed native test
  binary, so the measurement is Capture Vector's process boundary rather
  than interpreter startup.
- On Windows the test shell runs as `cmd /D /V:OFF /S /C`. The helper
  executable path contains spaces, `%`, `&`, and `!`, so AutoRun commands,
  delayed expansion, quoting, and command separators cannot change the
  invoked path.

## Failure bounds

The endless writer stops itself after 45 seconds. That bound covers the
escaped-writer failure this regression targets. If process-tree cleanup
breaks, the surviving writer eventually exits, the pipe readers finish, and
the test fails its 30-second elapsed assertion instead of hanging forever.

The 30-second assertion is not a watchdog. It runs only after
`run_captured` returns.

An independent deadlock inside `run_captured` or a capture-thread join
remains unbounded inside the test process. The same gap applies if
process-tree teardown never completes. The outer bound is the CI job
timeout, or the caller's timeout for a local run.

A watchdog that re-executes the test binary was considered and rejected. To
clean up a deadlocked test child, that watchdog would have to reproduce the
same Unix process-group and Windows job containment that the test is trying
to prove, so it would not provide independent evidence.

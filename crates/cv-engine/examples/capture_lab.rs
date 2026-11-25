//! Records `fixtures/captures/lab/<mode>/` from the pinned
//! `boris-migration-lab`. Use `scripts/capture-lab-fixtures.sh`, which
//! also rewrites the `[lab_captures]` manifest table:
//!
//! `cargo run -p cv-engine --example capture_lab -- <dest-dir>`
//!
//! Needs `CV_MIGRATION_LAB_BIN` (or the lab on `PATH`) and the pinned
//! Boris (`CV_BORIS_BIN` or `PATH`) for link-audit's HTML input.

#[path = "../tests/common/lab_capture.rs"]
mod lab_capture;

use cv_fixtures::lab::LAB_CAPTURES;
use std::path::PathBuf;

fn main() -> lab_capture::Result<()> {
    let dest = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: capture_lab <dest-dir>")?,
    );
    let scratch = tempfile::tempdir()?;
    let root = std::fs::canonicalize(scratch.path())?;
    let tools = lab_capture::Tools::from_env(&root)?;
    eprintln!(
        "lab {} at {:?}; boris {} at {:?}; fixtures {:?}",
        tools.lab.tool_id,
        tools.lab.path,
        tools.boris.version,
        tools.boris.path,
        tools.lab_fixtures
    );
    for spec in LAB_CAPTURES {
        let t = lab_capture::capture(&tools, spec, &root)?;
        t.write_capture(&dest.join(spec.mode))?;
        eprintln!(
            "{}: exit {}, {} file(s)",
            spec.mode,
            t.exit_code,
            t.out.len()
        );
    }
    Ok(())
}

// Integration-focused unit tests for the workspace snapshot projection.

use super::*;
use std::time::SystemTime;

#[test]
fn crlf_frontmatter_still_parses_fields() {
    let p = page("a.md", "---\r\nid: x\r\ntitle: T\r\n---\r\nbody\r\n");
    assert_eq!(p.id, "x");
    assert_eq!(p.title, "T");
    assert_eq!(p.body, "body\r\n");
}

#[test]
fn dash_leading_body_is_kept_verbatim() {
    let p = page("a.md", "---\ntitle: T\n---\n- one\n- two\n---\n");
    assert_eq!(p.body, "- one\n- two\n---\n");
}

#[test]
fn unclosed_fence_is_no_frontmatter() {
    let raw = "---\ntitle: T\nbody without a closing fence\n";
    let p = page("a.md", raw);
    assert_eq!(p.body, raw);
    assert_eq!(p.id, "a");
}

/// Frontmatter `id` spelling conformance, probed against Boris 0.9.0
/// (`zig-out/bin/boris validate`) as the authoritative oracle in
/// November 2026: every spelling variant Boris accepts binds exactly as
/// the scanner reads it, and every spelling the scanner reads
/// differently is one Boris hard-fails (`EFRONTMATTER`) — so over the
/// domain of workspaces Boris accepts at all, the scanner's notion of an
/// explicit id is exactly Boris's. The negative cases below pin the
/// scanner behavior for content that can never survive validation.
#[test]
fn explicit_id_spelling_variants_match_boris_acceptance() {
    // Variants Boris accepts and binds (duplicate-detection probes):
    for (name, fm, want) in [
        ("plain", "id: xx-a", "xx-a"),
        ("no space after colon", "id:xx-b", "xx-b"),
        ("space before colon", "id : xx-c", "xx-c"),
        ("double-quoted", "id: \"xx-d\"", "xx-d"),
    ] {
        let p = page("some/path.md", &format!("---\n{fm}\n---\nbody\n"));
        assert_eq!(p.id, want, "{name}");
        assert!(p.explicit_id, "{name} is explicit");
    }
}

/// Spellings Boris rejects with `EFRONTMATTER`: the scanner's reading is
/// unobservable in any workspace that validates, but pin it so a drift
/// in either direction is a deliberate diff, not an accident.
#[test]
fn boris_rejected_spellings_are_pinned_as_unobservable() {
    // Single-quoted: Boris hard-fails; the scanner keeps the quotes as
    // literal bytes — divergent in principle, unreachable in practice.
    let p = page("x.md", "---\nid: 'xx-e'\n---\nbody\n");
    assert!(
        p.explicit_id,
        "scanner marks it, but Boris rejects the file"
    );
    // Empty: Boris hard-fails (`non-empty` required); the scanner treats
    // it as absent and falls back to the path.
    let p = page("x.md", "---\nid:\n---\nbody\n");
    assert!(!p.explicit_id);
    assert_eq!(p.id, "x");
    // Uppercase key / indented line: Boris hard-fails as unknown keys or
    // unsupported nesting; the scanner reads neither as an id.
    let p = page("x.md", "---\nID: xx-f\n---\nbody\n");
    assert!(!p.explicit_id);
    let p = page("x.md", "---\n  id: xx-g\n---\nbody\n");
    assert!(
        !p.explicit_id,
        "indented lines are no key at all, like Boris"
    );
}

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn paths(pages: &[PageRecord]) -> Vec<&str> {
    pages.iter().map(|p| p.path.as_str()).collect()
}

#[test]
fn workspace_snapshot_uses_default_content_and_boris_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "content/index.md", "---\ntitle: Home\n---\nHello");
    write(root, "content/guides/recipe.cook", "Cook");
    write(root, "content/guides/format.textile", "Textile");
    write(root, "content/readme.txt", "Not a page");
    write(root, "outside-default-input.md", "Not a content page");
    write(
        root,
        "dist/_boris/proof/artifacts.json",
        corpus("happy").file(ARTIFACTS),
    );

    let snap = workspace_snapshot(root, "dist", Some("boris".into()), 7);
    assert_eq!(
        paths(&snap.pages),
        ["guides/format.textile", "guides/recipe.cook", "index.md"]
    );
    assert_eq!((snap.epoch, snap.compiler.as_deref()), (7, Some("boris")));
    assert_eq!(snap.target.as_deref(), Some("default"));
    assert_eq!(snap.pages[2].title, "Home");
    assert_eq!(snap.pages[2].body, "Hello");
    assert_eq!(snap.pages[2].output.as_deref(), Some("index.html"));
}

#[test]
fn outputs_follow_stable_entity_ids_and_mdx_pages_are_scanned() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "content/guides/renamed.md",
        "---\nid: \"guides/getting-started\"\ntitle: Renamed\n---\nBody",
    );
    write(root, "content/page.mdx", "# MDX");
    write(
        root,
        "dist/_boris/proof/artifacts.json",
        corpus("happy").file(ARTIFACTS),
    );

    let snap = workspace_snapshot(root, "dist", None, 1);
    assert_eq!(paths(&snap.pages), ["guides/renamed.md", "page.mdx"]);
    assert_eq!(
        snap.pages[0].output.as_deref(),
        Some("guides/getting-started.html")
    );
    assert_eq!(snap.pages[1].output, None);
}

/// Copies a captured editor-lane corpus into a fresh workspace,
/// with its captured IR graph at `ir_rel` when Boris wrote one.
fn captured_workspace(name: &str, ir_rel: &str) -> tempfile::TempDir {
    let editor = fixtures_root().join("editor");
    let manifest = Manifest::load(&editor.join("MANIFEST.toml"));
    let temp = tempfile::tempdir().unwrap();
    let src_prefix = format!("corpus-src/{name}/");
    for (key, _) in manifest.pinned_files() {
        if let Some(rel) = key.strip_prefix(&src_prefix) {
            write(temp.path(), rel, fs::read(editor.join(key)).unwrap());
        }
    }
    let key = format!("captures/{name}/graph.json");
    if let Ok(bytes) = fs::read(editor.join(&key)) {
        manifest.verify(&key, &bytes).unwrap();
        write(temp.path(), ir_rel, bytes);
    }
    temp
}

fn by_path<'a>(snap: &'a ProjectSnapshot, path: &str) -> &'a PageRecord {
    snap.pages.iter().find(|p| p.path == path).unwrap()
}

#[test]
fn quoted_scalars_display_as_boris_reports_them() {
    let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
    let snap = workspace_snapshot(ws.path(), "dist", None, 1);

    let home = by_path(&snap, "index.md");
    assert_eq!(home.fields, FieldSource::Boris);
    assert_eq!(home.id, "home");
    assert_eq!(home.title, "Mini Site (English)");
    assert_eq!(home.parent, None);
    assert_eq!(home.tags, ["start", "quoted: yes"]);

    let double = by_path(&snap, "guides/double.md");
    assert_eq!(double.fields, FieldSource::Boris);
    assert_eq!(double.id, "guides/double");
    assert_eq!(double.title, "Double: quoted (title)");
    assert_eq!(double.parent.as_deref(), Some("home"));
    assert_eq!(
        double.relations,
        [("relates_to".to_string(), "home".to_string())]
    );
}

#[test]
fn workspace_snapshot_shares_the_frozen_ir_graph_it_projects() {
    let ws = captured_workspace("quoted-scalars", ".cv/ir/graph.json");
    let snap = workspace_snapshot(ws.path(), "dist", None, 1);
    let graph = snap
        .ir_graph
        .as_ref()
        .expect("IR graph is retained in memory");
    assert_eq!(graph.schema_version, "0.3.0");
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.relations.len(), 1);
    assert_eq!(graph.relations[0].kind, "relates_to");
}

#[test]
fn an_ir_graph_beside_the_html_dir_is_read_too() {
    let ws = captured_workspace("quoted-scalars", "dist/graph.json");
    let snap = workspace_snapshot(ws.path(), "dist", None, 1);
    assert_eq!(by_path(&snap, "index.md").title, "Mini Site (English)");
    assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
}

fn set_mtime(root: &Path, rel: &str, at: SystemTime) {
    File::options()
        .write(true)
        .open(root.join(rel))
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[test]
fn the_live_ir_projection_is_read_first_and_a_newer_build_still_wins() {
    let ws = captured_workspace("quoted-scalars", ".cv/ir/graph.json");
    // A graph from another build that also covers `index.md`.
    let other = cv_fixtures::ir_capture("happy").file("graph.json");
    write(ws.path(), ".boris/graph.json", &other);
    write(ws.path(), "dist/graph.json", &other);
    let at = SystemTime::now() - std::time::Duration::from_secs(60);
    for rel in [".cv/ir/graph.json", ".boris/graph.json", "dist/graph.json"] {
        set_mtime(ws.path(), rel, at);
    }
    let title = |epoch| {
        let snap = workspace_snapshot(ws.path(), "dist", None, epoch);
        by_path(&snap, "index.md").title.clone()
    };
    assert_eq!(title(1), "Mini Site (English)");

    set_mtime(ws.path(), "dist/graph.json", SystemTime::now());
    assert_eq!(title(2), "My Boris Site");
}

#[test]
fn a_malformed_newest_ir_graph_falls_back_to_an_older_valid_graph() {
    let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
    write(ws.path(), ".cv/ir/graph.json", "{\"not\": ");
    let older = SystemTime::now() - std::time::Duration::from_secs(60);
    set_mtime(ws.path(), ".boris/graph.json", older);
    set_mtime(ws.path(), ".cv/ir/graph.json", SystemTime::now());

    let snap = workspace_snapshot(ws.path(), "dist", None, 1);
    assert_eq!(by_path(&snap, "index.md").title, "Mini Site (English)");
    assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
}

#[cfg(unix)]
#[test]
fn a_linked_newest_ir_graph_is_not_followed_and_an_older_valid_graph_wins() {
    let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
    let outside = tempfile::tempdir().unwrap();
    write(
        outside.path(),
        "graph.json",
        cv_fixtures::ir_capture("happy").file("graph.json"),
    );
    std::fs::create_dir_all(ws.path().join(".cv/ir")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("graph.json"),
        ws.path().join(".cv/ir/graph.json"),
    )
    .unwrap();
    let older = SystemTime::now() - std::time::Duration::from_secs(60);
    set_mtime(ws.path(), ".boris/graph.json", older);

    let snap = workspace_snapshot(ws.path(), "dist", None, 1);
    assert_eq!(by_path(&snap, "index.md").title, "Mini Site (English)");
    assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
}

#[test]
fn source_fallback_reads_boris_double_quoted_scalars_without_quotes() {
    let temp = tempfile::tempdir().unwrap();
    write(
        temp.path(),
        "content/guides/renamed.md",
        "---\nid: \"guides/getting-started\"\ntitle: \"Getting Started\"\nparent: \"index\"\n---\n# Body\n",
    );
    let snap = workspace_snapshot(temp.path(), "dist", None, 1);
    let p = by_path(&snap, "guides/renamed.md");
    assert_eq!(p.fields, FieldSource::Source);
    assert_eq!(p.id, "guides/getting-started");
    assert_eq!(p.title, "Getting Started");
    assert_eq!(p.parent.as_deref(), Some("index"));
    assert_eq!(
        p.frontmatter,
        vec![
            ("id".into(), "\"guides/getting-started\"".into()),
            ("title".into(), "\"Getting Started\"".into()),
            ("parent".into(), "\"index\"".into()),
        ],
        "frontmatter rows stay exactly as authored"
    );
}

#[test]
fn pages_without_boris_ir_are_marked_as_read_from_source() {
    let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
    write(
        ws.path(),
        "content/guides/new.md",
        "---\ntitle: \"Added after the build\"\n---\n",
    );
    let snap = workspace_snapshot(ws.path(), "dist", None, 1);
    assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
    assert_eq!(by_path(&snap, "guides/new.md").fields, FieldSource::Source);

    fs::remove_file(ws.path().join(".boris/graph.json")).unwrap();
    let snap = workspace_snapshot(ws.path(), "dist", None, 2);
    assert!(snap.pages.iter().all(|p| p.fields == FieldSource::Source));
}

/// Boris rejects single-quoted scalars, so it writes no IR for them and
/// the pages stay marked as a source guess.
#[test]
fn single_quoted_scalars_get_no_boris_ir_and_stay_marked() {
    let captures = fixtures_root().join("editor/captures/single-quoted");
    let stderr = fs::read_to_string(captures.join("ir-stderr.txt")).unwrap();
    assert!(
        stderr.contains("EFRONTMATTER: single.md:2:1: single-quoted values are not supported"),
        "{stderr}"
    );
    assert!(!captures.join("graph.json").exists());

    let ws = captured_workspace("single-quoted", ".boris/graph.json");
    let snap = workspace_snapshot(ws.path(), "dist", None, 1);
    assert_eq!(snap.pages.len(), 2);
    assert!(snap.pages.iter().all(|p| p.fields == FieldSource::Source));
}

#[test]
fn workspace_snapshot_keeps_pages_when_the_artifact_index_is_malformed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "content/index.md", "Hello");
    write(
        root,
        "dist/_boris/proof/artifacts.json",
        "{\"artifacts\": [",
    );

    let snap = workspace_snapshot(root, "dist", None, 1);
    assert_eq!(paths(&snap.pages), ["index.md"]);
    assert!(snap.outputs.is_empty());
    assert!(snap.target.is_none());
    assert!(snap.pages[0].output.is_none());
}

#[test]
fn corpus_snapshot_reads_the_captured_artifact_index() {
    let snap = corpus_snapshot("happy");
    assert_eq!(snap.target.as_deref(), Some("default"));
    assert!(snap.outputs.iter().any(|o| o.path == "index.html"));
}

#[test]
fn workspace_snapshot_respects_a_nested_profile_input() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "boris.json", r#"{"input":"site/posts/."}"#);
    write(root, "site/posts/guide.md", "Guide");
    write(root, "content/default.md", "Not the configured input");

    let snap = workspace_snapshot(root, "dist", None, 1);
    assert_eq!(paths(&snap.pages), ["guide.md"]);
}

#[test]
fn scan_preserves_skips_and_includes_pages_at_the_depth_limit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for path in [
        ".hidden/no.md",
        "dist/no.md",
        "themes/no.md",
        "node_modules/no.md",
        "target/no.md",
        "folder.md/within.md",
        "folder.md/deeper/no.md",
        "root.md",
    ] {
        write(root, path, "Body");
    }

    let mut scan = Scan::new("dist");
    scan.max_depth = 1;
    scan.walk(root, "", 0);
    assert_eq!(paths(&scan.pages), ["folder.md/within.md", "root.md"]);
}

#[test]
fn scan_omits_oversized_and_invalid_pages_without_truncating_valid_pages() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a-oversized.md", "12345");
    write(root, "b-invalid.md", [0xff]);
    write(root, "c-exact.md", "1234");
    write(root, "d-empty.md", "");

    let mut scan = Scan::new("dist");
    scan.max_source_bytes = 4;
    scan.max_pages = 2;
    scan.walk(root, "", 0);
    assert_eq!(paths(&scan.pages), ["c-exact.md", "d-empty.md"]);
    assert_eq!(scan.pages[0].body, "1234");
    assert_eq!(scan.bytes_left, MAX_TOTAL_SOURCE_BYTES - 5);
}

#[test]
fn scan_never_accepts_more_than_the_remaining_source_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.md", "1234");
    write(root, "b-cannot-fit.md", "1234");
    write(root, "c-fits.md", "123");
    write(root, "d-after-limit.md", "1");

    let mut scan = Scan::new("dist");
    scan.bytes_left = 7;
    scan.walk(root, "", 0);
    assert_eq!(paths(&scan.pages), ["a.md", "c-fits.md"]);
    assert_eq!(scan.bytes_left, 0);
}

#[test]
fn bounded_reader_checks_actual_length_with_one_extra_byte() {
    let mut remaining = 8;
    let mut input = &b"12345678"[..];
    assert!(bounded_text(&mut input, 4, &mut remaining).is_none());
    assert_eq!(remaining, 3);
    assert_eq!(input, b"678");
    assert_eq!(
        bounded_text(input, 4, &mut remaining).as_deref(),
        Some("678")
    );
    assert_eq!(remaining, 0);
}

#[test]
fn scan_limits_pages_in_sort_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for path in ["c.md", "a.md", "b.md"] {
        write(root, path, "Body");
    }
    let mut scan = Scan::new("dist");
    scan.max_pages = 2;
    scan.walk(root, "", 0);
    assert_eq!(paths(&scan.pages), ["a.md", "b.md"]);
}

#[test]
fn scan_limits_entries_before_sorting() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for path in ["a.md", "b.md", "c.md"] {
        write(root, path, "Body");
    }
    let mut scan = Scan::new("dist");
    scan.entries_left = 2;
    scan.walk(root, "", 0);
    assert_eq!(scan.entries_left, 0);
    assert_eq!(scan.pages.len(), 2);
    assert!(scan.pages[0].path < scan.pages[1].path);
}

#[cfg(unix)]
#[test]
fn workspace_snapshot_skips_linked_files_and_directories() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "content/real/one.md", "Inert page");
    symlink("real/one.md", root.join("content/copied.md")).unwrap();
    symlink("real", root.join("content/copied-dir")).unwrap();

    let snap = workspace_snapshot(root, "dist", None, 1);
    assert_eq!(paths(&snap.pages), ["real/one.md"]);
}

#[cfg(unix)]
#[test]
fn workspace_snapshot_refuses_a_link_in_the_content_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "actual/nested/one.md", "Inert page");
    std::os::unix::fs::symlink("actual", root.join("content")).unwrap();

    let implicit = workspace_snapshot(root, "dist", None, 1);
    assert!(implicit.pages.is_empty());
    write(root, "boris.json", r#"{"input":"content/nested"}"#);
    let explicit = workspace_snapshot(root, "dist", None, 2);
    assert!(explicit.pages.is_empty());
}

#[test]
fn scan_skips_a_sparse_page_over_the_size_cap_without_reading_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    File::create(root.join("a-huge.md"))
        .unwrap()
        .set_len(MAX_SOURCE_BYTES + 1)
        .unwrap();
    write(root, "b-page.md", "Body");

    let mut scan = Scan::new("dist");
    scan.walk(root, "", 0);
    assert_eq!(paths(&scan.pages), ["b-page.md"]);
    assert_eq!(scan.bytes_left, MAX_TOTAL_SOURCE_BYTES - 4);
}

#[test]
fn scan_stops_descending_past_the_default_depth_cap() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let deepest = vec!["d"; MAX_DEPTH].join("/");
    write(root, &format!("{deepest}/in.md"), "Body");
    write(root, &format!("{deepest}/d/out.md"), "Body");
    write(root, "top.md", "Body");

    let mut scan = Scan::new("dist");
    scan.walk(root, "", 0);
    assert_eq!(
        paths(&scan.pages),
        [format!("{deepest}/in.md").as_str(), "top.md"]
    );
}

/// A regression that blocks in `open` fails here instead of hanging
/// the test run; the stuck thread is abandoned.
fn before_deadline<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("reading the workspace blocked")
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
}

/// File links only: a Windows symlink and a Unix symlink both carry
/// the reparse bit `is_link` checks. std has no cross-platform link
/// API, so the tests share one helper. FIFOs and devices stay
/// unix-only — Windows has no equivalent a page open could hit.
#[cfg(unix)]
fn link_file(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn link_file(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn scan_pages(root: &Path) -> Vec<String> {
    let root = root.to_path_buf();
    before_deadline(move || {
        let mut scan = Scan::new("dist");
        scan.walk(&root, "", 0);
        scan.pages.into_iter().map(|p| p.path).collect()
    })
}

#[cfg(unix)]
#[test]
fn scan_skips_a_fifo_named_like_a_page_without_blocking() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    mkfifo(&root.join("a-pipe.md"));
    write(root, "b-page.md", "Body");

    assert_eq!(scan_pages(root), ["b-page.md"]);
}

#[cfg(unix)]
#[test]
fn scan_skips_a_link_to_a_device_named_like_a_page() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::os::unix::fs::symlink("/dev/zero", root.join("a-zero.md")).unwrap();
    write(root, "b-page.md", "Body");

    assert_eq!(scan_pages(root), ["b-page.md"]);
}

#[test]
fn scan_skips_a_link_to_a_page_outside_the_root() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "secret.md", "Outside");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    link_file(&outside.path().join("secret.md"), &root.join("a-link.md")).unwrap();
    write(root, "b-page.md", "Body");

    assert_eq!(scan_pages(root), ["b-page.md"]);
}

/// Stands in for an entry replaced between the type check and the open.
fn read_swapped(swapped_in: PathBuf, checked: Metadata) -> Option<String> {
    before_deadline(move || {
        let mut left = MAX_TOTAL_SOURCE_BYTES;
        regular_text(&swapped_in, &checked, MAX_SOURCE_BYTES, &mut left)
    })
}

#[cfg(unix)]
#[test]
fn regular_text_does_not_block_on_a_fifo_swapped_in_after_the_type_check() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "page.md", "Body");
    mkfifo(&root.join("pipe.md"));
    let checked = fs::symlink_metadata(root.join("page.md")).unwrap();

    assert_eq!(read_swapped(root.join("pipe.md"), checked), None);
}

#[test]
fn regular_text_does_not_follow_a_link_swapped_in_after_the_type_check() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "secret.md", "Outside");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "page.md", "Body");
    link_file(&outside.path().join("secret.md"), &root.join("link.md")).unwrap();
    let checked = fs::symlink_metadata(root.join("page.md")).unwrap();

    assert_eq!(read_swapped(root.join("link.md"), checked.clone()), None);
    assert_eq!(
        read_swapped(root.join("page.md"), checked).as_deref(),
        Some("Body")
    );
}
fn write_adoption_for(root: &Path, rec: &cv_store::imports::AdoptionRecord) {
    let lock = match cv_store::imports::claim_run(root, &rec.run_id) {
        Ok(lock) => lock,
        Err(cv_store::imports::Error::RunExists(_)) => {
            cv_store::imports::lock_run(root, &rec.run_id).unwrap()
        }
        Err(e) => panic!("adoption test lock: {e}"),
    };
    cv_store::imports::write_adoption(&lock, rec).unwrap();
}

/// Blocker-3 probe: provenance is keyed by the stable entity id, so
/// it survives the file being renamed — and a different page moved
/// onto the adopted page's old path must not inherit it.
#[test]
fn provenance_survives_rename_and_never_follows_the_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "content/posts/old-name.md",
        "---\nid: \"en/posts/hello-world\"\ntitle: Hello\n---\nBody",
    );
    let mut rec = cv_store::imports::AdoptionRecord::new("run-1", "hugo", 0, "ws", "existing");
    rec.pages.push(cv_store::imports::AdoptedPage {
        entity_id: "en/posts/hello-world".into(),
        path: "posts/old-name.md".into(),
        source: Some("hugo-export/post/hello-world.md".into()),
    });
    write_adoption_for(root, &rec);

    let snap = workspace_snapshot(root, "dist", None, 1);
    let page = snap
        .pages
        .iter()
        .find(|p| p.path == "posts/old-name.md")
        .unwrap();
    assert_eq!(
        page.imported_from,
        Some(("hugo".into(), "hugo-export/post/hello-world.md".into())),
        "provenance attaches by stable id before the rename"
    );

    // Rename the file; the explicit Boris id travels with the bytes.
    fs::create_dir_all(root.join("content/essays")).unwrap();
    fs::rename(
        root.join("content/posts/old-name.md"),
        root.join("content/essays/new-name.md"),
    )
    .unwrap();
    // A different page squats on the vacated path.
    write(
        root,
        "content/posts/old-name.md",
        "---\ntitle: Unrelated\n---\nOther",
    );

    let snap = workspace_snapshot(root, "dist", None, 2);
    let moved = snap
        .pages
        .iter()
        .find(|p| p.path == "essays/new-name.md")
        .unwrap();
    assert_eq!(
        moved.imported_from,
        Some(("hugo".into(), "hugo-export/post/hello-world.md".into())),
        "the same stable id keeps its provenance across the move"
    );
    let squatter = snap
        .pages
        .iter()
        .find(|p| p.path == "posts/old-name.md")
        .unwrap();
    assert_eq!(
        squatter.imported_from, None,
        "a different page at the old path must not inherit provenance"
    );
}

fn adopt_record(root: &Path, entity_id: &str, path: &str) {
    let mut rec = cv_store::imports::AdoptionRecord::new("run-x", "hugo", 0, "ws", "existing");
    rec.pages.push(cv_store::imports::AdoptedPage {
        entity_id: entity_id.into(),
        path: path.into(),
        source: Some("export/post.md".into()),
    });
    write_adoption_for(root, &rec);
}

/// Review probe: an unrelated page with no explicit id gets the
/// path-derived fallback `posts/squatter`; adoption provenance for
/// that same entity id must NOT attach to it.
#[test]
fn provenance_never_attaches_to_a_path_derived_fallback_id() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "content/posts/squatter.md",
        "---\ntitle: Unrelated\n---\nBody",
    );
    adopt_record(root, "posts/squatter", "posts/squatter.md");

    let snap = workspace_snapshot(root, "dist", None, 1);
    let page = snap
        .pages
        .iter()
        .find(|p| p.path == "posts/squatter.md")
        .unwrap();
    assert_eq!(page.id, "posts/squatter", "the fallback id is path-derived");
    assert!(!page.explicit_id);
    assert_eq!(
        page.imported_from, None,
        "path-derived ids never carry provenance"
    );
}

/// The adopted page itself, when it carries its entity id explicitly,
/// is matched even when its path stem differs from the id.
#[test]
fn provenance_attaches_to_the_explicit_id_not_the_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "content/posts/renamed-away.md",
        "---\nid: \"posts/squatter\"\ntitle: Real\n---\nBody",
    );
    adopt_record(root, "posts/squatter", "posts/squatter.md");

    let snap = workspace_snapshot(root, "dist", None, 1);
    let page = snap
        .pages
        .iter()
        .find(|p| p.path == "posts/renamed-away.md")
        .unwrap();
    assert!(page.explicit_id);
    assert_eq!(
        page.imported_from,
        Some(("hugo".into(), "export/post.md".into())),
        "the explicit id is stable identity regardless of path"
    );
}

/// A page missing an explicit id entirely is never marked, even when
/// the adoption record names exactly its fallback id.
#[test]
fn provenance_requires_an_explicit_id_even_at_the_recorded_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // The adoption record points at this very path, but the bytes
    // there carry no explicit id: the association is unprovable.
    write(
        root,
        "content/posts/plain.md",
        "---\ntitle: Plain\n---\nBody",
    );
    adopt_record(root, "posts/plain", "posts/plain.md");

    let snap = workspace_snapshot(root, "dist", None, 1);
    let page = snap
        .pages
        .iter()
        .find(|p| p.path == "posts/plain.md")
        .unwrap();
    assert_eq!(page.imported_from, None);
}

/// Two pages sharing one explicit id make the association ambiguous:
/// neither is marked. Boris independently refuses that workspace
/// with EDUPLICATEID, so nothing is lost.
#[test]
fn duplicate_explicit_ids_mark_neither_page() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "content/posts/one.md",
        "---\nid: \"posts/dup\"\ntitle: One\n---\nBody",
    );
    write(
        root,
        "content/posts/two.md",
        "---\nid: \"posts/dup\"\ntitle: Two\n---\nBody",
    );
    adopt_record(root, "posts/dup", "posts/one.md");

    let snap = workspace_snapshot(root, "dist", None, 1);
    for path in ["posts/one.md", "posts/two.md"] {
        let page = snap.pages.iter().find(|p| p.path == path).unwrap();
        assert_eq!(
            page.imported_from, None,
            "ambiguous duplicate id must mark neither: {path}"
        );
    }
}

/// Moving an explicitly-identified page into a subdirectory keeps its
/// provenance; the id, not the location, is the identity.
#[test]
fn provenance_survives_moves_into_subdirectories() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "content/a.md",
        "---\nid: \"stable/x\"\ntitle: A\n---\nBody",
    );
    adopt_record(root, "stable/x", "a.md");
    fs::create_dir_all(root.join("content/deep/nest")).unwrap();
    fs::rename(
        root.join("content/a.md"),
        root.join("content/deep/nest/a.md"),
    )
    .unwrap();

    let snap = workspace_snapshot(root, "dist", None, 1);
    let page = snap
        .pages
        .iter()
        .find(|p| p.path == "deep/nest/a.md")
        .unwrap();
    assert_eq!(
        page.imported_from,
        Some(("hugo".into(), "export/post.md".into()))
    );
}

//! cv-app — composition root binary. Native window by default; rune's
//! headless flags (`--scene`, `--tree`, `--snap`, `--script`) also apply.
//! `--version` / `--build-info` print and exit without opening a window.

fn main() {
    cv_app::logging::init_from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("{}", cv_app::build_info::version_line());
        return;
    }
    if args.iter().any(|a| a == "--build-info") {
        let cwd = std::env::current_dir().unwrap_or_default();
        println!("{}", cv_app::build_info::current(&cwd));
        return;
    }
    let headless = args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--scene" | "--tree" | "--snap" | "--script"));
    cv_app::logging::info(
        "app_start",
        &[
            ("argc", serde_json::json!(args.len())),
            ("headless", serde_json::json!(headless)),
        ],
    );
    let scene = args
        .iter()
        .position(|a| a == "--scene")
        .and_then(|i| args.get(i + 1));
    if scene.is_some_and(|s| cv_app::editor::SCENES.iter().any(|(n, _)| n == s)) {
        cv_widgets::editor::run_scenes(cv_app::editor::SCENES);
        return;
    }
    cv_widgets::app::run(cv_app::launch());
}

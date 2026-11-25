//! cv-app — composition root binary. Native window by default; rune's
//! headless flags (`--scene`, `--tree`, `--snap`, `--script`) also apply.
//! `--version` / `--build-info` print and exit without opening a window.
//! `--editor` runs one editor window instead, over the editor scenes.

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
    if args.iter().any(|a| a == "--editor") {
        cv_widgets::editor::host::run_scenes(cv_app::editor_scene::SCENES);
        return;
    }
    cv_widgets::app::run(cv_app::launch());
}

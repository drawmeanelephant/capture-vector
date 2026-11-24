//! cv-app — composition root binary. Native window by default; rune's
//! headless flags (`--scene`, `--tree`, `--snap`, `--script`) also apply.
//! `--version` / `--build-info` print and exit without opening a window.

fn main() {
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
    cv_widgets::app::run(cv_app::launch());
}

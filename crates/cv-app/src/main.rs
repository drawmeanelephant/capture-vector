//! cv-app — composition root binary. Native window by default; rune's
//! headless flags (`--scene`, `--tree`, `--snap`, `--script`) also apply.

fn main() {
    cv_widgets::app::run(cv_app::launch());
}

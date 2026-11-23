//! Regenerate `rulepacks/SOURCES.md` from the v1 rule citations:
//! `cargo run -p cv-rules --example gen-sources > rulepacks/SOURCES.md`

fn main() {
    print!("{}", cv_rules::Ruleset::v1().sources_markdown());
}

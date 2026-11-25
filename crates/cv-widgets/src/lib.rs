//! cv-widgets — the ONLY crate importing `rune`/`rune-kit`.
//! All rune contact is firewalled here so the pinned dev rev can be
//! swapped without touching lanes. CI's manifest grep enforces the boundary.
//!
//! - [`shell`]: the module rail/chrome view over `cv_model::workbench`;
//!   `palette` draws its command palette, `reading` and `inspector` its
//!   right-hand panes.
//! - [`theme`]: `cv_model::theme` tokens → rune colours/type/springs, kit.
//! - [`controls`], [`icons`]: buttons on the app kit; the icon set as data.
//! - [`fx`]: opt-in signature effects (`effects` feature for the GPU layers).
//! - [`harness`]: headless runtime, scripted input, tree/layout goldens.
//! - `reference` (feature): replay a headless frame on `rune-ref`'s CPU oracle.
//! - [`app`]: native window boot + companion windows.
//! - [`editor`]: the L7 editor surface (form + body, problems, draft sketch).

pub mod app;
pub mod controls;
pub mod diagnostic;
pub mod editor;
pub mod fx;
/// QA machinery (scripted-input harness, golden bless/check). On by
/// default so `cargo test --workspace` exercises it; the shipped app
/// (cv-app) depends with `default-features = false` so it isn't linked.
#[cfg(any(test, feature = "harness"))]
pub mod harness;
pub mod icons;
mod inspector;
pub mod keys;
mod palette;
mod reading;
#[cfg(feature = "reference")]
pub mod reference;
pub mod shell;
pub mod theme;
pub mod tree;

pub const FIREWALL: &str = "all rune contact lives here";

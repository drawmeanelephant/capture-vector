//! cv-widgets — the ONLY crate importing `rune`/`rune-kit`.
//! All rune contact is firewalled here so the pinned dev rev can be
//! swapped without touching lanes. CI's manifest grep enforces the boundary.
//!
//! - [`shell`]: the module rail/chrome view over `cv_model::workbench`.
//! - [`theme`]: `cv_model::theme` tokens → rune colours/type/springs.
//! - [`harness`]: headless runtime, scripted input, tree/layout goldens.
//! - [`app`]: native window boot + companion windows.

pub mod app;
pub mod diagnostic;
pub mod harness;
pub mod keys;
pub mod shell;
pub mod theme;
pub mod tree;

pub const FIREWALL: &str = "all rune contact lives here";

#[allow(unused_imports)]
use rune_kit as _rune_kit;

//! cv-widgets — the ONLY crate importing `rune`/`rune-kit`.
//! All rune contact is firewalled here so the pinned dev rev can be
//! swapped without touching lanes. `cargo-deny` enforces the boundary.

/// Marker proving the dependency edge exists — wave-0 wiring lands the
/// real widgets behind this firewall.
pub const FIREWALL: &str = "all rune contact lives here";

// Force-link the rune crates so `cargo tree`/`cargo-deny` observe the edge.
#[allow(unused_imports)]
use rune as _rune;
#[allow(unused_imports)]
use rune_kit as _rune_kit;

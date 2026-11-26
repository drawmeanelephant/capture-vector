//! cv-publish — target registry + PreflightGrant + delivery adapters (L5).
//! Every publish run ships a sealed plan; hash mismatch refuses.
//!
//! Lifecycle (all transitions are durable `.cv/` ledger records):
//!
//! ```text
//! preflight ─► GrantIssued | PreflightBlocked
//! approve   ─► Approved                       (before any dispatch)
//! publish   ─► Dispatching ─► Submitted | Unknown | Complete(status)
//! crash     ─► (reopen) Dispatching ⇒ Unknown ─► reconcile ─► Complete
//! ```
//!
//! A run is dispatched at most once. Nothing here retries: an `Unknown`
//! run blocks its target until a human reconciles it. Process work goes
//! through the `cv_core::engine::Engine` port; this crate never spawns.

pub mod adapters;
mod error;
pub mod exports;
pub mod plan;
pub mod preflight;
mod proofeval;
pub mod publisher;
pub mod secrets;
pub mod targets;

pub use error::{PublishError, Result};
pub use plan::{PlanFile, SealedPlan};
pub use preflight::{PreflightOutcome, PreflightRequest, Ruleset, RulesetVerdict};
pub use publisher::{Observation, Publisher};
pub use targets::{Target, TargetConfig, registry};

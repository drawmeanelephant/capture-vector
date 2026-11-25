//! cv-core — domain types + SYNC ports + event/state vocabulary.
//!
//! NO IO, NO async, NO UI. Ports are synchronous / channel-sink to match
//! rune's `&mut State` + wake-drain model. Normative source:
//! `spec/contracts.md` (v3). This crate is append-only after the
//! contract-hardening checkpoint.
//!
//! # Wave-0 contract pins (normative)
//!
//! - **Diagnostic span units**: `Span` offsets are **byte offsets into the
//!   UTF-8 source file exactly as stored on disk** — CRLF counts as two
//!   bytes, start is 0-based, end is half-open. `line`/`col` are 1-based
//!   display coordinates only; never feed them to the editor buffer.
//!   ropey indexes by byte, so anything else panics on multi-byte text.
//! - **Selection scope**: `Selection` is **per-window**. Companion windows
//!   get their own `Selection`; they never write each other's. A dirty
//!   buffer blocks its own window's close only.
//! - **Scale envelope**: 5k pages, diagnostics rivers (coalesced rebuild
//!   bursts), stable-ID finding dedup `(code, subject, hash(target))`.
//! - **Event cascade**: see [`events`] — build-finished → persist IR →
//!   rules re-eval → findings refresh → staleness badges.

pub mod diagnostic;
pub mod engine;
pub mod events;
pub mod id;
pub mod publish;
pub mod redact;
pub mod rules;
pub mod store;
pub mod sys;

mod error;
pub use error::{Error, Result};

//! cv-engine — the ONLY `std::process::Command` owner (L1 lane).
//! RawSubprocess adapter + EngineScheduler land in L1; see
//! `spec/lanes/L1-engine-actor.md`. Until then this crate is a stub that
//! re-exports the port — processes are spawned nowhere else.

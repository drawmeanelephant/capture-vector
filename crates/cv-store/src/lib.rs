//! cv-store — rusqlite (bundled) index + JSONL evidence trail in `.cv/`.
//! JSONL is the source of truth; SQLite (OS app-data, per project id) is
//! a rebuildable index. Store lands with the lanes that need it; the
//! `Store` port lives in `cv-core::store`.

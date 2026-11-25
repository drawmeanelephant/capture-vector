//! Migration-run adoption requests crossing from the pure Migrate module
//! to the shell/composition root. The data is plain and path-shaped;
//! validation and writes happen behind cv-engine/cv-store.

use crate::new_workspace::WorkspaceCreation;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptionTarget {
    NewWorkspace(WorkspaceCreation),
    ExistingWorkspace { subfolder: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionRequest {
    pub run_id: String,
    pub target: AdoptionTarget,
}

/// The workspace `.cv/imports/<run>` came from plus what should be adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionJob {
    pub source_root: PathBuf,
    pub request: AdoptionRequest,
}

/// A worker's completed adoption, for shell status/workspace handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionReport {
    pub workspace: PathBuf,
    pub files_written: usize,
    pub validate: String,
    pub build: String,
    pub new_workspace: bool,
}

/// Shell dialog that collects New workspace details for an adoption.
pub const ADOPT_NEW_DIALOG: &str = "shell.workspace.adopt-new";

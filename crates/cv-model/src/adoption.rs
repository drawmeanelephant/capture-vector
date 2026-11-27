//! Migration-run adoption requests crossing from the pure Migrate module
//! to the shell/composition root. The data is plain and path-shaped;
//! validation and writes happen behind cv-engine/cv-store.

use crate::new_workspace::WorkspaceCreation;
use cv_core::engine::CancelToken;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptionTarget {
    NewWorkspace(WorkspaceCreation),
    ExistingWorkspace {
        subfolder: String,
    },
    /// Apply an asset-filename utility run to the workspace it inspected.
    /// No candidate tree is copied and no subfolder is involved.
    ExistingWorkspaceInPlace,
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
    /// Stable operation identity. New-workspace adoption shares the
    /// creation token so progress cancellation and completion correlation
    /// are the same operation; existing-workspace adoption gets its own
    /// token even though it currently has no cancellation dialog.
    pub cancel: CancelToken,
}

/// A worker's completed adoption, for shell status/workspace handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionReport {
    pub workspace: PathBuf,
    pub files_written: usize,
    /// Pages in the converged adoption plan, whether this attempt wrote
    /// their bytes or a crash resume proved they had already landed.
    pub pages: usize,
    /// Non-page files in that same converged plan.
    pub assets: usize,
    /// User-facing content destination, `/`-separated and relative to the
    /// workspace (`content/imported/site`, for example).
    pub destination: String,
    pub validate: String,
    pub build: String,
    pub new_workspace: bool,
}

/// Shell dialog that collects New workspace details for an adoption.
pub const ADOPT_NEW_DIALOG: &str = "shell.workspace.adopt-new";

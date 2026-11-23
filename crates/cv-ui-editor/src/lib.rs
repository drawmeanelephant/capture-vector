//! cv-ui-editor — the editor lane's model (L7). No UI, no rune: the
//! widgets in `cv-widgets::editor` render this state.
//!
//! - [`buffer`]: ropey buffer, byte-offset edits, undo/redo, dirty state.
//! - [`save`]: fingerprinted staged saves (`*.tmp` + rename, 5 × 50 ms).
//! - [`frontmatter`]: the structured floor's form over Boris's closed keys.
//! - [`problems`]: build-tagged diagnostics, stale-dim policy, gutter.
//! - [`completion`]: wikilink completion from `completion.json`.
//! - [`sketch`]: the advisory draft-sketch source.
//! - [`session`]: N buffers.
//!
//! The rich-layer feasibility verdict lives in `crates/cv-ui-editor/RICH-PROBE.md`.

pub mod buffer;
pub mod completion;
pub mod frontmatter;
pub mod problems;
pub mod save;
pub mod session;
pub mod sketch;

pub use buffer::{Buffer, EditError};
pub use session::{BufferId, EditorSession};

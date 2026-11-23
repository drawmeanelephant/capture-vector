//! Draft sketch: the buffer's body handed to `rune_kit::Markdown` in the
//! reading pane. Advisory only — it is not Boris's rendering (no
//! wikilink resolution, includes, components, or theme), and it is not a
//! site preview. The authoritative view is Boris's next build, served to
//! the user's browser by `boris watch --serve`.

use crate::buffer::Buffer;
use crate::frontmatter;

pub const LABEL: &str = "Draft sketch — advisory";
pub const DISCLAIMER: &str = "Not Boris output. The next build is authoritative; \
     see the site in your browser via `boris watch --serve`.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sketch {
    pub revision: u64,
    pub source: String,
}

impl Sketch {
    pub fn of(buf: &Buffer) -> Self {
        let text = buf.text();
        Self {
            revision: buf.revision(),
            source: frontmatter::body(&text).to_string(),
        }
    }
}

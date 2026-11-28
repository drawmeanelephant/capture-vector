//! Graph search/filter predicates over Boris's frozen navigation facts.
//!
//! This is presentation filtering only: it does not infer relation meaning,
//! validate topology, or widen typed drift into a known trunk/satellite role.

use crate::graph::{GraphNavIndex, GraphNode, RelationDirection};
use cv_contracts::graph::NodeRole;

/// Trunk/satellite role filter. `Any` is the only non-filtering state;
/// typed `Unknown` role drift never satisfies either real role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoleFilter {
    #[default]
    Any,
    Trunk,
    Satellite,
}

impl RoleFilter {
    pub const CHOICES: [&'static str; 3] = ["any", "trunk", "satellite"];

    pub fn choice_index(self) -> usize {
        match self {
            Self::Any => 0,
            Self::Trunk => 1,
            Self::Satellite => 2,
        }
    }

    pub fn from_choice(choice: &str) -> Self {
        match choice {
            "trunk" => Self::Trunk,
            "satellite" => Self::Satellite,
            _ => Self::Any,
        }
    }

    fn matches(self, role: &NodeRole) -> bool {
        match self {
            Self::Any => true,
            Self::Trunk => role == &NodeRole::Trunk,
            Self::Satellite => role == &NodeRole::Satellite,
        }
    }
}

/// User-entered filter fields. Empty text fields do not filter; non-empty
/// fields conjoin. Matching is case-insensitive substring matching, except
/// for the closed trunk/satellite role choice.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphFilter {
    pub page_id: String,
    pub title: String,
    pub tag: String,
    pub relation_kind: String,
    pub role: RoleFilter,
}

impl GraphFilter {
    pub fn active(&self) -> bool {
        !self.page_id.trim().is_empty()
            || !self.title.trim().is_empty()
            || !self.tag.trim().is_empty()
            || !self.relation_kind.trim().is_empty()
            || self.role != RoleFilter::Any
    }

    pub fn trimmed(mut self) -> Self {
        self.page_id = self.page_id.trim().to_string();
        self.title = self.title.trim().to_string();
        self.tag = self.tag.trim().to_string();
        self.relation_kind = self.relation_kind.trim().to_string();
        self
    }

    pub fn compile(&self) -> CompiledGraphFilter {
        CompiledGraphFilter {
            page_id: self.page_id.to_lowercase(),
            title: self.title.to_lowercase(),
            tag: self.tag.to_lowercase(),
            relation_kind: self.relation_kind.to_lowercase(),
            role: self.role,
        }
    }
}

/// Case-folded filter needles, compiled once per search rather than once
/// per node at corpus scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledGraphFilter {
    page_id: String,
    title: String,
    tag: String,
    relation_kind: String,
    role: RoleFilter,
}

impl CompiledGraphFilter {
    pub fn matches(&self, index: &GraphNavIndex, node: &GraphNode<'_>) -> bool {
        contains(&node.id.to_lowercase(), &self.page_id)
            && (self.title.is_empty()
                || node
                    .title
                    .is_some_and(|title| contains(&title.to_lowercase(), &self.title)))
            && (self.tag.is_empty()
                || node
                    .tags
                    .iter()
                    .any(|tag| contains(&tag.to_lowercase(), &self.tag)))
            && (self.relation_kind.is_empty()
                || index
                    .relations_of(node.id, RelationDirection::Any)
                    .any(|relation| {
                        contains(&relation.kind.as_str().to_lowercase(), &self.relation_kind)
                    }))
            && self.role.matches(node.role)
    }
}

fn contains(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_only_text_filters_are_inactive() {
        let filter = GraphFilter {
            page_id: "  ".into(),
            title: "\t".into(),
            ..Default::default()
        };
        assert!(!filter.active());
        let trimmed = filter.trimmed();
        assert_eq!(trimmed.page_id, "");
        assert_eq!(trimmed.title, "");
    }

    #[test]
    fn unknown_role_drift_matches_neither_closed_role() {
        let role = NodeRole::Unknown("branch".into());
        assert!(!RoleFilter::Trunk.matches(&role));
        assert!(!RoleFilter::Satellite.matches(&role));
        assert!(RoleFilter::Any.matches(&role));
    }
}

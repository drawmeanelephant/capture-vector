//! Noun-view registry — the `kind → view` mapping the spec's "State &
//! identity" section requires (`spec/architecture.md`,
//! `spec/contracts.md`). Every kind `noun_kind::ALL` can mint names the
//! lane module that presents it; kinds read back from stored data that
//! are not registered render through the mandatory generic fallback —
//! the noun's `NounAttrRow` projection in a plain inspector.

use cv_core::id::NounKind;

/// How the shell presents a noun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NounView {
    /// A lane module presents it — the module id a `Link`/`Reveal`
    /// targets for nouns of this kind.
    Lane(&'static str),
    /// The generic fallback: the noun's `NounAttrRow` fields rendered
    /// by the shell's plain inspector, no dedicated lane. Mandatory —
    /// every kind, registered or not, can render this way.
    Generic,
}

/// kind → presenting lane. Holds one entry per `noun_kind::ALL` member;
/// the comprehensiveness test asserts that. The publish lane presents
/// the publish ledger's nouns (runs as `plan`, export read-backs as
/// `receipt`, deliveries, proof-pack checks and claims); `artifact`
/// nouns stay with the audit evidence lane.
const VIEWS: &[(&str, NounView)] = &[
    (
        cv_core::id::noun_kind::PAGE,
        NounView::Lane(crate::modules::pages::ID),
    ),
    (
        cv_core::id::noun_kind::FINDING,
        NounView::Lane(crate::modules::audit::ID),
    ),
    (
        cv_core::id::noun_kind::PLAN,
        NounView::Lane(crate::modules::publish::ID),
    ),
    (
        cv_core::id::noun_kind::RECEIPT,
        NounView::Lane(crate::modules::publish::ID),
    ),
    (
        cv_core::id::noun_kind::NOTE,
        NounView::Lane(crate::modules::research::ID),
    ),
    (
        cv_core::id::noun_kind::OUTPUT,
        NounView::Lane(crate::modules::outputs::ID),
    ),
    (
        cv_core::id::noun_kind::DIAGNOSTIC,
        NounView::Lane(crate::modules::pages::ID),
    ),
    (
        cv_core::id::noun_kind::EVENT,
        NounView::Lane(crate::modules::activity::ID),
    ),
    (
        cv_core::id::noun_kind::ARTIFACT,
        NounView::Lane(crate::modules::audit::ID),
    ),
    (
        cv_core::id::noun_kind::CHECK,
        NounView::Lane(crate::modules::publish::ID),
    ),
    (
        cv_core::id::noun_kind::CLAIM,
        NounView::Lane(crate::modules::publish::ID),
    ),
    (
        cv_core::id::noun_kind::DELIVERY,
        NounView::Lane(crate::modules::publish::ID),
    ),
    (
        cv_core::id::noun_kind::IMPORT_RUN,
        NounView::Lane(crate::modules::migrate::ID),
    ),
    (
        cv_core::id::noun_kind::REVIEW_ITEM,
        NounView::Lane(crate::modules::migrate::ID),
    ),
];

/// The view for `kind`; unregistered kinds — only reachable from stored
/// data — render through `NounView::Generic`.
pub fn view_for(kind: &NounKind) -> NounView {
    VIEWS
        .iter()
        .find(|(k, _)| kind.as_str() == *k)
        .map(|(_, v)| *v)
        .unwrap_or(NounView::Generic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::id::noun_kind;

    #[test]
    fn every_mintable_kind_has_a_registered_view() {
        for kind in noun_kind::ALL {
            assert!(
                matches!(view_for(&NounKind::register(kind)), NounView::Lane(_)),
                "{kind} has no registered view"
            );
        }
    }

    #[test]
    fn an_unknown_kind_renders_through_the_generic_fallback() {
        // Unregistered kinds cannot be minted; they arrive via stored data.
        let unknown: NounKind = serde_json::from_str("\"from-a-newer-build\"").unwrap();
        assert_eq!(view_for(&unknown), NounView::Generic);
    }

    #[test]
    fn registered_views_name_real_modules() {
        let registry = crate::module::Registry::with_all();
        for (_, v) in VIEWS {
            let NounView::Lane(id) = v else { continue };
            assert!(registry.index_of(id).is_some(), "{id} is not a module");
        }
    }
}

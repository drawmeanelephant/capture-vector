use cv_core::events::Envelope;
use cv_core::store::{Namespace, Predicate, Store};

use super::JOURNAL;

/// Highest sequence already carried by the durable `shell` local-event
/// session. `Some(0)` means the store opened and carries no shell events;
/// `None` means the store could not disclose its high-water mark.
pub(super) fn shell_high_water(store: &dyn Store) -> Option<u64> {
    let docs = store
        .query(&Namespace(JOURNAL.into()), &Predicate::default())
        .ok()?;
    Some(
        docs.into_iter()
            .filter_map(|doc| serde_json::from_value::<Envelope>(doc).ok())
            .filter(|env| env.cursor.session.0 == "shell")
            .map(|env| env.cursor.seq)
            .max()
            .unwrap_or(0),
    )
}

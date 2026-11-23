use super::*;

#[test]
fn edits_are_byte_addressed_and_refuse_mid_char() {
    let mut b = Buffer::open("p.md", "héllo\r\nworld", None);
    assert_eq!(b.len_bytes(), 13);
    assert_eq!(b.replace(2..3, "x"), Err(EditError::NotCharBoundary(2)));
    assert_eq!(
        b.replace(0..99, ""),
        Err(EditError::OutOfBounds { end: 99, len: 13 })
    );
    b.replace(1..3, "e").unwrap();
    assert_eq!(b.text(), "hello\r\nworld");
    assert_eq!(b.line_of_byte(7), 1);
    assert_eq!(b.line_range(0), Some(0..5));
    assert_eq!(b.line_range(1), Some(7..12));
}

#[test]
fn typing_coalesces_and_undo_redo_round_trip_dirty_state() {
    let mut b = Buffer::open("p.md", "ab", None);
    assert!(!b.is_dirty());
    for (i, c) in "xyz".chars().enumerate() {
        b.insert(1 + i, &c.to_string()).unwrap();
    }
    assert_eq!(b.text(), "axyzb");
    assert!(b.is_dirty());
    assert!(b.undo());
    assert_eq!(b.text(), "ab");
    assert!(!b.is_dirty(), "undo back to the saved revision is clean");
    assert!(!b.can_undo());
    assert!(b.redo());
    assert_eq!(b.text(), "axyzb");
    assert!(b.is_dirty());
}

#[test]
fn newline_and_boundary_split_undo_steps() {
    let mut b = Buffer::open("p.md", "", None);
    b.insert(0, "a").unwrap();
    b.insert(1, "\n").unwrap();
    b.insert(2, "b").unwrap();
    b.break_undo_group();
    b.insert(3, "c").unwrap();
    b.undo();
    assert_eq!(b.text(), "a\nb");
    b.undo();
    assert_eq!(b.text(), "a\n");
    b.undo();
    assert_eq!(b.text(), "a");
}

#[test]
fn save_point_is_an_undo_boundary() {
    let fp = Fingerprint {
        sha256: cv_core::sys::Hash([0; 32]),
        mtime: cv_core::sys::Timestamp(1),
    };
    let mut b = Buffer::open("p.md", "", None);
    b.insert(0, "a").unwrap();
    b.mark_saved(b.revision(), fp);
    assert!(!b.is_dirty());
    b.insert(1, "b").unwrap();
    assert!(b.is_dirty());
    b.undo();
    assert_eq!(b.text(), "a");
    assert!(!b.is_dirty());
    b.undo();
    assert_eq!(b.text(), "");
    assert!(
        b.is_dirty(),
        "undoing past the save point differs from disk"
    );
}

#[test]
fn set_text_folds_widget_strings_into_minimal_edits() {
    let mut b = Buffer::open("p.md", "see [[a]] ok", None);
    b.set_text("see [[abc]] ok").unwrap();
    assert_eq!(b.text(), "see [[abc]] ok");
    assert_eq!(diff_range("aé", "aè"), (1, 3, 3));
    assert_eq!(diff_range("aaa", "aa"), (2, 3, 2));
    b.undo();
    assert_eq!(b.text(), "see [[a]] ok");
}

#[test]
fn redo_is_dropped_by_a_new_edit() {
    let mut b = Buffer::open("p.md", "x", None);
    b.insert(1, "\ny").unwrap();
    b.undo();
    b.insert(1, "z").unwrap();
    assert!(!b.redo());
    assert_eq!(b.text(), "xz");
}

//! cv-model — AppState slices + AppEvent + cascade dispatcher + pure
//! *Model builders per lane directory. NO rune, NO IO — millisecond tests.
//! Lane-owned module dirs: `modules/{pages,outputs,activity,audit,publish,research}`.

pub mod app_state;
pub mod cascade;

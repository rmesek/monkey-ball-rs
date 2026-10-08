//! Game-flow rules for SMB1, independent of rendering and windowing (std only, strictly offline).
//!
//! * [`course`]: the reference's course scripts and their interpreter (`course.ts`).
//! * [`run`]: [`run::CourseRun`], the lives / continues / score / banana state machine fed by
//!   [`run::StageOutcome`]s; practice mode.
//! * [`progress`]: unlocks, best scores and times, user settings, and their text save format.
//! * [`storage`]: the [`storage::Storage`] trait with file and in-memory backends.
//! * [`play`]: the menu <-> gameplay interface ([`play::PlayRequest`], [`play::PlayReport`]).

pub mod course;
pub mod play;
pub mod progress;
pub mod run;
mod scripts;
pub mod stages;
pub mod storage;

mod bg_table;

pub use course::{Course, Difficulty, FloorEntry};
pub use play::{PlayMode, PlayReport, PlayRequest, PlayResult};
pub use progress::{Profile, Progress, Settings};
pub use run::{CourseRun, Floor, GoalType, Rules, StageOutcome, Step};
pub use storage::{FileStorage, MemoryStorage, SlotStorage, Storage};

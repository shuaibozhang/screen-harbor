mod output;
mod recording;

pub use output::{OutputConfig, OutputFormat, OutputResolution, OutputSettings, Resolution};
pub use recording::{RecordingAction, RecordingState, RecordingStateMachine, StateError};

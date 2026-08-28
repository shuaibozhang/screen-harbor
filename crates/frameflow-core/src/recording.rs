use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Idle,
    Selecting,
    Ready,
    Countdown,
    Recording,
    Paused,
    Finalizing,
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingAction {
    BeginSelection,
    CancelSelection,
    CancelCapture,
    ConfirmSelection,
    StartCountdown,
    CountdownFinished,
    Pause,
    Resume,
    Stop,
    Finalized,
    ProcessingCompleted,
    Fail,
    Reset,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("action {action:?} is not allowed while recording is {state:?}")]
pub struct StateError {
    pub state: RecordingState,
    pub action: RecordingAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordingStateMachine {
    state: RecordingState,
}

impl Default for RecordingStateMachine {
    fn default() -> Self {
        Self {
            state: RecordingState::Idle,
        }
    }
}

impl RecordingStateMachine {
    pub fn state(&self) -> RecordingState {
        self.state
    }

    pub fn apply(&mut self, action: RecordingAction) -> Result<RecordingState, StateError> {
        let next = match (self.state, action) {
            (RecordingState::Idle, RecordingAction::BeginSelection) => RecordingState::Selecting,
            (
                RecordingState::Selecting | RecordingState::Ready,
                RecordingAction::CancelSelection,
            ) => RecordingState::Idle,
            (
                RecordingState::Countdown | RecordingState::Recording | RecordingState::Paused,
                RecordingAction::CancelCapture,
            ) => RecordingState::Idle,
            (RecordingState::Selecting, RecordingAction::ConfirmSelection) => RecordingState::Ready,
            (RecordingState::Ready, RecordingAction::StartCountdown) => RecordingState::Countdown,
            (RecordingState::Countdown, RecordingAction::CountdownFinished) => {
                RecordingState::Recording
            }
            (RecordingState::Recording, RecordingAction::Pause) => RecordingState::Paused,
            (RecordingState::Paused, RecordingAction::Resume) => RecordingState::Recording,
            (RecordingState::Recording | RecordingState::Paused, RecordingAction::Stop) => {
                RecordingState::Finalizing
            }
            (RecordingState::Finalizing, RecordingAction::Finalized) => RecordingState::Processing,
            (RecordingState::Processing, RecordingAction::ProcessingCompleted) => {
                RecordingState::Completed
            }
            (_, RecordingAction::Fail) => RecordingState::Failed,
            (RecordingState::Completed | RecordingState::Failed, RecordingAction::Reset) => {
                RecordingState::Idle
            }
            (state, action) => return Err(StateError { state, action }),
        };

        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_the_happy_path() {
        let mut machine = RecordingStateMachine::default();
        let actions = [
            RecordingAction::BeginSelection,
            RecordingAction::ConfirmSelection,
            RecordingAction::StartCountdown,
            RecordingAction::CountdownFinished,
            RecordingAction::Pause,
            RecordingAction::Resume,
            RecordingAction::Stop,
            RecordingAction::Finalized,
            RecordingAction::ProcessingCompleted,
        ];

        for action in actions {
            machine.apply(action).unwrap();
        }

        assert_eq!(machine.state(), RecordingState::Completed);
    }

    #[test]
    fn rejects_an_illegal_transition_without_mutating_state() {
        let mut machine = RecordingStateMachine::default();
        let error = machine.apply(RecordingAction::Pause).unwrap_err();

        assert_eq!(error.state, RecordingState::Idle);
        assert_eq!(machine.state(), RecordingState::Idle);
    }

    #[test]
    fn failure_can_be_reported_from_any_state_and_reset() {
        let mut machine = RecordingStateMachine::default();
        machine.apply(RecordingAction::BeginSelection).unwrap();
        machine.apply(RecordingAction::Fail).unwrap();
        machine.apply(RecordingAction::Reset).unwrap();

        assert_eq!(machine.state(), RecordingState::Idle);
    }

    #[test]
    fn selection_can_be_cancelled_before_countdown() {
        let mut machine = RecordingStateMachine::default();
        machine.apply(RecordingAction::BeginSelection).unwrap();
        machine.apply(RecordingAction::ConfirmSelection).unwrap();
        machine.apply(RecordingAction::CancelSelection).unwrap();
        assert_eq!(machine.state(), RecordingState::Idle);
    }

    #[test]
    fn countdown_cancellation_returns_directly_to_idle() {
        let mut machine = RecordingStateMachine::default();
        machine.apply(RecordingAction::BeginSelection).unwrap();
        machine.apply(RecordingAction::ConfirmSelection).unwrap();
        machine.apply(RecordingAction::StartCountdown).unwrap();
        machine.apply(RecordingAction::CancelCapture).unwrap();
        assert_eq!(machine.state(), RecordingState::Idle);
    }

    #[test]
    fn capture_preview_can_exit_without_claiming_a_completed_file() {
        let mut machine = RecordingStateMachine::default();
        machine.apply(RecordingAction::BeginSelection).unwrap();
        machine.apply(RecordingAction::ConfirmSelection).unwrap();
        machine.apply(RecordingAction::StartCountdown).unwrap();
        machine.apply(RecordingAction::CountdownFinished).unwrap();
        machine.apply(RecordingAction::Pause).unwrap();
        machine.apply(RecordingAction::CancelCapture).unwrap();
        assert_eq!(machine.state(), RecordingState::Idle);
    }
}

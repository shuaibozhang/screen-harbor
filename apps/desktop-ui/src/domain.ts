export type RecordingState =
  | "idle"
  | "selecting"
  | "ready"
  | "countdown"
  | "recording"
  | "paused"
  | "finalizing"
  | "processing"
  | "completed"
  | "failed";

export type OutputStatus = "pending" | "processing" | "completed" | "failed";

export interface RecentRecording {
  id: string;
  name: string;
  durationSeconds: number;
  createdLabel: string;
  mainStatus: OutputStatus;
  previewStatus: OutputStatus;
}

export interface AppSnapshot {
  recordingState: RecordingState;
  saveDirectory: string;
  microphone: { enabled: boolean; deviceName?: string; available: boolean };
  systemAudio: { enabled: boolean; deviceName?: string; available: boolean };
  outputSummary: string;
  captureFps: number;
  supportedCaptureFps: number[];
  monitorRefreshRateHz?: number;
  recommendedCaptureFps: number;
  recentRecordings: RecentRecording[];
}

export interface RegionSelection {
  displayId: string;
  desktopRect: { x: number; y: number; width: number; height: number };
  captureRect: { x: number; y: number; width: number; height: number };
}

export interface CaptureStats {
  frameCount: number;
  elapsedMs: number;
  paused: boolean;
  lastError?: string;
  audio?: {
    microphone: LiveAudioInput;
    systemAudio: LiveAudioInput;
    warning?: string;
  };
}

export interface LiveAudioInput {
  enabled: boolean;
  muted: boolean;
  disconnected: boolean;
}

export interface AudioWarningEvent {
  source: "microphone" | "systemAudio";
  message: string;
}

export interface FinalizationEvent {
  success: boolean;
  outputPath?: string;
  error?: string;
}

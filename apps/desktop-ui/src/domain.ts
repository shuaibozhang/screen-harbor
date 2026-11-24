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

export type OutputStatus = "not_requested" | "pending" | "processing" | "completed" | "failed";
export type MainFormat = "mp4" | "webm";
export type VideoQuality = "low" | "balanced" | "high";
export type OutputFormat = "mp4" | "webm" | "animated_webp";

export interface RecentRecording {
  id: string;
  name: string;
  durationSeconds?: number;
  createdLabel: string;
  mainStatus: OutputStatus;
  mainFormat: MainFormat;
  previewStatus: OutputStatus;
  coverStatus: OutputStatus;
  coverPath?: string;
}

export interface InterruptedRecording {
  id: string;
  name: string;
  createdLabel: string;
  fileSizeBytes: number;
}

export interface RecordingArtifact {
  status: OutputStatus;
  path?: string;
  fileSizeBytes?: number;
  width?: number;
  height?: number;
  framesPerSecond?: number;
  error?: string;
}

export interface RecordingDetails {
  id: string;
  name: string;
  createdLabel: string;
  durationSeconds?: number;
  main: RecordingArtifact;
  preview: RecordingArtifact;
  cover: RecordingArtifact;
  highResolutionCover: RecordingArtifact;
}

export interface AppSnapshot {
  recordingState: RecordingState;
  saveDirectory: string;
  saveDirectoryAvailableBytes?: number;
  globalShortcutAvailable: boolean;
  microphone: AudioInputSnapshot;
  systemAudio: AudioInputSnapshot;
  outputSummary: string;
  captureFps: number;
  mainFormat: MainFormat;
  mainQuality: VideoQuality;
  outputResolution: OutputResolution;
  previewEnabled: boolean;
  previewFormat: OutputFormat;
  previewResolution: OutputResolution;
  previewFps: number;
  supportedCaptureFps: number[];
  supportedPreviewFps: number[];
  monitorRefreshRateHz?: number;
  recommendedCaptureFps: number;
  countdownEnabled: boolean;
  countdownSeconds: number;
  recentRecordings: RecentRecording[];
  interruptedRecordings: InterruptedRecording[];
}

export interface AudioDevice {
  id: string;
  name: string;
  isDefault: boolean;
}

export interface AudioInputSnapshot {
  enabled: boolean;
  deviceName?: string;
  available: boolean;
  selectedDeviceId?: string;
  devices: AudioDevice[];
}

export interface DisplayChangeEvent {
  selectionCancelled: boolean;
}

export type OutputResolution =
  | { mode: "native" }
  | { mode: "fitWithin"; width: number; height: number };

export interface RegionSelection {
  displayId: string;
  desktopRect: { x: number; y: number; width: number; height: number };
  captureRect: { x: number; y: number; width: number; height: number };
}

export interface RecordingPreferences {
  countdownEnabled: boolean;
  countdownSeconds: number;
}

export interface CaptureStats {
  frameCount: number;
  droppedFrames: number;
  effectiveFps: number;
  elapsedMs: number;
  paused: boolean;
  videoEncoder: {
    name: string;
    hardwareAccelerated?: boolean;
  };
  lastError?: string;
  audio?: {
    microphone: LiveAudioInput;
    systemAudio: LiveAudioInput;
    droppedPackets: number;
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

export interface StorageWarningEvent {
  health: "healthy" | "low" | "critical" | "unavailable";
  availableBytes?: number;
  message?: string;
  autoStopping: boolean;
}

export interface FinalizationEvent {
  success: boolean;
  recordingId?: string;
  outputPath?: string;
  error?: string;
}

import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshot, OutputFormat, OutputResolution } from "./domain";

const developmentSnapshot: AppSnapshot = {
  recordingState: "idle",
  saveDirectory: "D:\\FrameFlow\\Recordings",
  saveDirectoryAvailableBytes: 128 * 1024 * 1024 * 1024,
  globalShortcutAvailable: true,
  microphone: { enabled: true, deviceName: "默认麦克风", available: true },
  systemAudio: { enabled: false, deviceName: "扬声器（默认设备）", available: true },
  outputSummary: "母版 MP4 · H.264/AAC · 原始分辨率 · 30 FPS · 系统声 / 预览 WebM · 最高 854×480 · 15 FPS",
  captureFps: 30,
  outputResolution: { mode: "native" },
  previewFormat: "webm",
  previewResolution: { mode: "fitWithin", width: 854, height: 480 },
  supportedCaptureFps: [24, 30, 60],
  monitorRefreshRateHz: 60,
  recommendedCaptureFps: 60,
  captureDisplays: [
    { id: "\\\\.\\DISPLAY1", label: "屏幕 1（主屏幕）", width: 3840, height: 2160, scalePercent: 175, primary: true, refreshRateHz: 60 },
    { id: "\\\\.\\DISPLAY2", label: "屏幕 2", width: 1920, height: 1080, scalePercent: 100, primary: false, refreshRateHz: 60 },
  ],
  selectedCaptureDisplayId: "\\\\.\\DISPLAY1",
  recentRecordings: [],
  interruptedRecordings: [],
};

export const isTauriRuntime = () => "__TAURI_INTERNALS__" in window;

export async function getAppSnapshot(): Promise<AppSnapshot> {
  return isTauriRuntime() ? invoke<AppSnapshot>("get_app_snapshot") : developmentSnapshot;
}

export async function beginSelection(): Promise<void> {
  if (isTauriRuntime()) await invoke("begin_selection");
}

export async function chooseSaveDirectory(): Promise<string | null> {
  return isTauriRuntime() ? invoke<string | null>("choose_save_directory") : null;
}

export async function setCaptureDisplay(displayId: string): Promise<void> {
  if (isTauriRuntime()) await invoke("set_capture_display", { displayId });
}

export async function setOutputSettings(
  fps: number,
  resolution: OutputResolution,
  previewFormat: OutputFormat,
  previewResolution: OutputResolution,
): Promise<void> {
  if (isTauriRuntime()) await invoke("set_output_settings", { fps, resolution, previewFormat, previewResolution });
}

export async function setAudioInput(
  source: "microphone" | "systemAudio",
  enabled: boolean,
): Promise<void> {
  if (isTauriRuntime()) await invoke("set_audio_input", { source, enabled });
}

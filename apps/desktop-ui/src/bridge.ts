import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshot, OutputFormat, OutputResolution } from "./domain";

const developmentSnapshot: AppSnapshot = {
  recordingState: "idle",
  saveDirectory: "D:\\ScreenHarbor\\Recordings",
  saveDirectoryAvailableBytes: 128 * 1024 * 1024 * 1024,
  globalShortcutAvailable: true,
  microphone: {
    enabled: true,
    deviceName: "阵列麦克风",
    available: true,
    devices: [
      { id: "development-mic", name: "阵列麦克风", isDefault: true },
      { id: "development-usb-mic", name: "USB 麦克风", isDefault: false },
    ],
  },
  systemAudio: {
    enabled: false,
    deviceName: "扬声器",
    available: true,
    devices: [
      { id: "development-speakers", name: "扬声器", isDefault: true },
      { id: "development-headphones", name: "蓝牙耳机", isDefault: false },
    ],
  },
  outputSummary: "母版 MP4 · H.264/AAC · 原始分辨率 · 30 FPS · 系统声 / 预览 WebM · 最高 854×480 · 15 FPS",
  captureFps: 30,
  outputResolution: { mode: "native" },
  previewEnabled: true,
  previewFormat: "webm",
  previewResolution: { mode: "fitWithin", width: 854, height: 480 },
  previewFps: 15,
  supportedCaptureFps: [24, 30, 60],
  supportedPreviewFps: [10, 15, 24, 30],
  monitorRefreshRateHz: 60,
  recommendedCaptureFps: 60,
  countdownEnabled: false,
  countdownSeconds: 3,
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

export async function setOutputSettings(
  fps: number,
  resolution: OutputResolution,
  previewFormat: OutputFormat,
  previewResolution: OutputResolution,
  previewFps: number,
): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("set_output_settings", { fps, resolution, previewFormat, previewResolution, previewFps });
    return;
  }
  developmentSnapshot.captureFps = fps;
  developmentSnapshot.outputResolution = resolution;
  developmentSnapshot.previewFormat = previewFormat;
  developmentSnapshot.previewResolution = previewResolution;
  developmentSnapshot.previewFps = previewFps;
}

export async function setAudioInput(
  source: "microphone" | "systemAudio",
  enabled: boolean,
  deviceId?: string,
): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("set_audio_input", { source, enabled, deviceId: deviceId ?? null });
    return;
  }
  developmentSnapshot[source].enabled = enabled;
  developmentSnapshot[source].selectedDeviceId = deviceId;
  const selected = deviceId
    ? developmentSnapshot[source].devices.find((device) => device.id === deviceId)
    : developmentSnapshot[source].devices.find((device) => device.isDefault);
  developmentSnapshot[source].deviceName = selected?.name;
  developmentSnapshot[source].available = selected !== undefined;
}

export async function setPreviewEnabled(enabled: boolean): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("set_preview_enabled", { enabled });
    return;
  }
  developmentSnapshot.previewEnabled = enabled;
}

export async function setRecordingPreferences(
  countdownEnabled: boolean,
  countdownSeconds: number,
): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("set_recording_preferences", { countdownEnabled, countdownSeconds });
    return;
  }
  developmentSnapshot.countdownEnabled = countdownEnabled;
  developmentSnapshot.countdownSeconds = countdownSeconds;
}

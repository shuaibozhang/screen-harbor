import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshot, OutputResolution } from "./domain";

const developmentSnapshot: AppSnapshot = {
  recordingState: "idle",
  saveDirectory: "D:\\FrameFlow\\Recordings",
  microphone: { enabled: true, deviceName: "默认麦克风", available: true },
  systemAudio: { enabled: false, deviceName: "扬声器（默认设备）", available: true },
  outputSummary: "MP4 · 有声音 / WebM · 480p · 有声音 / 封面",
  captureFps: 30,
  outputResolution: { mode: "native" },
  supportedCaptureFps: [24, 30, 60],
  monitorRefreshRateHz: 60,
  recommendedCaptureFps: 60,
  recentRecordings: [],
};

export const isTauriRuntime = () => "__TAURI_INTERNALS__" in window;

export async function getAppSnapshot(): Promise<AppSnapshot> {
  return isTauriRuntime() ? invoke<AppSnapshot>("get_app_snapshot") : developmentSnapshot;
}

export async function beginSelection(): Promise<void> {
  if (isTauriRuntime()) await invoke("begin_selection");
}

export async function setOutputSettings(
  fps: number,
  resolution: OutputResolution,
): Promise<void> {
  if (isTauriRuntime()) await invoke("set_output_settings", { fps, resolution });
}

export async function setAudioInput(
  source: "microphone" | "systemAudio",
  enabled: boolean,
): Promise<void> {
  if (isTauriRuntime()) await invoke("set_audio_input", { source, enabled });
}

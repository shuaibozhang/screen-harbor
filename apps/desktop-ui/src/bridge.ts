import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshot } from "./domain";

const developmentSnapshot: AppSnapshot = {
  recordingState: "idle",
  saveDirectory: "D:\\FrameFlow\\Recordings",
  microphone: { enabled: true, deviceName: "默认麦克风", available: true },
  systemAudio: { enabled: true, available: true },
  outputSummary: "MP4 · 有声音 / WebM · 480p · 有声音 / 封面",
  captureFps: 30,
  supportedCaptureFps: [24, 30, 60],
  monitorRefreshRateHz: 60,
  recommendedCaptureFps: 60,
  recentRecordings: [],
};

const isTauri = () => "__TAURI_INTERNALS__" in window;

export async function getAppSnapshot(): Promise<AppSnapshot> {
  return isTauri() ? invoke<AppSnapshot>("get_app_snapshot") : developmentSnapshot;
}

export async function beginSelection(): Promise<void> {
  if (isTauri()) await invoke("begin_selection");
}

export async function setCaptureFps(fps: number): Promise<number> {
  return isTauri() ? invoke<number>("set_capture_fps", { fps }) : fps;
}

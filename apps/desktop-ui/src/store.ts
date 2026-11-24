import { create } from "zustand";
import { beginSelection, chooseSaveDirectory, getAppSnapshot, setAudioInput, setOutputSettings, setPreviewEnabled, setRecordingPreferences } from "./bridge";
import type { AppSnapshot, MainFormat, VideoQuality, OutputFormat, OutputResolution } from "./domain";
import { currentLocale, translate } from "./i18n";

const t = (source: string, values?: Record<string, string | number>) => translate(currentLocale(), source, values);

interface AppStore {
  snapshot: AppSnapshot | null;
  loading: boolean;
  error: string | null;
  hydrate: () => Promise<void>;
  startSelection: () => Promise<void>;
  updateOutputSettings: (fps: number, resolution: OutputResolution, previewFormat: OutputFormat, previewResolution: OutputResolution, previewFps: number, mainFormat: MainFormat, mainQuality: VideoQuality) => Promise<void>;
  updateAudioInput: (source: "microphone" | "systemAudio", enabled: boolean, deviceId?: string) => Promise<void>;
  updatePreviewEnabled: (enabled: boolean) => Promise<void>;
  updateRecordingPreferences: (countdownEnabled: boolean, countdownSeconds: number) => Promise<void>;
  updateSaveDirectory: () => Promise<boolean>;
}

export const useAppStore = create<AppStore>((set, get) => ({
  snapshot: null,
  loading: true,
  error: null,
  hydrate: async () => {
    // Keep an already-rendered screen in place while background events refresh
    // the snapshot. The full loading shell is only for the first launch.
    set({ loading: get().snapshot === null, error: null });
    try {
      set({ snapshot: await getAppSnapshot(), loading: false });
    } catch {
      set({ error: t("无法读取应用状态，请重新启动 ScreenHarbor。"), loading: false });
    }
  },
  startSelection: async () => {
    try {
      await beginSelection();
    } catch {
      set({ error: t("暂时无法进入框选，请稍后重试。") });
    }
  },
  updateOutputSettings: async (fps, resolution, previewFormat, previewResolution, previewFps, mainFormat, mainQuality) => {
    try {
      await setOutputSettings(fps, resolution, previewFormat, previewResolution, previewFps, mainFormat, mainQuality);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: t("无法保存输出设置：{error}", { error: String(cause) }) });
      throw cause;
    }
  },
  updateAudioInput: async (source, enabled, deviceId) => {
    try {
      await setAudioInput(source, enabled, deviceId);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: t("无法更新音频输入：{error}", { error: String(cause) }) });
      throw cause;
    }
  },
  updatePreviewEnabled: async (enabled) => {
    try {
      await setPreviewEnabled(enabled);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: t("无法更新轻量预览：{error}", { error: String(cause) }) });
      throw cause;
    }
  },
  updateRecordingPreferences: async (countdownEnabled, countdownSeconds) => {
    try {
      await setRecordingPreferences(countdownEnabled, countdownSeconds);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: t("无法更新录制倒计时：{error}", { error: String(cause) }) });
      throw cause;
    }
  },
  updateSaveDirectory: async () => {
    try {
      const selected = await chooseSaveDirectory();
      if (selected === null) return false;
      set({ snapshot: await getAppSnapshot(), error: null });
      return true;
    } catch (cause) {
      set({ error: t("无法更改保存位置：{error}", { error: String(cause) }) });
      throw cause;
    }
  },
}));

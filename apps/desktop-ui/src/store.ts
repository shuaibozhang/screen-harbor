import { create } from "zustand";
import { beginSelection, chooseSaveDirectory, getAppSnapshot, setAudioInput, setCaptureDisplay, setOutputSettings } from "./bridge";
import type { AppSnapshot, OutputFormat, OutputResolution } from "./domain";

interface AppStore {
  snapshot: AppSnapshot | null;
  loading: boolean;
  error: string | null;
  hydrate: () => Promise<void>;
  startSelection: () => Promise<void>;
  updateOutputSettings: (fps: number, resolution: OutputResolution, previewFormat: OutputFormat, previewResolution: OutputResolution) => Promise<void>;
  updateAudioInput: (source: "microphone" | "systemAudio", enabled: boolean) => Promise<void>;
  updateCaptureDisplay: (displayId: string) => Promise<void>;
  updateSaveDirectory: () => Promise<boolean>;
}

export const useAppStore = create<AppStore>((set) => ({
  snapshot: null,
  loading: true,
  error: null,
  hydrate: async () => {
    set({ loading: true, error: null });
    try {
      set({ snapshot: await getAppSnapshot(), loading: false });
    } catch {
      set({ error: "无法读取应用状态，请重新启动 FrameFlow。", loading: false });
    }
  },
  startSelection: async () => {
    try {
      await beginSelection();
    } catch {
      set({ error: "暂时无法进入框选，请稍后重试。" });
    }
  },
  updateOutputSettings: async (fps, resolution, previewFormat, previewResolution) => {
    try {
      await setOutputSettings(fps, resolution, previewFormat, previewResolution);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: `无法保存输出设置：${String(cause)}` });
      throw cause;
    }
  },
  updateAudioInput: async (source, enabled) => {
    try {
      await setAudioInput(source, enabled);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: `无法更新音频输入：${String(cause)}` });
      throw cause;
    }
  },
  updateCaptureDisplay: async (displayId) => {
    try {
      await setCaptureDisplay(displayId);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: `无法切换录制屏幕：${String(cause)}` });
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
      set({ error: `无法更改保存位置：${String(cause)}` });
      throw cause;
    }
  },
}));

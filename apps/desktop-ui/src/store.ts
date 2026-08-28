import { create } from "zustand";
import { beginSelection, getAppSnapshot, setAudioInput, setCaptureFps } from "./bridge";
import type { AppSnapshot } from "./domain";

interface AppStore {
  snapshot: AppSnapshot | null;
  loading: boolean;
  error: string | null;
  hydrate: () => Promise<void>;
  startSelection: () => Promise<void>;
  updateCaptureFps: (fps: number) => Promise<void>;
  updateAudioInput: (source: "microphone" | "systemAudio", enabled: boolean) => Promise<void>;
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
  updateCaptureFps: async (fps) => {
    try {
      await setCaptureFps(fps);
      set({ snapshot: await getAppSnapshot(), error: null });
    } catch (cause) {
      set({ error: `无法保存帧率设置：${String(cause)}` });
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
}));

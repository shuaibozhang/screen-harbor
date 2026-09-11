import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Play, RotateCcw, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { RecordingPreferences, RecordingState, RegionSelection } from "./domain";
import { useI18n } from "./i18n";
import "./selection-controls.css";

export function SelectionControls() {
  const { t } = useI18n();
  const [selection, setSelection] = useState<RegionSelection | null>(null);
  const [phase, setPhase] = useState<RecordingState>("idle");
  const [countdown, setCountdown] = useState(3);
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    countdownEnabled: false,
    countdownSeconds: 3,
  });
  const [error, setError] = useState<string | null>(null);
  const [drawing, setDrawing] = useState(false);

  useEffect(() => {
    document.documentElement.dataset.window = "selection-controls";
    const refreshInitialState = async () => {
      const [currentSelection, currentPreferences] = await Promise.all([
        invoke<RegionSelection | null>("get_current_selection"),
        invoke<RecordingPreferences>("get_recording_preferences"),
      ]);
      setSelection(currentSelection);
      setPreferences(currentPreferences);
      setCountdown(currentPreferences.countdownSeconds);
    };
    void refreshInitialState().catch(() => undefined);
    let disposed = false;
    let unlistenSelection: (() => void) | undefined;
    let unlistenSelectionStarted: (() => void) | undefined;
    let unlistenState: (() => void) | undefined;
    void listen<RegionSelection>("selection://confirmed", (event) => {
      setSelection(event.payload);
      setDrawing(false);
      setError(null);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenSelection = dispose;
    });
    void listen("selection://started", () => {
      setSelection(null);
      setDrawing(true);
      setError(null);
      void invoke<RecordingPreferences>("get_recording_preferences")
        .then((currentPreferences) => {
          setPreferences(currentPreferences);
          setCountdown(currentPreferences.countdownSeconds);
        })
        .catch(() => undefined);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenSelectionStarted = dispose;
    });
    void listen<RecordingState>("recording://state-changed", (event) => {
      setPhase(event.payload);
      if (event.payload === "idle") {
        setSelection(null);
        setDrawing(false);
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenState = dispose;
    });
    return () => {
      disposed = true;
      unlistenSelection?.();
      unlistenSelectionStarted?.();
      unlistenState?.();
    };
  }, []);

  useEffect(() => {
    if (phase !== "countdown" || !preferences.countdownEnabled) return;
    let cancelled = false;
    const run = async () => {
      for (let value = preferences.countdownSeconds; value >= 1; value -= 1) {
        if (cancelled) return;
        setCountdown(value);
        await new Promise((resolve) => window.setTimeout(resolve, 1000));
      }
      if (cancelled) return;
      try {
        await invoke("start_capture");
      } catch (cause) {
        setError(t("无法启动录制：{error}", { error: String(cause) }));
        await invoke("cancel_countdown").catch(() => undefined);
      }
    };
    void run();
    return () => { cancelled = true; };
  }, [phase, preferences.countdownEnabled, preferences.countdownSeconds, t]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      void invoke(phase === "countdown" ? "cancel_countdown" : "cancel_selection");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [phase]);

  const start = async () => {
    try {
      await invoke("start_countdown");
      if (!preferences.countdownEnabled) {
        await invoke("start_capture");
      }
      setError(null);
    } catch (cause) {
      setError(String(cause));
    }
  };
  const isDrawing = phase === "selecting" || drawing;

  return (
    <main className="selection-controls-shell">
      <div className="selection-controls-bar">
        <strong>
          {isDrawing
            ? t("智能选择区域")
            : phase === "countdown"
            ? preferences.countdownEnabled ? t("{countdown} 秒后开始", { countdown }) : t("正在开始")
            : selection
              ? `${selection.captureRect.width} × ${selection.captureRect.height}`
              : t("等待选区")}
        </strong>
        <span>
          {isDrawing
            ? t("单击选窗口 · 拖动自由框选 · Esc 取消")
            : phase === "countdown"
              ? preferences.countdownEnabled ? t("Esc 取消") : t("正在启动录制")
              : t("拖边框缩放 · 顶部手柄移动")}
        </span>
        {phase !== "countdown" && !isDrawing && (
          <button onClick={() => void invoke("redraw_selection")}>
            <RotateCcw size={15} />{t("重新框选")}
          </button>
        )}
        {!isDrawing && (
          <button className="primary" disabled={!selection || phase === "countdown"} onClick={() => void start()}>
            <Play size={15} />{t("开始录制")}
          </button>
        )}
        <button className="icon" aria-label={t("取消")} onClick={() => void invoke(phase === "countdown" ? "cancel_countdown" : "cancel_selection")}>
          <X size={16} />
        </button>
      </div>
      {error && <div className="selection-controls-error">{error}</div>}
    </main>
  );
}

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Play, RotateCcw, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { RecordingState, RegionSelection } from "./domain";
import "./selection-controls.css";

export function SelectionControls() {
  const [selection, setSelection] = useState<RegionSelection | null>(null);
  const [phase, setPhase] = useState<RecordingState>("idle");
  const [countdown, setCountdown] = useState(3);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    document.documentElement.dataset.window = "selection-controls";
    void invoke<RegionSelection | null>("get_current_selection")
      .then(setSelection)
      .catch(() => undefined);
    let disposed = false;
    let unlistenSelection: (() => void) | undefined;
    let unlistenState: (() => void) | undefined;
    void listen<RegionSelection>("selection://confirmed", (event) => {
      setSelection(event.payload);
      setError(null);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenSelection = dispose;
    });
    void listen<RecordingState>("recording://state-changed", (event) => {
      setPhase(event.payload);
      if (event.payload === "idle") setSelection(null);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenState = dispose;
    });
    return () => {
      disposed = true;
      unlistenSelection?.();
      unlistenState?.();
    };
  }, []);

  useEffect(() => {
    if (phase !== "countdown") return;
    let cancelled = false;
    const run = async () => {
      for (const value of [3, 2, 1]) {
        if (cancelled) return;
        setCountdown(value);
        await new Promise((resolve) => window.setTimeout(resolve, 850));
      }
      if (cancelled) return;
      try {
        await invoke("start_capture");
      } catch (cause) {
        setError(`无法启动录制：${String(cause)}`);
        await invoke("cancel_countdown").catch(() => undefined);
      }
    };
    void run();
    return () => { cancelled = true; };
  }, [phase]);

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
      setError(null);
    } catch (cause) {
      setError(String(cause));
    }
  };

  return (
    <main className="selection-controls-shell">
      <div className="selection-controls-bar">
        <strong>
          {phase === "countdown"
            ? `${countdown} 秒后开始`
            : selection
              ? `${selection.captureRect.width} × ${selection.captureRect.height}`
              : "等待选区"}
        </strong>
        <span>{phase === "countdown" ? "Esc 取消" : "拖边框缩放 · 顶部手柄移动"}</span>
        {phase !== "countdown" && (
          <button onClick={() => void invoke("redraw_selection")}>
            <RotateCcw size={15} />重新框选
          </button>
        )}
        <button className="primary" disabled={!selection || phase === "countdown"} onClick={() => void start()}>
          <Play size={15} />开始录制
        </button>
        <button className="icon" aria-label="取消" onClick={() => void invoke(phase === "countdown" ? "cancel_countdown" : "cancel_selection")}>
          <X size={16} />
        </button>
      </div>
      {error && <div className="selection-controls-error">{error}</div>}
    </main>
  );
}

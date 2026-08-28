import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Pause, Play, Square } from "lucide-react";
import { useEffect, useState } from "react";
import type { CaptureStats, RecordingState } from "./domain";
import "./recording-controls.css";

function formatElapsed(milliseconds: number) {
  const totalSeconds = Math.floor(milliseconds / 1000);
  const minutes = Math.floor(totalSeconds / 60).toString().padStart(2, "0");
  const seconds = (totalSeconds % 60).toString().padStart(2, "0");
  return `${minutes}:${seconds}`;
}

export function RecordingControls() {
  const [phase, setPhase] = useState<RecordingState>("idle");
  const [stats, setStats] = useState<CaptureStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const stopping = phase === "finalizing";

  useEffect(() => {
    document.documentElement.dataset.window = "recording-controls";
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<RecordingState>("recording://state-changed", (event) => {
      setPhase(event.payload);
      if (event.payload === "recording") {
        setStats(null);
        setError(null);
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (phase !== "recording" && phase !== "paused") return;
    const refresh = () => {
      void invoke<CaptureStats>("get_capture_stats")
        .then((next) => {
          setStats(next);
          setPhase(next.paused ? "paused" : "recording");
        })
        .catch(() => undefined);
    };
    refresh();
    const timer = window.setInterval(refresh, 250);
    return () => window.clearInterval(timer);
  }, [phase]);

  const togglePause = async () => {
    try {
      await invoke(phase === "paused" ? "resume_capture" : "pause_capture");
      setError(null);
    } catch {
      setError("无法切换暂停状态");
    }
  };

  const stopAndSave = async () => {
    if (stopping) return;
    setError(null);
    try {
      await invoke<RecordingState>("request_stop_capture");
    } catch (cause) {
      setError(`保存录制失败：${String(cause)}`);
    }
  };

  return (
    <main className="recording-controls-shell">
      <div className="recording-controls-bar">
        <span className="recording-state"><i />{stopping ? "正在保存" : phase === "paused" ? "已暂停" : "录制中"}</span>
        <strong>{formatElapsed(stats?.elapsedMs ?? 0)}</strong>
        <small>{stats?.frameCount ?? 0} 帧 · H.264 MP4</small>
        <button disabled={stopping} onClick={() => void togglePause()}>
          {phase === "paused" ? <Play size={15} /> : <Pause size={15} />}
          {phase === "paused" ? "继续" : "暂停"}
        </button>
        <button className="finish" disabled={stopping} onClick={() => void stopAndSave()}>
          <Square size={14} />{stopping ? "保存中…" : "停止并保存"}
        </button>
      </div>
      {(error || stats?.lastError) && <div className="recording-controls-error">{error ?? stats?.lastError}</div>}
    </main>
  );
}

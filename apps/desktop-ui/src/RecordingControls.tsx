import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Mic, MicOff, Pause, Play, Square, Volume2, VolumeX } from "lucide-react";
import { useEffect, useState } from "react";
import type { AudioWarningEvent, CaptureStats, LiveAudioInput, RecordingState } from "./domain";
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
  const [audioWarning, setAudioWarning] = useState<string | null>(null);
  const stopping = phase === "finalizing";

  useEffect(() => {
    document.documentElement.dataset.window = "recording-controls";
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenAudio: (() => void) | undefined;
    let unlistenExitBlocked: (() => void) | undefined;
    void Promise.all([
      listen<RecordingState>("recording://state-changed", (event) => {
        setPhase(event.payload);
        if (event.payload === "recording") {
          setStats(null);
          setError(null);
          setAudioWarning(null);
        }
      }),
      listen<AudioWarningEvent>("recording://audio-warning", (event) => {
        setAudioWarning(event.payload.message);
      }),
      listen<string>("recording://exit-blocked", (event) => {
        setError(event.payload);
      }),
    ]).then(async ([disposeState, disposeAudio, disposeExitBlocked]) => {
      if (disposed) {
        disposeState();
        disposeAudio();
        disposeExitBlocked();
        return;
      }
      unlisten = disposeState;
      unlistenAudio = disposeAudio;
      unlistenExitBlocked = disposeExitBlocked;
      try {
        const current = await invoke<RecordingState>("get_recording_state");
        if (!disposed) setPhase(current);
      } catch (cause) {
        if (!disposed) setError(`无法读取录制状态：${String(cause)}`);
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenAudio?.();
      unlistenExitBlocked?.();
    };
  }, []);

  useEffect(() => {
    if (phase !== "recording" && phase !== "paused") return;
    let disposed = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const next = await invoke<CaptureStats>("get_capture_stats");
        if (disposed) return;
        setStats(next);
        setPhase(next.paused ? "paused" : "recording");
        setError(null);
      } catch (cause) {
        if (disposed) return;
        setError(`录制进程状态不可用：${String(cause)}`);
        return;
      }
      if (!disposed) timer = window.setTimeout(refresh, 250);
    };
    void refresh();
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
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

  const toggleMute = async (
    source: "microphone" | "systemAudio",
    input: LiveAudioInput,
  ) => {
    try {
      await invoke("set_capture_audio_muted", { source, muted: !input.muted });
      setStats((current) => current?.audio ? {
        ...current,
        audio: {
          ...current.audio,
          [source]: { ...input, muted: !input.muted },
        },
      } : current);
      setError(null);
    } catch (cause) {
      setError(`无法切换音频静音：${String(cause)}`);
    }
  };

  const systemAudio = stats?.audio?.systemAudio;
  const microphone = stats?.audio?.microphone;
  const encoderLabel = stats?.videoEncoder.hardwareAccelerated === true
    ? "硬件 H.264"
    : stats?.videoEncoder.hardwareAccelerated === false
      ? "软件 H.264"
      : "H.264";

  return (
    <main className="recording-controls-shell">
      <div className="recording-controls-bar">
        <span className="recording-state"><i />{stopping ? "正在保存" : phase === "paused" ? "已暂停" : "录制中"}</span>
        <strong>{formatElapsed(stats?.elapsedMs ?? 0)}</strong>
        <small title={stats?.videoEncoder.name}>
          {stats?.frameCount ?? 0} 帧 · {(stats?.effectiveFps ?? 0).toFixed(1)} FPS
          {(stats?.droppedFrames ?? 0) > 0 ? ` · 丢 ${stats?.droppedFrames} 帧` : ""}
          {(stats?.audio?.droppedPackets ?? 0) > 0 ? ` · 音频丢 ${stats?.audio?.droppedPackets} 包` : ""}
          {` · ${encoderLabel}${stats?.audio ? "/AAC" : ""} MP4`}
        </small>
        {systemAudio?.enabled && (
          <button className={`audio-control ${systemAudio.muted ? "muted" : ""}`} disabled={stopping || systemAudio.disconnected} title={systemAudio.disconnected ? "系统声音设备已断开" : systemAudio.muted ? "恢复系统声音" : "静音系统声音"} onClick={() => void toggleMute("systemAudio", systemAudio)}>
            {systemAudio.muted || systemAudio.disconnected ? <VolumeX size={15} /> : <Volume2 size={15} />}
            系统声
          </button>
        )}
        {microphone?.enabled && (
          <button className={`audio-control ${microphone.muted ? "muted" : ""}`} disabled={stopping || microphone.disconnected} title={microphone.disconnected ? "麦克风已断开" : microphone.muted ? "恢复麦克风" : "静音麦克风"} onClick={() => void toggleMute("microphone", microphone)}>
            {microphone.muted || microphone.disconnected ? <MicOff size={15} /> : <Mic size={15} />}
            麦克风
          </button>
        )}
        <button disabled={stopping} onClick={() => void togglePause()}>
          {phase === "paused" ? <Play size={15} /> : <Pause size={15} />}
          {phase === "paused" ? "继续" : "暂停"}
        </button>
        <button className="finish" disabled={stopping} onClick={() => void stopAndSave()}>
          <Square size={14} />{stopping ? "保存中…" : "停止并保存"}
        </button>
      </div>
      {(error || stats?.lastError || audioWarning || stats?.audio?.warning) && <div className={`recording-controls-error ${!error && !stats?.lastError ? "warning" : ""}`}>{error ?? stats?.lastError ?? audioWarning ?? stats?.audio?.warning}</div>}
    </main>
  );
}

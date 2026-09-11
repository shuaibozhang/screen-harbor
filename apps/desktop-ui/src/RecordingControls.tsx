import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Mic, MicOff, Pause, Play, Square, Volume2, VolumeX } from "lucide-react";
import { useEffect, useState } from "react";
import type { AudioWarningEvent, CaptureStats, LiveAudioInput, RecordingState, StorageWarningEvent } from "./domain";
import { useI18n } from "./i18n";
import "./recording-controls.css";

function formatElapsed(milliseconds: number) {
  const totalSeconds = Math.floor(milliseconds / 1000);
  const minutes = Math.floor(totalSeconds / 60).toString().padStart(2, "0");
  const seconds = (totalSeconds % 60).toString().padStart(2, "0");
  return `${minutes}:${seconds}`;
}

export function RecordingControls() {
  const { t } = useI18n();
  const [phase, setPhase] = useState<RecordingState>("idle");
  const [stats, setStats] = useState<CaptureStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [audioWarning, setAudioWarning] = useState<string | null>(null);
  const [storageWarning, setStorageWarning] = useState<StorageWarningEvent | null>(null);
  const stopping = phase === "finalizing";

  useEffect(() => {
    document.documentElement.dataset.window = "recording-controls";
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenAudio: (() => void) | undefined;
    let unlistenExitBlocked: (() => void) | undefined;
    let unlistenStorage: (() => void) | undefined;
    void Promise.all([
      listen<RecordingState>("recording://state-changed", (event) => {
        setPhase(event.payload);
        if (event.payload === "recording") {
          setStats(null);
          setError(null);
          setAudioWarning(null);
          setStorageWarning(null);
        }
      }),
      listen<AudioWarningEvent>("recording://audio-warning", (event) => {
        setAudioWarning(event.payload.message);
      }),
      listen<string>("recording://exit-blocked", (event) => {
        setError(event.payload);
      }),
      listen<StorageWarningEvent>("recording://storage-warning", (event) => {
        setStorageWarning(event.payload.health === "healthy" ? null : event.payload);
      }),
    ]).then(async ([disposeState, disposeAudio, disposeExitBlocked, disposeStorage]) => {
      if (disposed) {
        disposeState();
        disposeAudio();
        disposeExitBlocked();
        disposeStorage();
        return;
      }
      unlisten = disposeState;
      unlistenAudio = disposeAudio;
      unlistenExitBlocked = disposeExitBlocked;
      unlistenStorage = disposeStorage;
      try {
        const current = await invoke<RecordingState>("get_recording_state");
        if (!disposed) setPhase(current);
      } catch (cause) {
        if (!disposed) setError(t("无法读取录制状态：{error}", { error: String(cause) }));
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenAudio?.();
      unlistenExitBlocked?.();
      unlistenStorage?.();
    };
  }, [t]);

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
        setError(t("录制进程状态不可用：{error}", { error: String(cause) }));
        return;
      }
      if (!disposed) timer = window.setTimeout(refresh, 250);
    };
    void refresh();
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [phase, t]);

  const togglePause = async () => {
    try {
      await invoke(phase === "paused" ? "resume_capture" : "pause_capture");
      setError(null);
    } catch {
      setError(t("无法切换暂停状态"));
    }
  };

  const stopAndSave = async () => {
    if (stopping) return;
    setError(null);
    try {
      await invoke<RecordingState>("request_stop_capture");
    } catch (cause) {
      setError(t("保存录制失败：{error}", { error: String(cause) }));
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
      setError(t("无法切换音频静音：{error}", { error: String(cause) }));
    }
  };

  const systemAudio = stats?.audio?.systemAudio;
  const microphone = stats?.audio?.microphone;
  const encoderLabel = stats?.videoEncoder.hardwareAccelerated === true
    ? t("硬件 H.264")
    : stats?.videoEncoder.hardwareAccelerated === false
      ? t("软件 H.264")
      : "H.264";
  const statsLabel = [
    t("{count} 帧", { count: stats?.frameCount ?? 0 }),
    `${(stats?.effectiveFps ?? 0).toFixed(1)} FPS`,
    (stats?.droppedFrames ?? 0) > 0 ? t("丢 {count} 帧", { count: stats?.droppedFrames ?? 0 }) : null,
    (stats?.audio?.droppedPackets ?? 0) > 0
      ? t("音频丢 {count} 包", { count: stats?.audio?.droppedPackets ?? 0 })
      : null,
    `${encoderLabel}${stats?.audio ? "/AAC" : ""} MP4`,
  ].filter(Boolean).join(" · ");
  const statusMessage = error
    ?? stats?.lastError
    ?? storageWarning?.message
    ?? audioWarning
    ?? stats?.audio?.warning;
  const statusIsWarning = !error
    && !stats?.lastError
    && storageWarning?.health !== "critical"
    && storageWarning?.health !== "unavailable";

  return (
    <main className="recording-controls-shell">
      <div className="recording-controls-bar">
        <span className="recording-state">{t(stopping ? "正在保存" : phase === "paused" ? "已暂停" : "录制中")}</span>
        <strong>{formatElapsed(stats?.elapsedMs ?? 0)}</strong>
        <small
          className={statusMessage ? `recording-controls-message${statusIsWarning ? " warning" : ""}` : undefined}
          title={statusMessage ?? `${statsLabel}${stats?.videoEncoder.name ? ` · ${stats.videoEncoder.name}` : ""}`}
        >
          {statusMessage ?? statsLabel}
        </small>
        {systemAudio?.enabled && (
          <button className={`audio-control ${systemAudio.muted ? "muted" : ""}`} disabled={stopping || systemAudio.disconnected} title={t(systemAudio.disconnected ? "系统声音设备已断开" : systemAudio.muted ? "恢复系统声音" : "静音系统声音")} onClick={() => void toggleMute("systemAudio", systemAudio)}>
            {systemAudio.muted || systemAudio.disconnected ? <VolumeX size={15} /> : <Volume2 size={15} />}
            {t("系统声")}
          </button>
        )}
        {microphone?.enabled && (
          <button className={`audio-control ${microphone.muted ? "muted" : ""}`} disabled={stopping || microphone.disconnected} title={t(microphone.disconnected ? "麦克风已断开" : microphone.muted ? "恢复麦克风" : "静音麦克风")} onClick={() => void toggleMute("microphone", microphone)}>
            {microphone.muted || microphone.disconnected ? <MicOff size={15} /> : <Mic size={15} />}
            {t("麦克风")}
          </button>
        )}
        <button disabled={stopping} onClick={() => void togglePause()}>
          {phase === "paused" ? <Play size={15} /> : <Pause size={15} />}
          {t(phase === "paused" ? "继续" : "暂停")}
        </button>
        <button className="finish" disabled={stopping} onClick={() => void stopAndSave()}>
          <Square size={14} />{t(stopping ? "保存中…" : "停止并保存")}
        </button>
      </div>
    </main>
  );
}

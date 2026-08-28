import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown,
  ChevronRight,
  Clock3,
  Folder,
  Mic2,
  MonitorSpeaker,
  Settings2,
  Video,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useAppStore } from "./store";
import type { FinalizationEvent, RecordingState } from "./domain";
import "./styles.css";

function StatusDot({ available }: { available: boolean }) {
  return <span className={available ? "status-dot ready" : "status-dot"} aria-hidden="true" />;
}

export function App() {
  const { snapshot, loading, error, hydrate, startSelection, updateCaptureFps, updateAudioInput } = useAppStore();
  const [finalizationError, setFinalizationError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [draftFps, setDraftFps] = useState(30);
  const [savingSettings, setSavingSettings] = useState(false);
  const [savingAudio, setSavingAudio] = useState<"microphone" | "systemAudio" | null>(null);
  const [showAllRecordings, setShowAllRecordings] = useState(false);
  const [openingRecording, setOpeningRecording] = useState<string | null>(null);

  useEffect(() => {
    void hydrate();
    let unlistenState: (() => void) | undefined;
    let unlistenFinalization: (() => void) | undefined;
    let disposed = false;
    void listen<RecordingState>("recording://state-changed", (event) => {
      if (event.payload === "completed" || event.payload === "idle") {
        void hydrate();
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenState = dispose;
    });
    void listen<FinalizationEvent>("recording://finalized", (event) => {
      if (!event.payload.success) {
        setFinalizationError(event.payload.error ?? "录制文件保存失败。");
      } else {
        setFinalizationError(null);
      }
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenFinalization = dispose;
    });
    return () => {
      disposed = true;
      unlistenState?.();
      unlistenFinalization?.();
    };
  }, [hydrate]);

  if (loading || !snapshot) {
    return <main className="shell loading">正在准备 FrameFlow…</main>;
  }

  const openSettings = () => {
    setDraftFps(snapshot.captureFps);
    setSettingsOpen(true);
  };

  const saveSettings = async () => {
    setSavingSettings(true);
    try {
      await updateCaptureFps(draftFps);
      setSettingsOpen(false);
    } catch {
      // The store exposes the backend error in the main alert.
    } finally {
      setSavingSettings(false);
    }
  };

  const exceedsMonitorRefresh = snapshot.monitorRefreshRateHz !== undefined
    && draftFps > snapshot.monitorRefreshRateHz + 1;

  const toggleAudio = async (source: "microphone" | "systemAudio", enabled: boolean) => {
    setSavingAudio(source);
    try {
      await updateAudioInput(source, enabled);
    } finally {
      setSavingAudio(null);
    }
  };

  const openRecordingLocation = async (recordingId: string) => {
    setOpeningRecording(recordingId);
    try {
      await invoke("open_recording_location", { recordingId });
      setFinalizationError(null);
    } catch (openError) {
      setFinalizationError(String(openError));
      await hydrate();
    } finally {
      setOpeningRecording(null);
    }
  };

  const visibleRecordings = showAllRecordings
    ? snapshot.recentRecordings
    : snapshot.recentRecordings.slice(0, 3);

  return (
    <main className="shell">
      <header className="titlebar">
        <div className="brand"><span className="brand-mark" />FrameFlow</div>
        <button className="icon-button" aria-label="打开设置" onClick={openSettings}><Settings2 size={18} /></button>
      </header>

      <div className="content">
        {(error || finalizationError) && (
          <div className="error" role="alert">{error ?? finalizationError}</div>
        )}

        <section className="hero" aria-labelledby="hero-title">
          <div>
            <p className="eyebrow">准备就绪</p>
            <h1 id="hero-title">录下重点，同时获得两个版本</h1>
            <p>轻量版快速分享，高清版安心留存。</p>
          </div>
          <div className="start-area">
            <div className="split-button">
              <button className="start-button" onClick={() => void startSelection()}>
                <span className="record-dot" />开始录屏
              </button>
              <button className="start-menu" aria-label="选择录制方式"><ChevronDown size={18} /></button>
            </div>
            <small>区域录制 · Alt + Shift + R</small>
          </div>
        </section>

        <section className="section" aria-labelledby="save-heading">
          <div className="section-heading"><h2 id="save-heading">保存位置</h2><button>更改</button></div>
          <button className="row path-row">
            <span className="row-icon"><Folder size={19} /></span>
            <span className="row-copy"><strong>{snapshot.saveDirectory}</strong><small>后续录制会按日期整理</small></span>
            <ChevronRight size={18} />
          </button>
        </section>

        <section className="section" aria-labelledby="input-heading">
          <div className="section-heading"><h2 id="input-heading">录制输入</h2></div>
          <div className="rows">
            <div className="row">
              <span className="row-icon"><Mic2 size={19} /></span>
              <span className="row-copy"><strong>麦克风</strong><small><StatusDot available={snapshot.microphone.available} />{snapshot.microphone.deviceName ?? "未找到可用麦克风"}</small></span>
              <button className={`input-toggle ${snapshot.microphone.enabled ? "enabled" : ""}`} type="button" role="switch" aria-checked={snapshot.microphone.enabled} aria-label="录制麦克风" disabled={!snapshot.microphone.available || savingAudio !== null} onClick={() => void toggleAudio("microphone", !snapshot.microphone.enabled)}><span /></button>
            </div>
            <div className="row">
              <span className="row-icon"><MonitorSpeaker size={19} /></span>
              <span className="row-copy"><strong>系统声音</strong><small><StatusDot available={snapshot.systemAudio.available} />{snapshot.systemAudio.deviceName ?? "未找到可用输出设备"}</small></span>
              <button className={`input-toggle ${snapshot.systemAudio.enabled ? "enabled" : ""}`} type="button" role="switch" aria-checked={snapshot.systemAudio.enabled} aria-label="录制系统声音" disabled={!snapshot.systemAudio.available || savingAudio !== null} onClick={() => void toggleAudio("systemAudio", !snapshot.systemAudio.enabled)}><span /></button>
            </div>
          </div>
        </section>

        <section className="section output" aria-labelledby="output-heading">
          <div className="section-heading"><h2 id="output-heading">输出</h2><button onClick={openSettings}>调整</button></div>
          <div className="row">
            <span className="row-icon"><Video size={19} /></span>
            <span className="row-copy"><strong>高清母版与轻量预览</strong><small>{snapshot.outputSummary}</small></span>
          </div>
        </section>

        <section className="section recent" aria-labelledby="recent-heading">
          <div className="section-heading"><h2 id="recent-heading">最近录制</h2>{snapshot.recentRecordings.length > 3 && <button onClick={() => setShowAllRecordings((visible) => !visible)}>{showAllRecordings ? "收起" : "查看全部"}</button>}</div>
          {snapshot.recentRecordings.length === 0 ? (
            <div className="empty"><Clock3 size={20} /><span><strong>还没有录制</strong><small>完成的内容会出现在这里</small></span></div>
          ) : (
            <div className="rows recent-list">
              {visibleRecordings.map((recording) => (
                <div className="row" key={recording.id}>
                  <span className="row-icon"><Video size={19} /></span>
                  <span className="row-copy">
                    <strong>{recording.name}</strong>
                    <small>{recording.createdLabel} · {recording.durationSeconds !== undefined ? `${recording.durationSeconds} 秒` : "时长未知"} · MP4 已保存</small>
                  </span>
                  <button className="recording-location" disabled={openingRecording !== null} onClick={() => void openRecordingLocation(recording.id)}>{openingRecording === recording.id ? "打开中…" : "打开位置"}</button>
                </div>
              ))}
            </div>
          )}
        </section>
      </div>
      {settingsOpen && (
        <div className="settings-backdrop" role="presentation" onMouseDown={(event) => {
          if (event.target === event.currentTarget && !savingSettings) setSettingsOpen(false);
        }}>
          <section className="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
            <header>
              <div><p>输出设置</p><h2 id="settings-title">录制帧率</h2></div>
              <button className="icon-button" aria-label="关闭设置" disabled={savingSettings} onClick={() => setSettingsOpen(false)}><X size={18} /></button>
            </header>
            <p className="settings-help">帧率越高，滚动和动画越流畅，同时会增加编码负载和文件大小。</p>
            <div className="monitor-rate">
              <MonitorSpeaker size={16} />
              <span>
                {snapshot.monitorRefreshRateHz !== undefined
                  ? `当前屏幕 ${snapshot.monitorRefreshRateHz} Hz · 推荐 ${snapshot.recommendedCaptureFps} FPS`
                  : `未读取到屏幕刷新率 · 推荐 ${snapshot.recommendedCaptureFps} FPS`}
              </span>
            </div>
            <div className="fps-options" role="radiogroup" aria-label="录制帧率">
              {snapshot.supportedCaptureFps.map((fps) => (
                <button
                  key={fps}
                  type="button"
                  role="radio"
                  aria-checked={draftFps === fps}
                  className={draftFps === fps ? "fps-option selected" : "fps-option"}
                  onClick={() => setDraftFps(fps)}
                >
                  <strong>{fps} FPS</strong>
                  <small>{fps === snapshot.recommendedCaptureFps ? "当前屏幕推荐" : fps === 24 ? "较小文件" : fps === 30 ? "通用流畅" : "高流畅度"}</small>
                </button>
              ))}
            </div>
            {exceedsMonitorRefresh && (
              <p className="fps-warning" role="status">该帧率高于当前屏幕刷新率，视频仍会按 {draftFps} FPS 输出，但可能包含重复帧并增加文件大小。</p>
            )}
            <footer>
              <button disabled={savingSettings} onClick={() => setSettingsOpen(false)}>取消</button>
              <button className="settings-save" disabled={savingSettings || draftFps === snapshot.captureFps} onClick={() => void saveSettings()}>
                {savingSettings ? "保存中…" : "保存设置"}
              </button>
            </footer>
          </section>
        </div>
      )}
    </main>
  );
}

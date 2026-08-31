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
import { isTauriRuntime } from "./bridge";
import { useAppStore } from "./store";
import type { FinalizationEvent, OutputResolution, RecordingState } from "./domain";
import "./styles.css";

function StatusDot({ available }: { available: boolean }) {
  return <span className={available ? "status-dot ready" : "status-dot"} aria-hidden="true" />;
}

type ResolutionChoice = "native" | "1080p" | "720p" | "custom";

function resolutionChoice(resolution: OutputResolution): ResolutionChoice {
  if (resolution.mode === "native") return "native";
  if (resolution.width === 1920 && resolution.height === 1080) return "1080p";
  if (resolution.width === 1280 && resolution.height === 720) return "720p";
  return "custom";
}

function outputResolution(
  choice: ResolutionChoice,
  customWidth: number,
  customHeight: number,
): OutputResolution {
  if (choice === "native") return { mode: "native" };
  if (choice === "1080p") return { mode: "fitWithin", width: 1920, height: 1080 };
  if (choice === "720p") return { mode: "fitWithin", width: 1280, height: 720 };
  return { mode: "fitWithin", width: customWidth, height: customHeight };
}

function sameResolution(left: OutputResolution, right: OutputResolution) {
  return left.mode === right.mode
    && (left.mode === "native" || (
      right.mode === "fitWithin"
      && left.width === right.width
      && left.height === right.height
    ));
}

export function App() {
  const { snapshot, loading, error, hydrate, startSelection, updateOutputSettings, updateAudioInput } = useAppStore();
  const [finalizationError, setFinalizationError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [draftFps, setDraftFps] = useState(30);
  const [draftResolution, setDraftResolution] = useState<ResolutionChoice>("native");
  const [customWidth, setCustomWidth] = useState(1920);
  const [customHeight, setCustomHeight] = useState(1080);
  const [savingSettings, setSavingSettings] = useState(false);
  const [savingAudio, setSavingAudio] = useState<"microphone" | "systemAudio" | null>(null);
  const [showAllRecordings, setShowAllRecordings] = useState(false);
  const [openingRecording, setOpeningRecording] = useState<string | null>(null);

  useEffect(() => {
    void hydrate();
    if (!isTauriRuntime()) return;
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

  if (loading) {
    return <main className="shell loading">正在准备 FrameFlow…</main>;
  }

  if (!snapshot) {
    return (
      <main className="shell loading">
        <div className="startup-error" role="alert">
          <strong>FrameFlow 初始化失败</strong>
          <span>{error ?? "无法读取应用状态。"}</span>
          <button type="button" onClick={() => void hydrate()}>重新加载</button>
        </div>
      </main>
    );
  }

  const openSettings = () => {
    setDraftFps(snapshot.captureFps);
    const choice = resolutionChoice(snapshot.outputResolution);
    setDraftResolution(choice);
    if (snapshot.outputResolution.mode === "fitWithin" && choice === "custom") {
      setCustomWidth(snapshot.outputResolution.width);
      setCustomHeight(snapshot.outputResolution.height);
    }
    setSettingsOpen(true);
  };

  const saveSettings = async () => {
    setSavingSettings(true);
    try {
      await updateOutputSettings(
        draftFps,
        outputResolution(draftResolution, customWidth, customHeight),
      );
      setSettingsOpen(false);
    } catch {
      // The store exposes the backend error in the main alert.
    } finally {
      setSavingSettings(false);
    }
  };

  const exceedsMonitorRefresh = snapshot.monitorRefreshRateHz !== undefined
    && draftFps > snapshot.monitorRefreshRateHz + 1;
  const draftOutputResolution = outputResolution(draftResolution, customWidth, customHeight);
  const invalidCustomResolution = draftResolution === "custom"
    && (!Number.isInteger(customWidth) || !Number.isInteger(customHeight) || customWidth < 2 || customHeight < 2);
  const settingsUnchanged = draftFps === snapshot.captureFps
    && sameResolution(draftOutputResolution, snapshot.outputResolution);

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
              <div><p>输出设置</p><h2 id="settings-title">视频</h2></div>
              <button className="icon-button" aria-label="关闭设置" disabled={savingSettings} onClick={() => setSettingsOpen(false)}><X size={18} /></button>
            </header>
            <p className="settings-help">原始分辨率保留选区的全部物理像素；缩放输出只在编码阶段生效，不改变选区范围。</p>
            <div className="setting-group">
              <h3>输出分辨率</h3>
              <div className="resolution-options" role="radiogroup" aria-label="输出分辨率">
                {([
                  ["native", "原始", "最佳画质"],
                  ["1080p", "1080p", "最高 1920×1080"],
                  ["720p", "720p", "最高 1280×720"],
                  ["custom", "自定义", "保持选区比例"],
                ] as const).map(([value, label, description]) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={draftResolution === value}
                    className={draftResolution === value ? "resolution-option selected" : "resolution-option"}
                    onClick={() => setDraftResolution(value)}
                  >
                    <strong>{label}</strong>
                    <small>{description}</small>
                  </button>
                ))}
              </div>
              {draftResolution === "custom" && (
                <div className="custom-resolution">
                  <label>最大宽度<input type="number" min="2" step="2" value={customWidth} onChange={(event) => setCustomWidth(Number.isFinite(event.currentTarget.valueAsNumber) ? event.currentTarget.valueAsNumber : 0)} /></label>
                  <span>×</span>
                  <label>最大高度<input type="number" min="2" step="2" value={customHeight} onChange={(event) => setCustomHeight(Number.isFinite(event.currentTarget.valueAsNumber) ? event.currentTarget.valueAsNumber : 0)} /></label>
                </div>
              )}
            </div>
            <div className="setting-group">
              <h3>录制帧率</h3>
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
            </div>
            <footer>
              <button disabled={savingSettings} onClick={() => setSettingsOpen(false)}>取消</button>
              <button className="settings-save" disabled={savingSettings || invalidCustomResolution || settingsUnchanged} onClick={() => void saveSettings()}>
                {savingSettings ? "保存中…" : "保存设置"}
              </button>
            </footer>
          </section>
        </div>
      )}
    </main>
  );
}

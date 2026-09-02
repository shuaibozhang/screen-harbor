import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  MonitorSpeaker,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { isTauriRuntime } from "./bridge";
import { HomeScreen } from "./HomeScreen";
import { useAppStore } from "./store";
import type { DisplayChangeEvent, FinalizationEvent, InterruptedRecording, OutputFormat, OutputResolution, RecordingState } from "./domain";
import "./styles.css";

type ResolutionChoice = "native" | "1080p" | "720p" | "custom";
type PreviewResolutionChoice = "480p" | "360p" | "240p" | "custom";

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

function previewResolutionChoice(resolution: OutputResolution): PreviewResolutionChoice {
  if (resolution.mode === "fitWithin") {
    if (resolution.width === 854 && resolution.height === 480) return "480p";
    if (resolution.width === 640 && resolution.height === 360) return "360p";
    if (resolution.width === 426 && resolution.height === 240) return "240p";
  }
  return "custom";
}

function previewResolution(choice: PreviewResolutionChoice, customWidth: number, customHeight: number): OutputResolution {
  if (choice === "480p") return { mode: "fitWithin", width: 854, height: 480 };
  if (choice === "360p") return { mode: "fitWithin", width: 640, height: 360 };
  if (choice === "240p") return { mode: "fitWithin", width: 426, height: 240 };
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
  const { snapshot, loading, error, hydrate, startSelection, updateOutputSettings, updateAudioInput, updatePreviewEnabled, updateRecordingPreferences, updateSaveDirectory } = useAppStore();
  const [finalizationError, setFinalizationError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [draftFps, setDraftFps] = useState(30);
  const [draftResolution, setDraftResolution] = useState<ResolutionChoice>("native");
  const [customWidth, setCustomWidth] = useState(1920);
  const [customHeight, setCustomHeight] = useState(1080);
  const [draftPreviewFormat, setDraftPreviewFormat] = useState<OutputFormat>("mp4");
  const [draftPreviewResolution, setDraftPreviewResolution] = useState<PreviewResolutionChoice>("480p");
  const [draftPreviewFps, setDraftPreviewFps] = useState(15);
  const [customPreviewWidth, setCustomPreviewWidth] = useState(854);
  const [customPreviewHeight, setCustomPreviewHeight] = useState(480);
  const [draftCountdownEnabled, setDraftCountdownEnabled] = useState(false);
  const [draftCountdownSeconds, setDraftCountdownSeconds] = useState(3);
  const [savingSettings, setSavingSettings] = useState(false);
  const [savingAudio, setSavingAudio] = useState<"microphone" | "systemAudio" | null>(null);
  const [savingPreview, setSavingPreview] = useState(false);
  const [showAllRecordings, setShowAllRecordings] = useState(false);
  const [openingRecording, setOpeningRecording] = useState<string | null>(null);
  const [recordingQuery, setRecordingQuery] = useState("");
  const [selectedRecordingIds, setSelectedRecordingIds] = useState<Set<string>>(new Set());
  const [deleteConfirmation, setDeleteConfirmation] = useState<{ ids: string[]; names: string[] } | null>(null);
  const [deletingRecordings, setDeletingRecordings] = useState(false);
  const [changingSaveDirectory, setChangingSaveDirectory] = useState(false);
  const [interruptedDeleteConfirmation, setInterruptedDeleteConfirmation] = useState<InterruptedRecording | null>(null);
  const [deletingInterrupted, setDeletingInterrupted] = useState(false);

  useEffect(() => {
    void hydrate();
    if (!isTauriRuntime()) return;
    let unlistenState: (() => void) | undefined;
    let unlistenFinalization: (() => void) | undefined;
    let unlistenLibrary: (() => void) | undefined;
    let unlistenDisplays: (() => void) | undefined;
    let unlistenShortcutError: (() => void) | undefined;
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
    void listen("recording://library-changed", () => {
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenLibrary = dispose;
    });
    void listen<DisplayChangeEvent>("display://changed", (event) => {
      if (event.payload.selectionCancelled) {
        setFinalizationError("显示器配置发生变化，本轮框选已安全取消。请重新开始框选。");
      }
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenDisplays = dispose;
    });
    void listen<string>("recording://shortcut-error", (event) => {
      setFinalizationError(`快捷键无法开始框选：${event.payload}`);
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenShortcutError = dispose;
    });
    return () => {
      disposed = true;
      unlistenState?.();
      unlistenFinalization?.();
      unlistenLibrary?.();
      unlistenDisplays?.();
      unlistenShortcutError?.();
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
    setDraftPreviewFormat(snapshot.previewFormat);
    setDraftPreviewFps(snapshot.previewFps);
    const previewChoice = previewResolutionChoice(snapshot.previewResolution);
    setDraftPreviewResolution(previewChoice);
    if (snapshot.previewResolution.mode === "fitWithin" && previewChoice === "custom") {
      setCustomPreviewWidth(snapshot.previewResolution.width);
      setCustomPreviewHeight(snapshot.previewResolution.height);
    }
    setDraftCountdownEnabled(snapshot.countdownEnabled);
    setDraftCountdownSeconds(snapshot.countdownSeconds);
    setSettingsOpen(true);
  };

  const changeSaveDirectory = async () => {
    setChangingSaveDirectory(true);
    try {
      await updateSaveDirectory();
    } catch {
      // The store exposes the native validation error in the page-level alert.
    } finally {
      setChangingSaveDirectory(false);
    }
  };

  const saveSettings = async () => {
    setSavingSettings(true);
    try {
      await updateOutputSettings(
        draftFps,
        outputResolution(draftResolution, customWidth, customHeight),
        draftPreviewFormat,
        previewResolution(draftPreviewResolution, customPreviewWidth, customPreviewHeight),
        draftPreviewFps,
      );
      await updateRecordingPreferences(draftCountdownEnabled, draftCountdownSeconds);
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
  const draftPreviewOutputResolution = previewResolution(draftPreviewResolution, customPreviewWidth, customPreviewHeight);
  const invalidCustomPreviewResolution = draftPreviewResolution === "custom"
    && (!Number.isInteger(customPreviewWidth) || !Number.isInteger(customPreviewHeight)
      || customPreviewWidth < 2 || customPreviewHeight < 2 || customPreviewWidth > 3840 || customPreviewHeight > 2160);
  const settingsUnchanged = draftFps === snapshot.captureFps
    && sameResolution(draftOutputResolution, snapshot.outputResolution)
    && draftPreviewFormat === snapshot.previewFormat
    && sameResolution(draftPreviewOutputResolution, snapshot.previewResolution)
    && draftPreviewFps === snapshot.previewFps
    && draftCountdownEnabled === snapshot.countdownEnabled
    && draftCountdownSeconds === snapshot.countdownSeconds;

  const toggleAudio = async (source: "microphone" | "systemAudio", enabled: boolean) => {
    setSavingAudio(source);
    try {
      await updateAudioInput(source, enabled);
    } finally {
      setSavingAudio(null);
    }
  };

  const togglePreview = async (enabled: boolean) => {
    setSavingPreview(true);
    try {
      await updatePreviewEnabled(enabled);
    } finally {
      setSavingPreview(false);
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

  const toggleRecordingSelection = (recordingId: string) => {
    setSelectedRecordingIds((current) => {
      const next = new Set(current);
      if (next.has(recordingId)) next.delete(recordingId);
      else next.add(recordingId);
      return next;
    });
  };

  const requestRecordingDeletion = (ids: string[]) => {
    const recordings = snapshot.recentRecordings.filter((recording) => ids.includes(recording.id));
    if (recordings.length !== ids.length) {
      setFinalizationError("部分录制已经不在当前录制库中，请刷新后重试。");
      return;
    }
    setDeleteConfirmation({ ids, names: recordings.map((recording) => recording.name) });
  };

  const confirmRecordingDeletion = async () => {
    if (!deleteConfirmation) return;
    setDeletingRecordings(true);
    try {
      await invoke("delete_recordings", { recordingIds: deleteConfirmation.ids });
      setSelectedRecordingIds((current) => {
        const next = new Set(current);
        deleteConfirmation.ids.forEach((id) => next.delete(id));
        return next;
      });
      setDeleteConfirmation(null);
      setFinalizationError(null);
      await hydrate();
    } catch (deleteError) {
      setFinalizationError(String(deleteError));
    } finally {
      setDeletingRecordings(false);
    }
  };

  const openInterruptedLocation = async (recordingId: string) => {
    try {
      await invoke("open_interrupted_recording_location", { recordingId });
      setFinalizationError(null);
    } catch (openError) {
      setFinalizationError(String(openError));
      await hydrate();
    }
  };

  const confirmInterruptedDeletion = async () => {
    if (!interruptedDeleteConfirmation) return;
    setDeletingInterrupted(true);
    try {
      await invoke("delete_interrupted_recording", {
        recordingId: interruptedDeleteConfirmation.id,
      });
      setInterruptedDeleteConfirmation(null);
      setFinalizationError(null);
      await hydrate();
    } catch (deleteError) {
      setFinalizationError(String(deleteError));
    } finally {
      setDeletingInterrupted(false);
    }
  };

  return (
    <>
      <HomeScreen
        snapshot={snapshot}
        alert={error ?? finalizationError}
        savingAudio={savingAudio}
        savingPreview={savingPreview}
        changingSaveDirectory={changingSaveDirectory}
        showAllRecordings={showAllRecordings}
        recordingQuery={recordingQuery}
        selectedRecordingIds={selectedRecordingIds}
        openingRecording={openingRecording}
        deletingRecordings={deletingRecordings}
        onOpenSettings={openSettings}
        onStartSelection={() => void startSelection()}
        onChangeSaveDirectory={() => void changeSaveDirectory()}
        onToggleAudio={(source, enabled) => void toggleAudio(source, enabled)}
        onTogglePreview={(enabled) => void togglePreview(enabled)}
        onShowAllRecordings={setShowAllRecordings}
        onRecordingQueryChange={setRecordingQuery}
        onToggleRecordingSelection={toggleRecordingSelection}
        onSetVisibleSelection={(recordingIds, selected) => {
          setSelectedRecordingIds((current) => {
            const next = new Set(current);
            recordingIds.forEach((recordingId) => {
              if (selected) next.add(recordingId);
              else next.delete(recordingId);
            });
            return next;
          });
        }}
        onRequestRecordingDeletion={requestRecordingDeletion}
        onOpenRecordingLocation={(recordingId) => void openRecordingLocation(recordingId)}
        onOpenInterruptedLocation={(recordingId) => void openInterruptedLocation(recordingId)}
        onRequestInterruptedDeletion={setInterruptedDeleteConfirmation}
      />
      {deleteConfirmation && (
        <div className="settings-backdrop" role="presentation" onMouseDown={(event) => {
          if (event.target === event.currentTarget && !deletingRecordings) setDeleteConfirmation(null);
        }}>
          <section className="delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-title" aria-describedby="delete-description">
            <span className="delete-dialog-icon"><Trash2 size={22} /></span>
            <h2 id="delete-title">移入 Windows 回收站？</h2>
            <p id="delete-description">{deleteConfirmation.ids.length === 1 ? deleteConfirmation.names[0] : `${deleteConfirmation.ids.length} 条录制`}及其母版、预览和两张封面会一起移入回收站，之后仍可恢复。</p>
            <footer>
              <button type="button" disabled={deletingRecordings} onClick={() => setDeleteConfirmation(null)}>取消</button>
              <button className="delete-confirm" type="button" disabled={deletingRecordings} onClick={() => void confirmRecordingDeletion()}>{deletingRecordings ? "正在移动…" : "移入回收站"}</button>
            </footer>
          </section>
        </div>
      )}
      {interruptedDeleteConfirmation && (
        <div className="settings-backdrop" role="presentation" onMouseDown={(event) => {
          if (event.target === event.currentTarget && !deletingInterrupted) setInterruptedDeleteConfirmation(null);
        }}>
          <section className="delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="interrupted-delete-title" aria-describedby="interrupted-delete-description">
            <span className="delete-dialog-icon"><Trash2 size={22} /></span>
            <h2 id="interrupted-delete-title">移除未完成录制？</h2>
            <p id="interrupted-delete-description">{interruptedDeleteConfirmation.name} 尚未形成可播放 MP4，其整个录制组会移入 Windows 回收站，之后仍可恢复。</p>
            <footer>
              <button type="button" disabled={deletingInterrupted} onClick={() => setInterruptedDeleteConfirmation(null)}>取消</button>
              <button className="delete-confirm" type="button" disabled={deletingInterrupted} onClick={() => void confirmInterruptedDeletion()}>{deletingInterrupted ? "正在移动…" : "移入回收站"}</button>
            </footer>
          </section>
        </div>
      )}
      {settingsOpen && (
        <div className="settings-backdrop" role="presentation" onMouseDown={(event) => {
          if (event.target === event.currentTarget && !savingSettings) setSettingsOpen(false);
        }}>
          <section className="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
            <header>
              <div><p>录制设置</p><h2 id="settings-title">视频</h2></div>
              <button className="icon-button" aria-label="关闭设置" disabled={savingSettings} onClick={() => setSettingsOpen(false)}><X size={18} /></button>
            </header>
            <p className="settings-help">原始分辨率保留选区的全部物理像素；缩放输出只在编码阶段生效，不改变选区范围。</p>
            <div className="setting-group countdown-settings">
              <div className="countdown-setting-head">
                <span>
                  <h3>开始倒计时</h3>
                  <small>关闭后，确认选区会立即开始录制</small>
                </span>
                <button
                  className={`settings-switch${draftCountdownEnabled ? " enabled" : ""}`}
                  type="button"
                  role="switch"
                  aria-checked={draftCountdownEnabled}
                  aria-label="录制开始倒计时"
                  onClick={() => setDraftCountdownEnabled((enabled) => !enabled)}
                ><span /></button>
              </div>
              {draftCountdownEnabled && (
                <div className="countdown-options" role="radiogroup" aria-label="倒计时秒数">
                  {[3, 5, 10].map((seconds) => (
                    <button
                      key={seconds}
                      type="button"
                      role="radio"
                      aria-checked={draftCountdownSeconds === seconds}
                      className={draftCountdownSeconds === seconds ? "selected" : ""}
                      onClick={() => setDraftCountdownSeconds(seconds)}
                    >{seconds} 秒</button>
                  ))}
                </div>
              )}
            </div>
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
                  onClick={() => {
                    setDraftFps(fps);
                    if (draftPreviewFps > fps) setDraftPreviewFps(fps);
                  }}
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
            <div className="setting-group preview-settings">
              <h3>轻量预览格式</h3>
              <div className="format-options" role="radiogroup" aria-label="轻量预览格式">
                {([
                  ["mp4", "MP4", "H.264 + AAC，兼容性最好"],
                  ["webm", "WebM", "VP9 + Opus，通常文件更小"],
                  ["animated_webp", "WebP", "动态图片，无音频"],
                ] as const).map(([value, label, description]) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={draftPreviewFormat === value}
                    className={draftPreviewFormat === value ? "format-option selected" : "format-option"}
                    onClick={() => setDraftPreviewFormat(value)}
                  >
                    <strong>{label}</strong>
                    <small>{description}</small>
                  </button>
                ))}
              </div>
              <h3 className="preview-resolution-heading">轻量预览分辨率</h3>
              <div className="resolution-options" role="radiogroup" aria-label="轻量预览分辨率">
                {([
                  ["480p", "480p", "最高 854×480"],
                  ["360p", "360p", "最高 640×360"],
                  ["240p", "240p", "最高 426×240"],
                  ["custom", "自定义", "保持母版比例"],
                ] as const).map(([value, label, description]) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={draftPreviewResolution === value}
                    className={draftPreviewResolution === value ? "resolution-option selected" : "resolution-option"}
                    onClick={() => setDraftPreviewResolution(value)}
                  >
                    <strong>{label}</strong>
                    <small>{description}</small>
                  </button>
                ))}
              </div>
              {draftPreviewResolution === "custom" && (
                <div className="custom-resolution">
                  <label>最大宽度<input type="number" min="2" max="3840" step="2" value={customPreviewWidth} onChange={(event) => setCustomPreviewWidth(Number.isFinite(event.currentTarget.valueAsNumber) ? event.currentTarget.valueAsNumber : 0)} /></label>
                  <span>×</span>
                  <label>最大高度<input type="number" min="2" max="2160" step="2" value={customPreviewHeight} onChange={(event) => setCustomPreviewHeight(Number.isFinite(event.currentTarget.valueAsNumber) ? event.currentTarget.valueAsNumber : 0)} /></label>
                </div>
              )}
              <h3 className="preview-resolution-heading">轻量预览帧率</h3>
              <div className="preview-fps-options" role="radiogroup" aria-label="轻量预览帧率">
                {snapshot.supportedPreviewFps.filter((fps) => fps <= draftFps).map((fps) => (
                  <button
                    key={fps}
                    type="button"
                    role="radio"
                    aria-checked={draftPreviewFps === fps}
                    className={draftPreviewFps === fps ? "selected" : ""}
                    onClick={() => setDraftPreviewFps(fps)}
                  >{fps} FPS</button>
                ))}
              </div>
              <p className="preview-settings-note">设置仅应用于之后完成的录制；每个录制组会保存自己的预览配置，重试时不会被后来修改的设置覆盖。</p>
            </div>
            <p className="third-party-notice">FrameFlow 使用 FFmpeg 项目的独立运行时处理预览与封面，该运行时依据 LGPLv3 或更高版本分发；许可证、对应源码和构建配方随安装包提供。</p>
            <footer>
              <button disabled={savingSettings} onClick={() => setSettingsOpen(false)}>取消</button>
              <button className="settings-save" disabled={savingSettings || invalidCustomResolution || invalidCustomPreviewResolution || settingsUnchanged} onClick={() => void saveSettings()}>
                {savingSettings ? "保存中…" : "保存设置"}
              </button>
            </footer>
          </section>
        </div>
      )}
    </>
  );
}

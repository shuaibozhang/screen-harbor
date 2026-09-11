import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { isTauriRuntime } from "./bridge";
import { HomeScreen } from "./HomeScreen";
import { useI18n } from "./i18n";
import { useAppStore } from "./store";
import type { DisplayChangeEvent, FinalizationEvent, InterruptedRecording, OutputFormat, OutputResolution, RecordingState } from "./domain";
import "./styles.css";

type ResolutionChoice = "native" | "1080p" | "720p";
type PreviewResolutionChoice = "480p" | "360p" | "240p";

function waitForRemovalTransition() {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return Promise.resolve();
  return new Promise<void>((resolve) => window.setTimeout(resolve, 200));
}

function resolutionChoice(resolution: OutputResolution): ResolutionChoice {
  if (resolution.mode === "native") return "native";
  if (resolution.width === 1920 && resolution.height === 1080) return "1080p";
  if (resolution.width === 1280 && resolution.height === 720) return "720p";
  return "native";
}

function outputResolution(choice: ResolutionChoice): OutputResolution {
  if (choice === "native") return { mode: "native" };
  if (choice === "1080p") return { mode: "fitWithin", width: 1920, height: 1080 };
  return { mode: "fitWithin", width: 1280, height: 720 };
}

function previewResolutionChoice(resolution: OutputResolution): PreviewResolutionChoice {
  if (resolution.mode === "fitWithin") {
    if (resolution.width === 854 && resolution.height === 480) return "480p";
    if (resolution.width === 640 && resolution.height === 360) return "360p";
    if (resolution.width === 426 && resolution.height === 240) return "240p";
  }
  return "480p";
}

function previewResolution(choice: PreviewResolutionChoice): OutputResolution {
  if (choice === "480p") return { mode: "fitWithin", width: 854, height: 480 };
  if (choice === "360p") return { mode: "fitWithin", width: 640, height: 360 };
  return { mode: "fitWithin", width: 426, height: 240 };
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
  const { locale, setLocale, t } = useI18n();
  const { snapshot, loading, error, hydrate, startSelection, updateOutputSettings, updateAudioInput, updatePreviewEnabled, updateRecordingPreferences, updateSaveDirectory } = useAppStore();
  const [finalizationError, setFinalizationError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [draftFps, setDraftFps] = useState(30);
  const [draftResolution, setDraftResolution] = useState<ResolutionChoice>("native");
  const [draftPreviewFormat, setDraftPreviewFormat] = useState<OutputFormat>("mp4");
  const [draftPreviewResolution, setDraftPreviewResolution] = useState<PreviewResolutionChoice>("480p");
  const [draftPreviewFps, setDraftPreviewFps] = useState(15);
  const [draftCountdownEnabled, setDraftCountdownEnabled] = useState(false);
  const [draftCountdownSeconds, setDraftCountdownSeconds] = useState(3);
  const [savingSettings, setSavingSettings] = useState(false);
  const [savingAudio, setSavingAudio] = useState<"microphone" | "systemAudio" | null>(null);
  const [savingPreview, setSavingPreview] = useState(false);
  const [showAllRecordings, setShowAllRecordings] = useState(false);
  const [openingRecording, setOpeningRecording] = useState<string | null>(null);
  const [deleteConfirmation, setDeleteConfirmation] = useState<{ ids: string[]; names: string[] } | null>(null);
  const [deletingRecordings, setDeletingRecordings] = useState(false);
  const [removingRecordingIds, setRemovingRecordingIds] = useState<Set<string>>(new Set());
  const [changingSaveDirectory, setChangingSaveDirectory] = useState(false);
  const [interruptedDeleteConfirmation, setInterruptedDeleteConfirmation] = useState<InterruptedRecording | null>(null);
  const [deletingInterrupted, setDeletingInterrupted] = useState(false);
  const [removingInterruptedId, setRemovingInterruptedId] = useState<string | null>(null);
  const suppressLibraryRefresh = useRef(false);

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
        setFinalizationError(event.payload.error ?? t("录制文件保存失败。"));
      } else {
        setFinalizationError(null);
      }
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenFinalization = dispose;
    });
    void listen("recording://library-changed", () => {
      if (!suppressLibraryRefresh.current) void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenLibrary = dispose;
    });
    void listen<DisplayChangeEvent>("display://changed", (event) => {
      if (event.payload.selectionCancelled) {
        setFinalizationError(t("显示器配置发生变化，本轮框选已安全取消。请重新开始框选。"));
      }
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenDisplays = dispose;
    });
    void listen<string>("recording://shortcut-error", (event) => {
      setFinalizationError(t("快捷键无法开始框选：{error}", { error: event.payload }));
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
  }, [hydrate, t]);

  useEffect(() => {
    const refreshLibrary = () => void hydrate();
    window.addEventListener("focus", refreshLibrary);
    return () => window.removeEventListener("focus", refreshLibrary);
  }, [hydrate]);

  if (loading) {
    return <main className="shell loading">{t("正在准备 ScreenHarbor…")}</main>;
  }

  if (!snapshot) {
    return (
      <main className="shell loading">
        <div className="startup-error" role="alert">
          <strong>{t("ScreenHarbor 初始化失败")}</strong>
          <span>{error ?? t("无法读取应用状态。")}</span>
          <button type="button" onClick={() => void hydrate()}>{t("重新加载")}</button>
        </div>
      </main>
    );
  }

  const openSettings = () => {
    setDraftFps(snapshot.captureFps);
    const choice = resolutionChoice(snapshot.outputResolution);
    setDraftResolution(choice);
    setDraftPreviewFormat(snapshot.previewFormat);
    setDraftPreviewFps(snapshot.previewFps);
    const previewChoice = previewResolutionChoice(snapshot.previewResolution);
    setDraftPreviewResolution(previewChoice);
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
        outputResolution(draftResolution),
        draftPreviewFormat,
        previewResolution(draftPreviewResolution),
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
  const draftOutputResolution = outputResolution(draftResolution);
  const draftPreviewOutputResolution = previewResolution(draftPreviewResolution);
  const settingsUnchanged = draftFps === snapshot.captureFps
    && sameResolution(draftOutputResolution, snapshot.outputResolution)
    && draftPreviewFormat === snapshot.previewFormat
    && sameResolution(draftPreviewOutputResolution, snapshot.previewResolution)
    && draftPreviewFps === snapshot.previewFps
    && draftCountdownEnabled === snapshot.countdownEnabled
    && draftCountdownSeconds === snapshot.countdownSeconds;

  const updateAudio = async (source: "microphone" | "systemAudio", enabled: boolean, deviceId?: string) => {
    setSavingAudio(source);
    try {
      await updateAudioInput(source, enabled, deviceId);
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

  const requestRecordingDeletion = (ids: string[]) => {
    const recordings = snapshot.recentRecordings.filter((recording) => ids.includes(recording.id));
    if (recordings.length !== ids.length) {
      setFinalizationError(t("部分录制已经不在当前录制库中，请刷新后重试。"));
      return;
    }
    setDeleteConfirmation({ ids, names: recordings.map((recording) => recording.name) });
  };

  const confirmRecordingDeletion = async () => {
    if (!deleteConfirmation) return;
    const deletedIds = [...deleteConfirmation.ids];
    setDeletingRecordings(true);
    suppressLibraryRefresh.current = true;
    try {
      await invoke("delete_recordings", { recordingIds: deletedIds });
      setDeleteConfirmation(null);
      setFinalizationError(null);
      setRemovingRecordingIds(new Set(deletedIds));
      await waitForRemovalTransition();
      await hydrate();
    } catch (deleteError) {
      setFinalizationError(String(deleteError));
    } finally {
      suppressLibraryRefresh.current = false;
      setRemovingRecordingIds(new Set());
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
    const deletedId = interruptedDeleteConfirmation.id;
    setDeletingInterrupted(true);
    suppressLibraryRefresh.current = true;
    try {
      await invoke("delete_interrupted_recording", {
        recordingId: deletedId,
      });
      setInterruptedDeleteConfirmation(null);
      setFinalizationError(null);
      setRemovingInterruptedId(deletedId);
      await waitForRemovalTransition();
      await hydrate();
    } catch (deleteError) {
      setFinalizationError(String(deleteError));
    } finally {
      suppressLibraryRefresh.current = false;
      setRemovingInterruptedId(null);
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
        openingRecording={openingRecording}
        deletingRecordings={deletingRecordings}
        removingRecordingIds={removingRecordingIds}
        removingInterruptedId={removingInterruptedId}
        onOpenSettings={openSettings}
        onStartSelection={() => void startSelection()}
        onChangeSaveDirectory={() => void changeSaveDirectory()}
        onChangeAudio={(source, enabled, deviceId) => void updateAudio(source, enabled, deviceId)}
        onTogglePreview={(enabled) => void togglePreview(enabled)}
        onShowAllRecordings={setShowAllRecordings}
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
            <h2 id="delete-title">{t("移入 Windows 回收站？")}</h2>
            <p id="delete-description">{t("{name}及其母版、预览和两张封面会一起移入回收站，之后仍可恢复。", { name: deleteConfirmation.ids.length === 1 ? deleteConfirmation.names[0] : t("{count} 条录制", { count: deleteConfirmation.ids.length }) })}</p>
            <footer>
              <button type="button" disabled={deletingRecordings} onClick={() => setDeleteConfirmation(null)}>{t("取消")}</button>
              <button className="delete-confirm" type="button" disabled={deletingRecordings} onClick={() => void confirmRecordingDeletion()}>{t(deletingRecordings ? "正在移动…" : "移入回收站")}</button>
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
            <h2 id="interrupted-delete-title">{t("移除未完成录制？")}</h2>
            <p id="interrupted-delete-description">{t("{name} 尚未形成可播放 MP4，其整个录制组会移入 Windows 回收站，之后仍可恢复。", { name: interruptedDeleteConfirmation.name })}</p>
            <footer>
              <button type="button" disabled={deletingInterrupted} onClick={() => setInterruptedDeleteConfirmation(null)}>{t("取消")}</button>
              <button className="delete-confirm" type="button" disabled={deletingInterrupted} onClick={() => void confirmInterruptedDeletion()}>{t(deletingInterrupted ? "正在移动…" : "移入回收站")}</button>
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
              <div><p>{t("录制设置")}</p><h2 id="settings-title">{t("视频")}</h2></div>
              <button className="icon-button" aria-label={t("关闭设置")} disabled={savingSettings} onClick={() => setSettingsOpen(false)}><X size={18} /></button>
            </header>
            <div className="settings-body">
              <div className="setting-group language-settings">
                <div className="countdown-setting-head">
                  <h3>{t("语言")}</h3>
                  <div className="compact-options" role="radiogroup" aria-label={t("语言")}>
                    <button type="button" role="radio" aria-checked={locale === "zh-CN"} className={locale === "zh-CN" ? "selected" : ""} onClick={() => setLocale("zh-CN")}>{t("中文（简体）")}</button>
                    <button type="button" role="radio" aria-checked={locale === "en"} className={locale === "en" ? "selected" : ""} onClick={() => setLocale("en")}>{t("英文")}</button>
                  </div>
                </div>
              </div>
              <div className="setting-group countdown-settings">
                <div className="countdown-setting-head">
                  <h3>{t("开始倒计时")}</h3>
                  <div className="countdown-setting-actions">
                    {draftCountdownEnabled && (
                      <div className="compact-options countdown-options" role="radiogroup" aria-label={t("倒计时时长")}>
                        {[3, 5, 10].map((seconds) => (
                          <button
                            key={seconds}
                            type="button"
                            role="radio"
                            aria-checked={draftCountdownSeconds === seconds}
                            className={draftCountdownSeconds === seconds ? "selected" : ""}
                            onClick={() => setDraftCountdownSeconds(seconds)}
                          >{t("{seconds} 秒", { seconds })}</button>
                        ))}
                      </div>
                    )}
                    <button
                      className={`settings-switch${draftCountdownEnabled ? " enabled" : ""}`}
                      type="button"
                      role="switch"
                      aria-checked={draftCountdownEnabled}
                      aria-label={t("录制开始倒计时")}
                      onClick={() => setDraftCountdownEnabled((enabled) => !enabled)}
                    ><span /></button>
                  </div>
                </div>
              </div>

              <section className="settings-section" aria-labelledby="master-settings-heading">
                <h3 id="master-settings-heading">{t("母版输出")}</h3>
                <div className="settings-grid master-settings-grid">
                  <div className="settings-field">
                    <span className="settings-field-label" id="output-resolution-label">{t("输出分辨率")}</span>
                    <div className="compact-options" role="radiogroup" aria-labelledby="output-resolution-label">
                      {([['native', '原始'], ['1080p', '1080p'], ['720p', '720p']] as const).map(([value, label]) => (
                        <button key={value} type="button" role="radio" aria-checked={draftResolution === value} className={draftResolution === value ? "selected" : ""} onClick={() => setDraftResolution(value)}>{t(label)}</button>
                      ))}
                    </div>
                    <small>{draftResolution === "native" ? t("最佳画质") : t("最高 {size}", { size: draftResolution === "1080p" ? "1920×1080" : "1280×720" })}</small>
                  </div>
                  <div className="settings-field">
                    <span className="settings-field-label" id="capture-fps-label">{t("录制帧率")}</span>
                    <div className="compact-options" role="radiogroup" aria-labelledby="capture-fps-label">
                      {snapshot.supportedCaptureFps.map((fps) => (
                        <button key={fps} type="button" role="radio" aria-checked={draftFps === fps} className={draftFps === fps ? "selected" : ""} onClick={() => {
                          setDraftFps(fps);
                          if (draftPreviewFps > fps) setDraftPreviewFps(fps);
                        }}>{fps} FPS</button>
                      ))}
                    </div>
                    <span className="settings-field-meta">
                      {snapshot.monitorRefreshRateHz !== undefined ? t("当前屏幕 {hz} Hz", { hz: snapshot.monitorRefreshRateHz }) : t("未读取到屏幕刷新率")}
                      {draftFps === snapshot.recommendedCaptureFps && <em>{t("推荐")}</em>}
                    </span>
                    {exceedsMonitorRefresh && (
                      <p className="fps-warning" role="status">{t("该帧率高于当前屏幕刷新率，视频仍会按 {fps} FPS 输出，但可能包含重复帧并增加文件大小。", { fps: draftFps })}</p>
                    )}
                  </div>
                </div>
              </section>

              <section className="settings-section preview-settings-section" aria-labelledby="preview-settings-heading">
                <h3 id="preview-settings-heading">{t("轻量预览")}</h3>
                <div className="settings-grid preview-settings-grid">
                  <div className="settings-field">
                    <span className="settings-field-label" id="preview-format-label">{t("格式")}</span>
                    <div className="compact-options" role="radiogroup" aria-labelledby="preview-format-label">
                      {([['mp4', 'MP4'], ['webm', 'WebM'], ['animated_webp', 'WebP']] as const).map(([value, label]) => (
                        <button key={value} type="button" role="radio" aria-checked={draftPreviewFormat === value} className={draftPreviewFormat === value ? "selected" : ""} onClick={() => setDraftPreviewFormat(value)}>{label}</button>
                      ))}
                    </div>
                    <small>{t(draftPreviewFormat === "mp4" ? "H.264 + AAC，兼容性最好" : draftPreviewFormat === "webm" ? "VP9 + Opus，通常文件更小" : "动态图片，无音频")}</small>
                  </div>
                  <div className="settings-field">
                    <span className="settings-field-label" id="preview-resolution-label">{t("分辨率")}</span>
                    <div className="compact-options" role="radiogroup" aria-labelledby="preview-resolution-label">
                      {([['480p', '480p'], ['360p', '360p'], ['240p', '240p']] as const).map(([value, label]) => (
                        <button key={value} type="button" role="radio" aria-checked={draftPreviewResolution === value} className={draftPreviewResolution === value ? "selected" : ""} onClick={() => setDraftPreviewResolution(value)}>{label}</button>
                      ))}
                    </div>
                    <small>{t("最高 {size}", { size: draftPreviewResolution === "480p" ? "854×480" : draftPreviewResolution === "360p" ? "640×360" : "426×240" })}</small>
                  </div>
                  <div className="settings-field">
                    <span className="settings-field-label" id="preview-fps-label">{t("帧率")}</span>
                    <div className="compact-options" role="radiogroup" aria-labelledby="preview-fps-label">
                      {snapshot.supportedPreviewFps.filter((fps) => fps <= draftFps).map((fps) => (
                        <button key={fps} type="button" role="radio" aria-checked={draftPreviewFps === fps} className={draftPreviewFps === fps ? "selected" : ""} onClick={() => setDraftPreviewFps(fps)}>{fps} FPS</button>
                      ))}
                    </div>
                  </div>
                </div>
              </section>
            </div>
            <footer>
              <button disabled={savingSettings} onClick={() => setSettingsOpen(false)}>{t("取消")}</button>
              <button className="settings-save" disabled={savingSettings || settingsUnchanged} onClick={() => void saveSettings()}>
                {t(savingSettings ? "保存中…" : "保存设置")}
              </button>
            </footer>
          </section>
        </div>
      )}
    </>
  );
}

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  ChevronRight,
  Clock3,
  Folder,
  FolderOpen,
  Mic2,
  MonitorSpeaker,
  Search,
  Settings2,
  Trash2,
  Video,
  X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { isTauriRuntime } from "./bridge";
import { RecordingResult } from "./RecordingResult";
import { useAppStore } from "./store";
import type { DisplayChangeEvent, FinalizationEvent, InterruptedRecording, OutputFormat, OutputResolution, OutputStatus, RecordingDetails, RecordingState } from "./domain";
import "./styles.css";

function StatusDot({ available }: { available: boolean }) {
  return <span className={available ? "status-dot ready" : "status-dot"} aria-hidden="true" />;
}

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

function formatAvailableSpace(bytes?: number) {
  if (bytes === undefined) return "可用空间暂时不可读取";
  const gibibytes = bytes / (1024 ** 3);
  return `可用 ${gibibytes >= 10 ? gibibytes.toFixed(0) : gibibytes.toFixed(1)} GB`;
}

function formatFileSize(bytes: number) {
  const mebibytes = bytes / (1024 ** 2);
  return mebibytes >= 1024
    ? `${(mebibytes / 1024).toFixed(1)} GB`
    : `${Math.max(mebibytes, 0.1).toFixed(1)} MB`;
}

export function App() {
  const { snapshot, loading, error, hydrate, startSelection, updateOutputSettings, updateAudioInput, updateSaveDirectory } = useAppStore();
  const [finalizationError, setFinalizationError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [draftFps, setDraftFps] = useState(30);
  const [draftResolution, setDraftResolution] = useState<ResolutionChoice>("native");
  const [customWidth, setCustomWidth] = useState(1920);
  const [customHeight, setCustomHeight] = useState(1080);
  const [draftPreviewFormat, setDraftPreviewFormat] = useState<OutputFormat>("mp4");
  const [draftPreviewResolution, setDraftPreviewResolution] = useState<PreviewResolutionChoice>("480p");
  const [customPreviewWidth, setCustomPreviewWidth] = useState(854);
  const [customPreviewHeight, setCustomPreviewHeight] = useState(480);
  const [savingSettings, setSavingSettings] = useState(false);
  const [savingAudio, setSavingAudio] = useState<"microphone" | "systemAudio" | null>(null);
  const [showAllRecordings, setShowAllRecordings] = useState(false);
  const [openingRecording, setOpeningRecording] = useState<string | null>(null);
  const [retryingPreview, setRetryingPreview] = useState<string | null>(null);
  const [retryingCover, setRetryingCover] = useState<string | null>(null);
  const [selectedRecordingId, setSelectedRecordingId] = useState<string | null>(null);
  const [recordingDetails, setRecordingDetails] = useState<RecordingDetails | null>(null);
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
        if (event.payload.recordingId) {
          setSelectedRecordingId(event.payload.recordingId);
          void invoke<RecordingDetails>("get_recording_details", {
            recordingId: event.payload.recordingId,
          }).then(setRecordingDetails).catch((detailsError) => {
            setFinalizationError(String(detailsError));
          });
        }
      }
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenFinalization = dispose;
    });
    void listen("recording://library-changed", () => {
      void hydrate();
      if (selectedRecordingId) {
        void invoke<RecordingDetails>("get_recording_details", {
          recordingId: selectedRecordingId,
        }).then(setRecordingDetails).catch((detailsError) => {
          setFinalizationError(String(detailsError));
        });
      }
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
  }, [hydrate, selectedRecordingId]);

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
    const previewChoice = previewResolutionChoice(snapshot.previewResolution);
    setDraftPreviewResolution(previewChoice);
    if (snapshot.previewResolution.mode === "fitWithin" && previewChoice === "custom") {
      setCustomPreviewWidth(snapshot.previewResolution.width);
      setCustomPreviewHeight(snapshot.previewResolution.height);
    }
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
  const draftPreviewOutputResolution = previewResolution(draftPreviewResolution, customPreviewWidth, customPreviewHeight);
  const invalidCustomPreviewResolution = draftPreviewResolution === "custom"
    && (!Number.isInteger(customPreviewWidth) || !Number.isInteger(customPreviewHeight)
      || customPreviewWidth < 2 || customPreviewHeight < 2 || customPreviewWidth > 3840 || customPreviewHeight > 2160);
  const settingsUnchanged = draftFps === snapshot.captureFps
    && sameResolution(draftOutputResolution, snapshot.outputResolution)
    && draftPreviewFormat === snapshot.previewFormat
    && sameResolution(draftPreviewOutputResolution, snapshot.previewResolution);

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

  const retryPreview = async (recordingId: string) => {
    setRetryingPreview(recordingId);
    try {
      await invoke("retry_preview", { recordingId });
      setFinalizationError(null);
      await hydrate();
      if (selectedRecordingId === recordingId) {
        setRecordingDetails(await invoke<RecordingDetails>("get_recording_details", { recordingId }));
      }
    } catch (previewError) {
      setFinalizationError(String(previewError));
      await hydrate();
    } finally {
      setRetryingPreview(null);
    }
  };

  const retryCover = async (recordingId: string) => {
    setRetryingCover(recordingId);
    try {
      await invoke("retry_cover", { recordingId });
      setFinalizationError(null);
      await hydrate();
      if (selectedRecordingId === recordingId) {
        setRecordingDetails(await invoke<RecordingDetails>("get_recording_details", { recordingId }));
      }
    } catch (coverError) {
      setFinalizationError(String(coverError));
      await hydrate();
    } finally {
      setRetryingCover(null);
    }
  };

  const previewLabel = (status: OutputStatus) => {
    if (status === "completed") return "轻量预览已生成";
    if (status === "processing") return "正在生成轻量预览";
    if (status === "failed") return "轻量预览生成失败";
    if (status === "pending") return "轻量预览等待处理";
    return "仅高清母版";
  };

  const normalizedQuery = recordingQuery.trim().toLocaleLowerCase();
  const filteredRecordings = normalizedQuery
    ? snapshot.recentRecordings.filter((recording) => recording.name.toLocaleLowerCase().includes(normalizedQuery))
    : snapshot.recentRecordings;
  const visibleRecordings = showAllRecordings || normalizedQuery
    ? filteredRecordings
    : filteredRecordings.slice(0, 3);
  const allVisibleSelected = visibleRecordings.length > 0
    && visibleRecordings.every((recording) => selectedRecordingIds.has(recording.id));

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

  const openRecordingResult = async (recordingId: string) => {
    setSelectedRecordingId(recordingId);
    setRecordingDetails(null);
    try {
      setRecordingDetails(await invoke<RecordingDetails>("get_recording_details", { recordingId }));
      setFinalizationError(null);
    } catch (detailsError) {
      setSelectedRecordingId(null);
      setFinalizationError(String(detailsError));
    }
  };

  if (selectedRecordingId) {
    if (!recordingDetails) {
      return <main className="shell loading">正在读取录制结果…</main>;
    }
    return (
      <RecordingResult
        details={recordingDetails}
        retrying={retryingPreview === selectedRecordingId}
        retryingCover={retryingCover === selectedRecordingId}
        onBack={() => {
          setSelectedRecordingId(null);
          setRecordingDetails(null);
          void hydrate();
        }}
        onOpenLocation={() => void openRecordingLocation(selectedRecordingId)}
        onRetryPreview={() => void retryPreview(selectedRecordingId)}
        onRetryCover={() => void retryCover(selectedRecordingId)}
      />
    );
  }

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
            <div className="start-control">
              <button className="start-button" onClick={() => void startSelection()}>
                <span className="record-dot" />开始录屏
              </button>
            </div>
            <small>
              {snapshot.globalShortcutAvailable
                ? "开始后在任意屏幕框选 · Alt + Shift + R"
                : "开始后在任意屏幕框选 · 快捷键被其他应用占用"}
            </small>
          </div>
        </section>

        <section className="section" aria-labelledby="save-heading">
          <div className="section-heading">
            <h2 id="save-heading">保存位置</h2>
            <button
              disabled={changingSaveDirectory || snapshot.recordingState !== "idle"}
              onClick={() => void changeSaveDirectory()}
            >
              {changingSaveDirectory ? "选择中…" : "更改"}
            </button>
          </div>
          <button
            className="row path-row"
            disabled={changingSaveDirectory || snapshot.recordingState !== "idle"}
            onClick={() => void changeSaveDirectory()}
            title={snapshot.saveDirectory}
          >
            <span className="row-icon"><Folder size={19} /></span>
            <span className="row-copy">
              <strong>{snapshot.saveDirectory}</strong>
              <small>{formatAvailableSpace(snapshot.saveDirectoryAvailableBytes)} · 切换不会移动或隐藏旧录像</small>
            </span>
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

        {snapshot.interruptedRecordings.length > 0 && (
          <section className="section interrupted" aria-labelledby="interrupted-heading">
            <div className="section-heading"><h2 id="interrupted-heading">未完成录制</h2></div>
            <div className="interrupted-notice">
              <AlertTriangle size={17} />
              <span>以下文件来自异常退出，未形成可播放的完整 MP4。FrameFlow 已原样保留。</span>
            </div>
            <div className="rows">
              {snapshot.interruptedRecordings.map((recording) => (
                <div className="row" key={recording.id}>
                  <span className="row-icon interrupted-icon"><AlertTriangle size={18} /></span>
                  <span className="row-copy">
                    <strong>{recording.name}</strong>
                    <small>{recording.createdLabel} · 已保留 {formatFileSize(recording.fileSizeBytes)}</small>
                  </span>
                  <button className="recording-location" type="button" onClick={() => void openInterruptedLocation(recording.id)}>
                    <FolderOpen size={14} />打开位置
                  </button>
                  <button className="recording-delete" type="button" aria-label={`删除未完成录制 ${recording.name}`} onClick={() => setInterruptedDeleteConfirmation(recording)}>
                    <Trash2 size={15} />
                  </button>
                </div>
              ))}
            </div>
          </section>
        )}

        <section className="section recent" aria-labelledby="recent-heading">
          <div className="section-heading"><h2 id="recent-heading">录制库</h2>{snapshot.recentRecordings.length > 3 && !normalizedQuery && <button onClick={() => setShowAllRecordings((visible) => !visible)}>{showAllRecordings ? "收起" : "查看全部"}</button>}</div>
          {snapshot.recentRecordings.length > 0 && (
            <div className="library-tools">
              <label className="library-search">
                <Search size={15} />
                <input value={recordingQuery} onChange={(event) => setRecordingQuery(event.target.value)} placeholder="搜索录制名称" aria-label="搜索录制名称" />
              </label>
              <button className="library-select-all" type="button" onClick={() => {
                setSelectedRecordingIds((current) => {
                  const next = new Set(current);
                  visibleRecordings.forEach((recording) => {
                    if (allVisibleSelected) next.delete(recording.id);
                    else next.add(recording.id);
                  });
                  return next;
                });
              }}>{allVisibleSelected ? "取消全选" : "全选当前"}</button>
              {selectedRecordingIds.size > 0 && (
                <button className="library-delete-selected" type="button" onClick={() => requestRecordingDeletion([...selectedRecordingIds])}>
                  <Trash2 size={14} />移入回收站（{selectedRecordingIds.size}）
                </button>
              )}
            </div>
          )}
          {snapshot.recentRecordings.length === 0 ? (
            <div className="empty"><Clock3 size={20} /><span><strong>还没有录制</strong><small>完成的内容会出现在这里</small></span></div>
          ) : visibleRecordings.length === 0 ? (
            <div className="empty"><Search size={20} /><span><strong>没有匹配的录制</strong><small>换一个名称关键词试试</small></span></div>
          ) : (
            <div className={`rows recent-list ${showAllRecordings || normalizedQuery ? "expanded" : ""}`}>
              {visibleRecordings.map((recording) => (
                <div className="row" key={recording.id}>
                  <input className="recording-select" type="checkbox" aria-label={`选择 ${recording.name}`} checked={selectedRecordingIds.has(recording.id)} onChange={() => toggleRecordingSelection(recording.id)} />
                  <span className="recording-thumbnail">
                    {recording.coverStatus === "completed" && recording.coverPath
                      ? <img src={convertFileSrc(recording.coverPath)} alt="" />
                      : <Video size={19} />}
                  </span>
                  <span className="row-copy">
                    <strong>{recording.name}</strong>
                    <small>{recording.createdLabel} · {recording.durationSeconds !== undefined ? `${recording.durationSeconds} 秒` : "时长未知"} · {previewLabel(recording.previewStatus)}</small>
                  </span>
                  {recording.previewStatus === "failed" && <button className="recording-location" disabled={openingRecording !== null || retryingPreview !== null} onClick={() => void retryPreview(recording.id)}>{retryingPreview === recording.id ? "重试中…" : "重试预览"}</button>}
                  {recording.coverStatus === "failed" && <button className="recording-location" disabled={openingRecording !== null || retryingCover !== null} onClick={() => void retryCover(recording.id)}>{retryingCover === recording.id ? "重试中…" : "重试封面"}</button>}
                  <button className="recording-location" disabled={openingRecording !== null || retryingPreview !== null} onClick={() => void openRecordingResult(recording.id)}>查看</button>
                  <button className="recording-delete" aria-label={`删除 ${recording.name}`} disabled={deletingRecordings} onClick={() => requestRecordingDeletion([recording.id])}><Trash2 size={15} /></button>
                </div>
              ))}
            </div>
          )}
        </section>
      </div>
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
            <div className="setting-group preview-settings">
              <h3>轻量预览格式</h3>
              <div className="format-options" role="radiogroup" aria-label="轻量预览格式">
                {([
                  ["mp4", "MP4", "H.264 + AAC，兼容性最好"],
                  ["webm", "WebM", "VP9 + Opus，通常文件更小"],
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
    </main>
  );
}

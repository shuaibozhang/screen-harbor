import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  AlertTriangle,
  CalendarDays,
  ChevronDown,
  Clock3,
  FileVideo2,
  Folder,
  FolderOpen,
  Mic2,
  Minus,
  MonitorSpeaker,
  Search,
  Settings,
  Square,
  Trash2,
  X,
} from "lucide-react";
import type { AppSnapshot, InterruptedRecording, OutputResolution, OutputStatus, RecentRecording } from "./domain";
import { isTauriRuntime } from "./bridge";
import "./home-screen.css";

interface HomeScreenProps {
  snapshot: AppSnapshot;
  alert: string | null;
  savingAudio: "microphone" | "systemAudio" | null;
  savingPreview: boolean;
  changingSaveDirectory: boolean;
  showAllRecordings: boolean;
  recordingQuery: string;
  selectedRecordingIds: Set<string>;
  openingRecording: string | null;
  deletingRecordings: boolean;
  onOpenSettings: () => void;
  onStartSelection: () => void;
  onChangeSaveDirectory: () => void;
  onToggleAudio: (source: "microphone" | "systemAudio", enabled: boolean) => void;
  onTogglePreview: (enabled: boolean) => void;
  onShowAllRecordings: (visible: boolean) => void;
  onRecordingQueryChange: (query: string) => void;
  onToggleRecordingSelection: (recordingId: string) => void;
  onSetVisibleSelection: (recordingIds: string[], selected: boolean) => void;
  onRequestRecordingDeletion: (recordingIds: string[]) => void;
  onOpenRecordingLocation: (recordingId: string) => void;
  onOpenInterruptedLocation: (recordingId: string) => void;
  onRequestInterruptedDeletion: (recording: InterruptedRecording) => void;
}

function resolutionLabel(resolution: OutputResolution) {
  return resolution.mode === "native"
    ? "原始分辨率"
    : `${resolution.width} × ${resolution.height}`;
}

function formatLabel(format: AppSnapshot["previewFormat"]) {
  if (format === "webm") return "WebM";
  if (format === "animated_webp") return "WebP";
  return "MP4";
}

function formatDuration(seconds?: number) {
  if (seconds === undefined) return "时长未知";
  const wholeSeconds = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(wholeSeconds / 60);
  const remainder = wholeSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`;
}

function formatFileSize(bytes: number) {
  const mebibytes = bytes / (1024 ** 2);
  return mebibytes >= 1024
    ? `${(mebibytes / 1024).toFixed(1)} GB`
    : `${Math.max(mebibytes, 0.1).toFixed(1)} MB`;
}

function previewStatusLabel(status: OutputStatus) {
  if (status === "completed") return "轻量预览已生成";
  if (status === "processing") return "正在生成轻量预览";
  if (status === "failed") return "轻量预览生成失败";
  if (status === "pending") return "轻量预览等待处理";
  return "仅高清母版";
}

async function runWindowAction(action: "minimize" | "maximize" | "close") {
  if (!isTauriRuntime()) return;
  const appWindow = getCurrentWindow();
  if (action === "minimize") await appWindow.minimize();
  if (action === "maximize") await appWindow.toggleMaximize();
  if (action === "close") await appWindow.close();
}

function RecordingThumbnail({ recording }: { recording: RecentRecording }) {
  return (
    <span className="home-recording-thumbnail">
      {recording.coverStatus === "completed" && recording.coverPath
        ? <img src={convertFileSrc(recording.coverPath)} alt="" />
        : <FileVideo2 size={24} strokeWidth={1.45} />}
    </span>
  );
}

export function HomeScreen({
  snapshot,
  alert,
  savingAudio,
  savingPreview,
  changingSaveDirectory,
  showAllRecordings,
  recordingQuery,
  selectedRecordingIds,
  openingRecording,
  deletingRecordings,
  onOpenSettings,
  onStartSelection,
  onChangeSaveDirectory,
  onToggleAudio,
  onTogglePreview,
  onShowAllRecordings,
  onRecordingQueryChange,
  onToggleRecordingSelection,
  onSetVisibleSelection,
  onRequestRecordingDeletion,
  onOpenRecordingLocation,
  onOpenInterruptedLocation,
  onRequestInterruptedDeletion,
}: HomeScreenProps) {
  const normalizedQuery = recordingQuery.trim().toLocaleLowerCase();
  const filteredRecordings = normalizedQuery
    ? snapshot.recentRecordings.filter((recording) => recording.name.toLocaleLowerCase().includes(normalizedQuery))
    : snapshot.recentRecordings;
  const visibleRecordings = showAllRecordings || normalizedQuery
    ? filteredRecordings
    : filteredRecordings.slice(0, 3);
  const allVisibleSelected = visibleRecordings.length > 0
    && visibleRecordings.every((recording) => selectedRecordingIds.has(recording.id));
  const canConfigure = snapshot.recordingState === "idle";

  return (
    <main className="shell home-shell">
      <header
        className="app-titlebar"
        data-tauri-drag-region
        onDoubleClick={() => void runWindowAction("maximize")}
      >
        <div className="app-brand" data-tauri-drag-region>
          <span className="app-brand-mark" aria-hidden="true" />
          <span data-tauri-drag-region>FrameFlow</span>
        </div>
        <div className="window-actions" onDoubleClick={(event) => event.stopPropagation()}>
          <button type="button" aria-label="打开设置" onClick={onOpenSettings}><Settings size={20} /></button>
          <button type="button" aria-label="最小化" onClick={() => void runWindowAction("minimize")}><Minus size={20} /></button>
          <button type="button" aria-label="最大化或还原" onClick={() => void runWindowAction("maximize")}><Square size={17} /></button>
          <button className="window-close" type="button" aria-label="关闭" onClick={() => void runWindowAction("close")}><X size={21} /></button>
        </div>
      </header>

      <div className="home-scroll">
        {alert && <div className="home-alert" role="alert">{alert}</div>}

        <section className="home-section" aria-labelledby="audio-heading">
          <h1 id="audio-heading">录制声音</h1>
          <div className="preference-list">
            <button
              className="preference-row preference-action"
              type="button"
              role="switch"
              aria-checked={snapshot.microphone.enabled}
              disabled={!snapshot.microphone.available || savingAudio !== null || !canConfigure}
              onClick={() => onToggleAudio("microphone", !snapshot.microphone.enabled)}
            >
              <Mic2 size={28} strokeWidth={1.45} />
              <span className="preference-copy">
                <strong>麦克风</strong>
                <small>{snapshot.microphone.deviceName ?? "未找到可用麦克风"}</small>
              </span>
              <span className="preference-state">{snapshot.microphone.available ? (snapshot.microphone.enabled ? "已开启" : "未开启") : "不可用"}</span>
              <ChevronDown size={17} />
            </button>
            <button
              className="preference-row preference-action"
              type="button"
              role="switch"
              aria-checked={snapshot.systemAudio.enabled}
              disabled={!snapshot.systemAudio.available || savingAudio !== null || !canConfigure}
              onClick={() => onToggleAudio("systemAudio", !snapshot.systemAudio.enabled)}
            >
              <MonitorSpeaker size={28} strokeWidth={1.45} />
              <span className="preference-copy">
                <strong>系统声音</strong>
                <small>{snapshot.systemAudio.deviceName ?? "未找到可用输出设备"}</small>
              </span>
              <span className="preference-state">{snapshot.systemAudio.available ? (snapshot.systemAudio.enabled ? "已开启" : "未开启") : "不可用"}</span>
              <ChevronDown size={17} />
            </button>
          </div>
        </section>

        <section className="home-section" aria-labelledby="output-heading">
          <h1 id="output-heading">输出文件</h1>
          <div className="preference-list">
            <button className="preference-row preference-action" type="button" onClick={onOpenSettings}>
              <FileVideo2 size={27} strokeWidth={1.45} />
              <span className="preference-copy">
                <strong>高清母版</strong>
                <small>MP4 · {resolutionLabel(snapshot.outputResolution)} · {snapshot.captureFps} FPS</small>
              </span>
              <ChevronDown size={17} />
            </button>
            <div className="preference-row preview-row">
              <FileVideo2 size={27} strokeWidth={1.45} />
              <span className="preference-copy">
                <strong>轻量预览</strong>
                <small>{formatLabel(snapshot.previewFormat)} · {resolutionLabel(snapshot.previewResolution)} · {snapshot.previewFps} FPS</small>
                <span className="preference-note">关闭后不生成轻量预览及对应封面</span>
              </span>
              <button
                className={`sage-switch ${snapshot.previewEnabled ? "enabled" : ""}`}
                type="button"
                role="switch"
                aria-checked={snapshot.previewEnabled}
                aria-label="生成轻量预览和封面"
                disabled={savingPreview || !canConfigure}
                onClick={() => onTogglePreview(!snapshot.previewEnabled)}
              ><span /></button>
            </div>
          </div>
        </section>

        <section className="home-section" aria-labelledby="save-heading">
          <h1 id="save-heading">保存位置</h1>
          <div className="preference-row save-row">
            <Folder size={27} strokeWidth={1.45} />
            <span className="save-path" title={snapshot.saveDirectory}>{snapshot.saveDirectory}</span>
            <button
              className="text-action"
              type="button"
              disabled={changingSaveDirectory || !canConfigure}
              onClick={onChangeSaveDirectory}
            >{changingSaveDirectory ? "选择中…" : "更改"}</button>
          </div>
        </section>

        {snapshot.interruptedRecordings.length > 0 && (
          <section className="home-section interrupted-section" aria-labelledby="interrupted-heading">
            <h1 id="interrupted-heading">未完成录制</h1>
            <p className="section-note"><AlertTriangle size={16} />异常退出留下的文件已原样保留，可打开位置检查或移入回收站。</p>
            <div className="preference-list">
              {snapshot.interruptedRecordings.map((recording) => (
                <div className="interrupted-row" key={recording.id}>
                  <FileVideo2 size={25} strokeWidth={1.45} />
                  <span className="preference-copy"><strong>{recording.name}</strong><small>{recording.createdLabel} · {formatFileSize(recording.fileSizeBytes)}</small></span>
                  <button className="row-tool" type="button" onClick={() => onOpenInterruptedLocation(recording.id)}><FolderOpen size={16} />打开位置</button>
                  <button className="row-icon-action danger" type="button" aria-label={`删除未完成录制 ${recording.name}`} onClick={() => onRequestInterruptedDeletion(recording)}><Trash2 size={17} /></button>
                </div>
              ))}
            </div>
          </section>
        )}

        <section className="home-section recent-section" aria-labelledby="recent-heading">
          <div className="home-section-heading">
            <h1 id="recent-heading">最近录制</h1>
            {snapshot.recentRecordings.length > 3 && !normalizedQuery && (
              <button className="text-action" type="button" onClick={() => onShowAllRecordings(!showAllRecordings)}>{showAllRecordings ? "收起" : "查看全部"}</button>
            )}
          </div>

          {(snapshot.recentRecordings.length > 3 || normalizedQuery || selectedRecordingIds.size > 0) && (
            <div className="library-tools home-library-tools">
              <label className="library-search">
                <Search size={14} />
                <input value={recordingQuery} onChange={(event) => onRecordingQueryChange(event.target.value)} placeholder="搜索录制名称" aria-label="搜索录制名称" />
              </label>
              <button className="library-select-all" type="button" onClick={() => onSetVisibleSelection(visibleRecordings.map((recording) => recording.id), !allVisibleSelected)}>{allVisibleSelected ? "取消全选" : "全选当前"}</button>
              {selectedRecordingIds.size > 0 && (
                <button className="library-delete-selected" type="button" onClick={() => onRequestRecordingDeletion([...selectedRecordingIds])}><Trash2 size={13} />移入回收站（{selectedRecordingIds.size}）</button>
              )}
            </div>
          )}

          {snapshot.recentRecordings.length === 0 ? (
            <div className="home-empty"><Clock3 size={22} /><span><strong>还没有录制</strong><small>完成的内容会出现在这里</small></span></div>
          ) : visibleRecordings.length === 0 ? (
            <div className="home-empty"><Search size={20} /><span><strong>没有匹配的录制</strong><small>换一个名称关键词试试</small></span></div>
          ) : (
            <div className={`home-recording-list ${showAllRecordings || normalizedQuery ? "expanded" : ""}`}>
              {visibleRecordings.map((recording) => (
                <article className="home-recording" key={recording.id}>
                  <input className="recording-select" type="checkbox" aria-label={`选择 ${recording.name}`} checked={selectedRecordingIds.has(recording.id)} onChange={() => onToggleRecordingSelection(recording.id)} />
                  <RecordingThumbnail recording={recording} />
                  <div className="recording-copy">
                    <strong>{recording.name}</strong>
                    <span className="recording-meta">
                      <span><CalendarDays size={14} />{recording.createdLabel}</span>
                      <span><Clock3 size={14} />{formatDuration(recording.durationSeconds)}</span>
                      <span><FileVideo2 size={14} />MP4</span>
                    </span>
                    <small>{previewStatusLabel(recording.previewStatus)}</small>
                  </div>
                  <div className="recording-actions">
                    <button className="row-icon-action" aria-label={`打开 ${recording.name} 所在文件夹`} title="打开所在文件夹" disabled={openingRecording !== null} onClick={() => onOpenRecordingLocation(recording.id)}><FolderOpen size={17} /></button>
                    <button className="row-icon-action danger recording-remove" aria-label={`删除 ${recording.name}`} title="移入回收站" disabled={deletingRecordings} onClick={() => onRequestRecordingDeletion([recording.id])}><Trash2 size={15} /></button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </section>
      </div>

      <footer className="home-footer">
        <button className="record-button" type="button" disabled={!canConfigure} onClick={onStartSelection}>
          {canConfigure ? "开始录屏" : "录制进行中"}
        </button>
      </footer>
    </main>
  );
}

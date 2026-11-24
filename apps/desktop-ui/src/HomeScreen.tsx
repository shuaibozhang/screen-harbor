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
  Settings,
  Square,
  Trash2,
  X,
} from "lucide-react";
import type { AppSnapshot, InterruptedRecording, OutputResolution, OutputStatus, RecentRecording } from "./domain";
import { isTauriRuntime } from "./bridge";
import { localizeRelativeTime, useI18n } from "./i18n";
import screenHarborMark from "./assets/screenharbor-mark.svg";
import "./home-screen.css";

interface HomeScreenProps {
  snapshot: AppSnapshot;
  alert: string | null;
  savingAudio: "microphone" | "systemAudio" | null;
  savingPreview: boolean;
  changingSaveDirectory: boolean;
  showAllRecordings: boolean;
  openingRecording: string | null;
  deletingRecordings: boolean;
  removingRecordingIds: Set<string>;
  removingInterruptedId: string | null;
  onOpenSettings: () => void;
  onStartSelection: () => void;
  onChangeSaveDirectory: () => void;
  onChangeAudio: (source: "microphone" | "systemAudio", enabled: boolean, deviceId?: string) => void;
  onTogglePreview: (enabled: boolean) => void;
  onShowAllRecordings: (visible: boolean) => void;
  onRequestRecordingDeletion: (recordingIds: string[]) => void;
  onOpenRecordingLocation: (recordingId: string) => void;
  onOpenInterruptedLocation: (recordingId: string) => void;
  onRequestInterruptedDeletion: (recording: InterruptedRecording) => void;
}

function resolutionLabel(resolution: OutputResolution, t: (source: string) => string) {
  return resolution.mode === "native"
    ? t("原始分辨率")
    : `${resolution.width} × ${resolution.height}`;
}

function formatLabel(format: AppSnapshot["previewFormat"]) {
  if (format === "webm") return "WebM";
  if (format === "animated_webp") return "WebP";
  return "MP4";
}

function formatDuration(seconds: number | undefined, t: (source: string) => string) {
  if (seconds === undefined) return t("时长未知");
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

function previewStatusLabel(status: OutputStatus, t: (source: string) => string) {
  if (status === "completed") return t("轻量预览已生成");
  if (status === "processing") return t("正在生成轻量预览");
  if (status === "failed") return t("轻量预览生成失败");
  if (status === "pending") return t("轻量预览等待处理");
  return t("仅高清母版");
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
  openingRecording,
  deletingRecordings,
  removingRecordingIds,
  removingInterruptedId,
  onOpenSettings,
  onStartSelection,
  onChangeSaveDirectory,
  onChangeAudio,
  onTogglePreview,
  onShowAllRecordings,
  onRequestRecordingDeletion,
  onOpenRecordingLocation,
  onOpenInterruptedLocation,
  onRequestInterruptedDeletion,
}: HomeScreenProps) {
  const { locale, t } = useI18n();
  const visibleRecordings = showAllRecordings
    ? snapshot.recentRecordings
    : snapshot.recentRecordings.slice(0, 3);
  const canConfigure = snapshot.recordingState === "idle";
  const audioSummary = [
    snapshot.microphone.enabled ? t("麦克风") : null,
    snapshot.systemAudio.enabled ? t("系统声音") : null,
  ].filter(Boolean).join(" + ") || t("无声音");

  return (
    <main className="shell home-shell">
      <header
        className="app-titlebar"
        data-tauri-drag-region
        onDoubleClick={() => void runWindowAction("maximize")}
      >
        <div className="app-brand" data-tauri-drag-region>
          <img className="app-brand-mark" src={screenHarborMark} alt="" draggable={false} />
          <span className="app-brand-name" data-tauri-drag-region>
            <span className="app-brand-screen" data-tauri-drag-region>Screen</span>
            <span className="app-brand-harbor" data-tauri-drag-region>Harbor</span>
          </span>
        </div>
        <div className="window-actions" onDoubleClick={(event) => event.stopPropagation()}>
          <button type="button" aria-label={t("打开设置")} onClick={onOpenSettings}><Settings size={18} strokeWidth={1.5} /></button>
          <button type="button" aria-label={t("最小化")} onClick={() => void runWindowAction("minimize")}><Minus size={18} strokeWidth={1.5} /></button>
          <button type="button" aria-label={t("最大化或还原")} onClick={() => void runWindowAction("maximize")}><Square size={15} strokeWidth={1.5} /></button>
          <button className="window-close" type="button" aria-label={t("关闭")} onClick={() => void runWindowAction("close")}><X size={19} strokeWidth={1.5} /></button>
        </div>
      </header>

      <div className="home-scroll">
        {alert && <div className="home-alert" role="alert">{t(alert)}</div>}

        <section className="home-section" aria-labelledby="audio-heading">
          <h1 id="audio-heading">{t("录制声音")}</h1>
          <div className="preference-list">
            <div className="preference-row audio-preference-row">
              <span className="preference-icon"><Mic2 size={20} strokeWidth={1.5} /></span>
              <strong className="audio-label">{t("麦克风")}</strong>
              <span className="device-select">
                <select
                  aria-label={t("选择麦克风")}
                  value={snapshot.microphone.selectedDeviceId ?? ""}
                  disabled={savingAudio !== null || !canConfigure}
                  onChange={(event) => onChangeAudio("microphone", snapshot.microphone.enabled, event.currentTarget.value || undefined)}
                >
                  <option value="">{t("跟随系统默认")}{snapshot.microphone.devices.some((device) => device.isDefault) ? "" : t("（当前不可用）")}</option>
                  {snapshot.microphone.selectedDeviceId && !snapshot.microphone.devices.some((device) => device.id === snapshot.microphone.selectedDeviceId) && <option value={snapshot.microphone.selectedDeviceId}>{t("之前选择的麦克风（当前不可用）")}</option>}
                  {snapshot.microphone.devices.map((device) => <option key={device.id} value={device.id}>{device.name}{device.isDefault ? t("（当前默认）") : ""}</option>)}
                </select>
                <ChevronDown size={14} strokeWidth={1.5} aria-hidden="true" />
              </span>
              <button
                className={snapshot.microphone.enabled ? "sage-switch enabled" : "sage-switch"}
                type="button"
                role="switch"
                aria-label={t("录制麦克风")}
                aria-checked={snapshot.microphone.enabled}
                disabled={(!snapshot.microphone.available && !snapshot.microphone.enabled) || savingAudio !== null || !canConfigure}
                onClick={() => onChangeAudio("microphone", !snapshot.microphone.enabled, snapshot.microphone.selectedDeviceId)}
              ><span /></button>
            </div>
            <div className="preference-row audio-preference-row">
              <span className="preference-icon"><MonitorSpeaker size={20} strokeWidth={1.5} /></span>
              <strong className="audio-label">{t("系统声音")}</strong>
              <span className="device-select">
                <select
                  aria-label={t("选择系统声音输出设备")}
                  value={snapshot.systemAudio.selectedDeviceId ?? ""}
                  disabled={savingAudio !== null || !canConfigure}
                  onChange={(event) => onChangeAudio("systemAudio", snapshot.systemAudio.enabled, event.currentTarget.value || undefined)}
                >
                  <option value="">{t("跟随系统默认")}{snapshot.systemAudio.devices.some((device) => device.isDefault) ? "" : t("（当前不可用）")}</option>
                  {snapshot.systemAudio.selectedDeviceId && !snapshot.systemAudio.devices.some((device) => device.id === snapshot.systemAudio.selectedDeviceId) && <option value={snapshot.systemAudio.selectedDeviceId}>{t("之前选择的输出设备（当前不可用）")}</option>}
                  {snapshot.systemAudio.devices.map((device) => <option key={device.id} value={device.id}>{device.name}{device.isDefault ? t("（当前默认）") : ""}</option>)}
                </select>
                <ChevronDown size={14} strokeWidth={1.5} aria-hidden="true" />
              </span>
              <button
                className={snapshot.systemAudio.enabled ? "sage-switch enabled" : "sage-switch"}
                type="button"
                role="switch"
                aria-label={t("录制系统声音")}
                aria-checked={snapshot.systemAudio.enabled}
                disabled={(!snapshot.systemAudio.available && !snapshot.systemAudio.enabled) || savingAudio !== null || !canConfigure}
                onClick={() => onChangeAudio("systemAudio", !snapshot.systemAudio.enabled, snapshot.systemAudio.selectedDeviceId)}
              ><span /></button>
            </div>
          </div>
        </section>

        <section className="home-section" aria-labelledby="output-heading">
          <h1 id="output-heading">{t("输出文件")}</h1>
          <div className="preference-list">
            <button className="preference-row preference-action" type="button" onClick={onOpenSettings}>
              <FileVideo2 size={27} strokeWidth={1.45} />
              <span className="preference-copy">
                <strong>{t("主视频")}</strong>
                <small>{formatLabel(snapshot.mainFormat)} · {t(snapshot.mainQuality === "low" ? "低画质" : snapshot.mainQuality === "high" ? "高画质" : "平衡")} · {resolutionLabel(snapshot.outputResolution, t)} · {snapshot.captureFps} FPS</small>
              </span>
              <ChevronDown size={17} />
            </button>
            <div className="preference-row preview-row">
              <FileVideo2 size={27} strokeWidth={1.45} />
              <span className="preference-copy">
                <strong>{t("同时生成轻量预览")}</strong>
                <small>{formatLabel(snapshot.previewFormat)} · {resolutionLabel(snapshot.previewResolution, t)} · {snapshot.previewFps} FPS</small>
              </span>
              <button
                className={`sage-switch ${snapshot.previewEnabled ? "enabled" : ""}`}
                type="button"
                role="switch"
                aria-checked={snapshot.previewEnabled}
                aria-label={t("生成轻量预览和封面")}
                disabled={savingPreview || !canConfigure}
                onClick={() => onTogglePreview(!snapshot.previewEnabled)}
              ><span /></button>
            </div>
          </div>
        </section>

        <section className="home-section" aria-labelledby="save-heading">
          <h1 id="save-heading">{t("保存位置")}</h1>
          <div className="preference-row save-row">
            <Folder size={27} strokeWidth={1.45} />
            <span className="save-path" title={snapshot.saveDirectory}>{snapshot.saveDirectory}</span>
            <button
              className="text-action"
              type="button"
              disabled={changingSaveDirectory || !canConfigure}
              onClick={onChangeSaveDirectory}
            >{t(changingSaveDirectory ? "选择中…" : "更改")}</button>
          </div>
        </section>

        {snapshot.interruptedRecordings.length > 0 && (
          <section className="home-section interrupted-section" aria-labelledby="interrupted-heading">
            <h1 id="interrupted-heading">{t("未完成录制")}</h1>
            <p className="section-note"><AlertTriangle size={16} />{t("异常退出留下的文件已原样保留，可打开位置检查或移入回收站。")}</p>
            <div className="preference-list">
              {snapshot.interruptedRecordings.map((recording) => (
                <div className={`interrupted-row${removingInterruptedId === recording.id ? " removing" : ""}`} key={recording.id}>
                  <FileVideo2 size={25} strokeWidth={1.45} />
                  <span className="preference-copy"><strong>{recording.name}</strong><small>{localizeRelativeTime(locale, recording.createdLabel)} · {formatFileSize(recording.fileSizeBytes)}</small></span>
                  <button className="row-tool" type="button" onClick={() => onOpenInterruptedLocation(recording.id)}><FolderOpen size={16} />{t("打开位置")}</button>
                  <button className="row-icon-action danger" type="button" aria-label={t("删除未完成录制 {name}", { name: recording.name })} onClick={() => onRequestInterruptedDeletion(recording)}><Trash2 size={17} /></button>
                </div>
              ))}
            </div>
          </section>
        )}

        <section className="home-section recent-section" aria-labelledby="recent-heading">
          <div className="home-section-heading">
            <h1 id="recent-heading">{t("最近录制")}</h1>
            {snapshot.recentRecordings.length > 3 && (
              <button className="text-action" type="button" onClick={() => onShowAllRecordings(!showAllRecordings)}>{t(showAllRecordings ? "收起" : "查看全部")}</button>
            )}
          </div>

          {snapshot.recentRecordings.length === 0 ? (
            <div className="home-empty"><Clock3 size={22} /><span><strong>{t("还没有录制")}</strong><small>{t("完成的内容会出现在这里")}</small></span></div>
          ) : (
            <div className={`home-recording-list ${showAllRecordings ? "expanded" : ""}`}>
              {visibleRecordings.map((recording) => (
                <article className={`home-recording${removingRecordingIds.has(recording.id) ? " removing" : ""}`} key={recording.id}>
                  <RecordingThumbnail recording={recording} />
                  <div className="recording-copy">
                    <strong>{recording.name}</strong>
                    <span className="recording-meta">
                      <span><CalendarDays size={14} />{localizeRelativeTime(locale, recording.createdLabel)}</span>
                      <span><Clock3 size={14} />{formatDuration(recording.durationSeconds, t)}</span>
                      <span><FileVideo2 size={14} />{formatLabel(recording.mainFormat)}</span>
                    </span>
                    <small>{previewStatusLabel(recording.previewStatus, t)}</small>
                  </div>
                  <div className="recording-actions">
                    <button className="row-icon-action" aria-label={t("打开 {name} 所在文件夹", { name: recording.name })} title={t("打开所在文件夹")} disabled={openingRecording !== null} onClick={() => onOpenRecordingLocation(recording.id)}><FolderOpen size={17} /></button>
                    <button className="row-icon-action danger recording-remove" aria-label={t("删除 {name}", { name: recording.name })} title={t("移入回收站")} disabled={deletingRecordings} onClick={() => onRequestRecordingDeletion([recording.id])}><Trash2 size={15} /></button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </section>
      </div>

      <footer className="home-footer">
        <div className="current-config" aria-label={t("当前录制配置")}>
          <strong>{t("当前配置")}</strong>
          <span>{resolutionLabel(snapshot.outputResolution, t)} · {snapshot.captureFps} FPS · {audioSummary}{snapshot.previewEnabled ? ` · ${t("含轻量预览")}` : ""}</span>
        </div>
        <button className="record-button" type="button" disabled={!canConfigure} onClick={onStartSelection}>
          <span className="record-dot" aria-hidden="true" />
          {t(canConfigure ? "开始录屏" : "录制进行中")}
        </button>
      </footer>
    </main>
  );
}

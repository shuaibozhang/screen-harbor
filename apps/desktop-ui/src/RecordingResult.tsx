import { convertFileSrc } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  CheckCircle2,
  FolderOpen,
  LoaderCircle,
  RotateCcw,
} from "lucide-react";
import type { RecordingArtifact, RecordingDetails } from "./domain";
import { localizeRelativeTime, useI18n } from "./i18n";

interface RecordingResultProps {
  details: RecordingDetails;
  retrying: boolean;
  retryingCover: boolean;
  onBack: () => void;
  onOpenLocation: () => void;
  onRetryPreview: () => void;
  onRetryCover: () => void;
}

function formatBytes(bytes: number | undefined, t: (source: string) => string) {
  if (bytes === undefined) return t("大小未知");
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

function formatDuration(seconds: number | undefined, t: (source: string) => string) {
  if (seconds === undefined) return t("时长未知");
  const minutes = Math.floor(seconds / 60);
  const remainder = Math.floor(seconds % 60).toString().padStart(2, "0");
  return `${minutes}:${remainder}`;
}

function artifactSummary(artifact: RecordingArtifact, t: (source: string) => string) {
  const resolution = artifact.width && artifact.height
    ? `${artifact.width}×${artifact.height}`
    : t("分辨率未知");
  const fps = artifact.framesPerSecond ? `${artifact.framesPerSecond} FPS` : t("帧率未知");
  return `${resolution} · ${fps} · ${formatBytes(artifact.fileSizeBytes, t)}`;
}

function statusText(artifact: RecordingArtifact, t: (source: string) => string) {
  if (artifact.status === "completed") return t("已完成");
  if (artifact.status === "processing") return t("生成中");
  if (artifact.status === "failed") return t("生成失败");
  if (artifact.status === "pending") return t("等待处理");
  return t("未生成");
}

function StatusIcon({ artifact }: { artifact: RecordingArtifact }) {
  if (artifact.status === "completed") return <CheckCircle2 size={16} />;
  if (artifact.status === "processing" || artifact.status === "pending") {
    return <LoaderCircle className="spinning" size={16} />;
  }
  return <AlertTriangle size={16} />;
}

export function RecordingResult({
  details,
  retrying,
  retryingCover,
  onBack,
  onOpenLocation,
  onRetryPreview,
  onRetryCover,
}: RecordingResultProps) {
  const { locale, t } = useI18n();
  const playable = details.preview.path ? details.preview : details.main;
  const playingPreview = playable === details.preview;

  return (
    <main className="shell result-shell">
      <header className="result-header">
        <button className="result-back" type="button" onClick={onBack}>
          <ArrowLeft size={18} />{t("返回")}
        </button>
        <div>
          <p>{t("录制完成")}</p>
          <h1>{details.name}</h1>
          <small>{localizeRelativeTime(locale, details.createdLabel)} · {formatDuration(details.durationSeconds, t)}</small>
        </div>
        <button className="result-open" type="button" onClick={onOpenLocation}>
          <FolderOpen size={16} />{t("打开位置")}
        </button>
      </header>

      <section className="result-content">
        <div className="result-player">
          {playable.path ? (
            <video
              key={playable.path}
              src={convertFileSrc(playable.path)}
              poster={details.highResolutionCover.status === "completed" && details.highResolutionCover.path
                ? convertFileSrc(details.highResolutionCover.path)
                : details.cover.status === "completed" && details.cover.path
                  ? convertFileSrc(details.cover.path)
                  : undefined}
              controls
              preload="metadata"
            />
          ) : (
            <div className="result-player-empty">{t("视频文件暂不可用")}</div>
          )}
          <span>{t(playingPreview ? "轻量预览" : "高清母版")}</span>
        </div>

        <div className="artifact-grid">
          <article className={`artifact-card ${details.main.status}`}>
            <header><strong>{t("高清母版")}</strong><span><StatusIcon artifact={details.main} />{statusText(details.main, t)}</span></header>
            <p>{artifactSummary(details.main, t)}</p>
            <small>{t("保留录制时的输出分辨率，用于存档和后续编辑。")}</small>
          </article>

          <article className={`artifact-card ${details.preview.status}`}>
            <header><strong>{t("轻量预览")}</strong><span><StatusIcon artifact={details.preview} />{statusText(details.preview, t)}</span></header>
            <p>{artifactSummary(details.preview, t)}</p>
            {details.preview.status === "failed" ? (
              <div className="artifact-failure">
                <small>{details.preview.error ?? t("预览生成失败，高清母版不受影响。")}</small>
                <button type="button" disabled={retrying} onClick={onRetryPreview}>
                  <RotateCcw size={14} />{t(retrying ? "正在重试…" : "重试预览")}
                </button>
              </div>
            ) : (
              <small>{t("适合快速播放和分享，后台生成不影响母版。")}</small>
            )}
          </article>


          <article className={`artifact-card ${details.cover.status}`}>
            <header><strong>{t("预览封面")}</strong><span><StatusIcon artifact={details.cover} />{statusText(details.cover, t)}</span></header>
            <p>{details.cover.width && details.cover.height ? `${details.cover.width}×${details.cover.height}` : t("分辨率未知")} · {formatBytes(details.cover.fileSizeBytes, t)}</p>
            {details.cover.status === "failed" ? (
              <div className="artifact-failure">
                <small>{details.cover.error ?? t("封面生成失败，视频文件不受影响。")}</small>
                <button type="button" disabled={retryingCover} onClick={onRetryCover}>
                  <RotateCcw size={14} />{t(retryingCover ? "正在重试…" : "重试封面")}
                </button>
              </div>
            ) : (
              <small>{t("WebP 小图，用于录制列表和快速加载。")}</small>
            )}
          </article>

          <article className={`artifact-card ${details.highResolutionCover.status}`}>
            <header><strong>{t("高清封面")}</strong><span><StatusIcon artifact={details.highResolutionCover} />{statusText(details.highResolutionCover, t)}</span></header>
            <p>{details.highResolutionCover.width && details.highResolutionCover.height ? `${details.highResolutionCover.width}×${details.highResolutionCover.height}` : t("分辨率未知")} · {formatBytes(details.highResolutionCover.fileSizeBytes, t)}</p>
            {details.highResolutionCover.status === "failed" ? (
              <div className="artifact-failure">
                <small>{details.highResolutionCover.error ?? t("高清封面生成失败，视频文件不受影响。")}</small>
                <button type="button" disabled={retryingCover} onClick={onRetryCover}>
                  <RotateCcw size={14} />{t(retryingCover ? "正在重试…" : "重试双封面")}
                </button>
              </div>
            ) : (
              <small>{t("WebP 高清图，最高 1920×1080，用于播放器封面和导出。")}</small>
            )}
          </article>
        </div>
      </section>
    </main>
  );
}

import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type Locale = "zh-CN" | "en";

const STORAGE_KEY = "screenharbor.locale";
const LOCALE_EVENT = "screenharbor:locale-changed";

const english: Record<string, string> = {
  "保留原始分辨率": "Preserves the original resolution",
  "主视频": "Main video", "主视频输出": "Main video output",
  "视频格式": "Video format", "视频画质": "Video quality",
  "低画质": "Low", "平衡": "Balanced", "高画质": "High",
  "兼容常见设备和播放器": "Compatible with common devices and players",
  "适合网页播放，录制结束后需要额外处理时间": "For web playback; takes extra processing time after recording",
  "文件更小，适合快速分享": "Smaller files for quick sharing",
  "兼顾清晰度与文件大小，适合日常网页使用": "Balances clarity and file size for everyday web use",
  "优先保留细节，文件更大": "Preserves more detail with larger files",
  "正在准备 ScreenHarbor…": "Preparing ScreenHarbor…",
  "ScreenHarbor 初始化失败": "ScreenHarbor failed to initialize",
  "无法读取应用状态。": "Unable to read the app state.",
  "重新加载": "Reload",
  "录制文件保存失败。": "The recording could not be saved.",
  "显示器配置发生变化，本轮框选已安全取消。请重新开始框选。": "The display configuration changed, so the selection was safely cancelled. Please select an area again.",
  "快捷键无法开始框选：{error}": "The shortcut could not start area selection: {error}",
  "部分录制已经不在当前录制库中，请刷新后重试。": "Some recordings are no longer in the library. Refresh and try again.",
  "移入 Windows 回收站？": "Move to the Windows Recycle Bin?",
  "{name}及其母版、预览和两张封面会一起移入回收站，之后仍可恢复。": "{name}, its master, preview, and both cover images will be moved to the Recycle Bin and can be restored later.",
  "{count} 条录制": "{count} recordings",
  "取消": "Cancel", "正在移动…": "Moving…", "移入回收站": "Move to Recycle Bin",
  "移除未完成录制？": "Remove incomplete recording?",
  "{name} 尚未形成可播放 MP4，其整个录制组会移入 Windows 回收站，之后仍可恢复。": "{name} does not have a playable MP4 yet. The entire recording group will be moved to the Windows Recycle Bin and can be restored later.",
  "录制设置": "Recording settings", "视频": "Video", "关闭设置": "Close settings",
  "语言": "Language", "中文（简体）": "Chinese (Simplified)", "英文": "English",
  "开始倒计时": "Start countdown", "倒计时时长": "Countdown duration", "{seconds} 秒": "{seconds} sec",
  "录制开始倒计时": "Recording start countdown", "母版输出": "Master output", "输出分辨率": "Output resolution",
  "原始": "Native", "最佳画质": "Best quality", "最高 {size}": "Up to {size}", "录制帧率": "Recording frame rate",
  "当前屏幕 {hz} Hz": "Current display: {hz} Hz", "未读取到屏幕刷新率": "Display refresh rate unavailable", "推荐": "Recommended",
  "该帧率高于当前屏幕刷新率，视频仍会按 {fps} FPS 输出，但可能包含重复帧并增加文件大小。": "This rate exceeds the current display refresh rate. The video will still be exported at {fps} FPS, but may contain duplicate frames and use more space.",
  "轻量预览": "Lightweight preview", "格式": "Format", "H.264 + AAC，兼容性最好": "H.264 + AAC, best compatibility",
  "VP9 + Opus，通常文件更小": "VP9 + Opus, usually smaller", "动态图片，无音频": "Animated image, no audio",
  "分辨率": "Resolution", "帧率": "Frame rate", "保存中…": "Saving…", "保存设置": "Save settings",
  "打开设置": "Open settings", "最小化": "Minimize", "最大化或还原": "Maximize or restore", "关闭": "Close",
  "录制声音": "Record audio", "麦克风": "Microphone", "选择麦克风": "Select microphone",
  "跟随系统默认": "Use system default", "（当前不可用）": " (currently unavailable)", "之前选择的麦克风（当前不可用）": "Previously selected microphone (currently unavailable)",
  "（当前默认）": " (current default)", "录制麦克风": "Record microphone", "系统声音": "System audio",
  "选择系统声音输出设备": "Select system audio output", "之前选择的输出设备（当前不可用）": "Previously selected output (currently unavailable)",
  "录制系统声音": "Record system audio", "输出文件": "Output files", "高清母版": "High-quality master",
  "原始分辨率": "Native resolution", "同时生成轻量预览": "Also create a lightweight preview", "生成轻量预览和封面": "Create lightweight preview and covers",
  "保存位置": "Save location", "选择中…": "Choosing…", "更改": "Change", "未完成录制": "Incomplete recordings",
  "异常退出留下的文件已原样保留，可打开位置检查或移入回收站。": "Files left by an unexpected exit have been preserved. Open their location to inspect them or move them to the Recycle Bin.",
  "打开位置": "Open location", "删除未完成录制 {name}": "Delete incomplete recording {name}", "最近录制": "Recent recordings",
  "收起": "Show less", "查看全部": "View all", "还没有录制": "No recordings yet", "完成的内容会出现在这里": "Completed recordings will appear here",
  "打开 {name} 所在文件夹": "Open the folder containing {name}", "打开所在文件夹": "Open containing folder", "删除 {name}": "Delete {name}",
  "当前录制配置": "Current recording configuration", "当前配置": "Current settings", "含轻量预览": "with lightweight preview",
  "无声音": "No audio", "开始录屏": "Start recording", "录制进行中": "Recording in progress",
  "时长未知": "Unknown duration", "轻量预览已生成": "Lightweight preview ready", "正在生成轻量预览": "Creating lightweight preview",
  "轻量预览生成失败": "Lightweight preview failed", "轻量预览等待处理": "Lightweight preview queued", "仅高清母版": "High-quality master only",
  "智能选择区域": "Smart area selection", "{countdown} 秒后开始": "Starting in {countdown} sec", "正在开始": "Starting",
  "等待选区": "Waiting for selection", "单击选窗口 · 拖动自由框选 · Esc 取消": "Click a window · Drag to select an area · Esc to cancel",
  "Esc 取消": "Esc to cancel", "正在启动录制": "Starting recording", "拖边框缩放 · 顶部手柄移动": "Drag edges to resize · Drag top handle to move",
  "重新框选": "Select again", "开始录制": "Start recording", "无法启动录制：{error}": "Unable to start recording: {error}",
  "无法读取录制状态：{error}": "Unable to read recording status: {error}", "无法刷新录制状态：{error}": "Unable to refresh recording status: {error}",
  "录制进程状态不可用：{error}": "Recording process status unavailable: {error}", "保存录制失败：{error}": "Unable to save recording: {error}",
  "无法停止录制：{error}": "Unable to stop recording: {error}", "无法切换暂停状态": "Unable to toggle pause",
  "无法切换音频静音：{error}": "Unable to toggle audio mute: {error}", "硬件 H.264": "Hardware H.264", "软件 H.264": "Software H.264",
  "{count} 帧": "{count} frames", "丢 {count} 帧": "{count} dropped frames", "音频丢 {count} 包": "{count} dropped audio packets",
  "正在保存": "Saving", "已暂停": "Paused", "录制中": "Recording", "系统声音设备已断开": "System audio device disconnected",
  "恢复系统声音": "Unmute system audio", "静音系统声音": "Mute system audio", "系统声": "System audio",
  "麦克风已断开": "Microphone disconnected", "恢复麦克风": "Unmute microphone", "静音麦克风": "Mute microphone",
  "继续": "Resume", "暂停": "Pause", "停止并保存": "Stop and save",
  "大小未知": "Unknown size", "分辨率未知": "Unknown resolution", "帧率未知": "Unknown frame rate", "已完成": "Completed",
  "生成中": "Processing", "生成失败": "Failed", "等待处理": "Queued", "未生成": "Not created", "录制完成": "Recording complete",
  "返回": "Back", "视频文件暂不可用": "Video file is currently unavailable",
  "保留录制时的输出分辨率，用于存档和后续编辑。": "Keeps the recording output resolution for archiving and editing.",
  "预览生成失败，高清母版不受影响。": "Preview creation failed; the master is unaffected.", "正在重试…": "Retrying…", "重试预览": "Retry preview",
  "适合快速播放和分享，后台生成不影响母版。": "Optimized for quick playback and sharing; background creation does not affect the master.",
  "预览封面": "Preview cover", "封面生成失败，视频文件不受影响。": "Cover creation failed; video files are unaffected.", "重试封面": "Retry cover",
  "WebP 小图，用于录制列表和快速加载。": "Small WebP image for the recording list and fast loading.", "高清封面": "High-resolution cover",
  "高清封面生成失败，视频文件不受影响。": "High-resolution cover creation failed; video files are unaffected.", "重试双封面": "Retry both covers",
  "WebP 高清图，最高 1920×1080，用于播放器封面和导出。": "High-resolution WebP, up to 1920×1080, for the player poster and export.",
  "无法读取应用状态，请重新启动 ScreenHarbor。": "Unable to read the app state. Restart ScreenHarbor.",
  "暂时无法进入框选，请稍后重试。": "Unable to start area selection right now. Try again shortly.",
  "无法保存输出设置：{error}": "Unable to save output settings: {error}", "无法更新音频输入：{error}": "Unable to update audio input: {error}",
  "无法更新轻量预览：{error}": "Unable to update lightweight preview: {error}", "无法更新录制倒计时：{error}": "Unable to update the recording countdown: {error}",
  "无法更改保存位置：{error}": "Unable to change the save location: {error}",
};

function initialLocale(): Locale {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === "zh-CN" || saved === "en") return saved;
  return navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
}

export function translate(locale: Locale, source: string, values: Record<string, string | number> = {}) {
  const template = locale === "en" ? english[source] ?? source : source;
  return template.replace(/\{(\w+)\}/g, (_, key: string) => String(values[key] ?? `{${key}}`));
}

export function localizeRelativeTime(locale: Locale, label: string) {
  if (locale !== "en") return label;
  if (label === "刚刚") return "Just now";
  const match = label.match(/^(\d+)\s*(分钟|小时|天)前$/);
  if (!match) return label;
  const count = Number(match[1]);
  const unit = match[2] === "分钟" ? "minute" : match[2] === "小时" ? "hour" : "day";
  return `${count} ${unit}${count === 1 ? "" : "s"} ago`;
}

interface I18nValue { locale: Locale; setLocale: (locale: Locale) => void; t: typeof translate extends (locale: Locale, ...args: infer P) => infer R ? (...args: P) => R : never }
const I18nContext = createContext<I18nValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, updateLocale] = useState<Locale>(initialLocale);
  useEffect(() => {
    document.documentElement.lang = locale;
    const onStorage = (event: StorageEvent) => {
      if (event.key === STORAGE_KEY && (event.newValue === "zh-CN" || event.newValue === "en")) updateLocale(event.newValue);
    };
    const onLocale = (event: Event) => updateLocale((event as CustomEvent<Locale>).detail);
    window.addEventListener("storage", onStorage);
    window.addEventListener(LOCALE_EVENT, onLocale);
    return () => { window.removeEventListener("storage", onStorage); window.removeEventListener(LOCALE_EVENT, onLocale); };
  }, [locale]);
  const value = useMemo<I18nValue>(() => ({
    locale,
    setLocale(next) { localStorage.setItem(STORAGE_KEY, next); updateLocale(next); window.dispatchEvent(new CustomEvent(LOCALE_EVENT, { detail: next })); },
    t: (source, values) => translate(locale, source, values),
  }), [locale]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n() {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useI18n must be used inside I18nProvider");
  return value;
}

export function currentLocale(): Locale { return initialLocale(); }

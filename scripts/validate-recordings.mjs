import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_DIRECTORY = join(homedir(), "Videos", "FrameFlow");

export function parseRate(value) {
  if (typeof value !== "string") return Number.NaN;
  const [numerator, denominator = "1"] = value.split("/").map(Number);
  return denominator === 0 ? Number.NaN : numerator / denominator;
}

export function validateProbe(probe, options = {}) {
  const failures = [];
  const format = probe?.format ?? {};
  const streams = Array.isArray(probe?.streams) ? probe.streams : [];
  const video = streams.find((stream) => stream.codec_type === "video");
  const audio = streams.find((stream) => stream.codec_type === "audio");
  const duration = Number(format.duration ?? video?.duration);

  if (!video) {
    failures.push("缺少视频轨");
  } else {
    if (video.codec_name !== "h264") failures.push(`视频编码为 ${video.codec_name ?? "未知"}，期望 h264`);
    if (options.width && Number(video.width) !== options.width) failures.push(`宽度为 ${video.width}，期望 ${options.width} 物理像素`);
    if (options.height && Number(video.height) !== options.height) failures.push(`高度为 ${video.height}，期望 ${options.height} 物理像素`);

    const frameRate = parseRate(video.avg_frame_rate);
    if (!Number.isFinite(frameRate) || frameRate <= 0) {
      failures.push("无法读取有效平均帧率");
    } else if (options.fps && Math.abs(frameRate - options.fps) > (options.fpsTolerance ?? 0.05)) {
      failures.push(`平均帧率为 ${frameRate.toFixed(3)} FPS，期望 ${options.fps} FPS`);
    }
  }

  if (!Number.isFinite(duration) || duration <= 0) {
    failures.push("无法读取有效时长");
  } else if (
    options.duration &&
    Math.abs(duration - options.duration) > (options.durationTolerance ?? 1)
  ) {
    failures.push(`时长为 ${duration.toFixed(3)} 秒，期望 ${options.duration}±${options.durationTolerance ?? 1} 秒`);
  }

  if (options.audio === "required" && !audio) failures.push("缺少音轨");
  if (options.audio === "forbidden" && audio) failures.push("存在未预期的音轨");
  if (audio) {
    if (audio.codec_name !== "aac") failures.push(`音频编码为 ${audio.codec_name ?? "未知"}，期望 aac`);
    if (Number(audio.sample_rate) !== 48_000) failures.push(`音频采样率为 ${audio.sample_rate ?? "未知"} Hz，期望 48000 Hz`);
    if (Number(audio.channels) !== 2) failures.push(`音频声道数为 ${audio.channels ?? "未知"}，期望 2`);
  }

  return {
    ok: failures.length === 0,
    failures,
    summary: {
      duration,
      width: Number(video?.width),
      height: Number(video?.height),
      fps: parseRate(video?.avg_frame_rate),
      videoCodec: video?.codec_name,
      audioCodec: audio?.codec_name,
      audioSampleRate: audio ? Number(audio.sample_rate) : undefined,
      audioChannels: audio ? Number(audio.channels) : undefined,
    },
  };
}

function parsePositiveNumber(value, option) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) throw new Error(`${option} 需要正数`);
  return parsed;
}

export function parseArguments(argv) {
  const options = { audio: "optional", latest: 2, fpsTolerance: 0.05, durationTolerance: 1 };
  const files = [];

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length) throw new Error(`${argument} 缺少参数`);
      return argv[index];
    };

    if (argument === "--directory") options.directory = resolve(next());
    else if (argument === "--latest") options.latest = parsePositiveNumber(next(), argument);
    else if (argument === "--expected-size") {
      const match = /^(\d+)x(\d+)$/i.exec(next());
      if (!match) throw new Error("--expected-size 格式应为 WIDTHxHEIGHT");
      options.width = Number(match[1]);
      options.height = Number(match[2]);
    } else if (argument === "--expected-fps") options.fps = parsePositiveNumber(next(), argument);
    else if (argument === "--fps-tolerance") options.fpsTolerance = parsePositiveNumber(next(), argument);
    else if (argument === "--expected-duration") options.duration = parsePositiveNumber(next(), argument);
    else if (argument === "--duration-tolerance") options.durationTolerance = parsePositiveNumber(next(), argument);
    else if (argument === "--audio") {
      options.audio = next();
      if (!["required", "optional", "forbidden"].includes(options.audio)) {
        throw new Error("--audio 仅支持 required、optional 或 forbidden");
      }
    } else if (argument === "--ffprobe") options.ffprobe = resolve(next());
    else if (argument === "--help" || argument === "-h") options.help = true;
    else if (argument.startsWith("--")) throw new Error(`未知参数：${argument}`);
    else files.push(resolve(argument));
  }

  return { files, options };
}

function collectMp4Files(directory) {
  if (!existsSync(directory)) return [];
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...collectMp4Files(path));
    else if (entry.isFile() && extname(entry.name).toLowerCase() === ".mp4") files.push(path);
  }
  return files;
}

export function resolveInputFiles(files, options) {
  if (files.length > 0) return files;
  const directory = options.directory ?? DEFAULT_DIRECTORY;
  return collectMp4Files(directory)
    .map((path) => ({ path, modified: statSync(path).mtimeMs }))
    .sort((left, right) => right.modified - left.modified)
    .slice(0, options.latest)
    .map(({ path }) => path);
}

function probeFile(path, ffprobe) {
  const output = execFileSync(
    ffprobe,
    ["-v", "error", "-show_entries", "format=duration,format_name:stream=codec_type,codec_name,width,height,avg_frame_rate,duration,sample_rate,channels", "-of", "json", path],
    { encoding: "utf8", windowsHide: true },
  );
  return JSON.parse(output);
}

function usage() {
  return `用法：npm run validate:recordings -- -- [文件...] [选项]

未指定文件时，默认检查 ~/Videos/FrameFlow 中最近两段 MP4。

  --directory DIR              从指定目录递归寻找成片
  --latest N                   自动选择最近 N 段，默认 2
  --expected-size 1920x1080    期望物理像素尺寸
  --expected-fps 30            期望平均帧率
  --expected-duration 10       期望时长（秒）
  --duration-tolerance 1       时长允许误差（秒）
  --audio required|optional|forbidden
  --ffprobe PATH               指定 ffprobe.exe 路径`;
}

function main(argv) {
  let parsed;
  try {
    parsed = parseArguments(argv);
  } catch (error) {
    console.error(`参数错误：${error.message}\n\n${usage()}`);
    return 2;
  }
  if (parsed.options.help) {
    console.log(usage());
    return 0;
  }

  const files = resolveInputFiles(parsed.files, parsed.options);
  if (files.length === 0) {
    console.error("没有找到待检查的 MP4 文件。请先完成录制，或显式传入文件路径。");
    return 2;
  }

  const ffprobe = parsed.options.ffprobe ?? process.env.FFPROBE_PATH ?? "ffprobe";
  let failed = false;
  for (const path of files) {
    try {
      const result = validateProbe(probeFile(path, ffprobe), parsed.options);
      const summary = result.summary;
      const audio = summary.audioCodec
        ? `${summary.audioCodec} ${summary.audioSampleRate} Hz ${summary.audioChannels}ch`
        : "无音轨";
      console.log(`${result.ok ? "通过" : "失败"} ${path}`);
      console.log(`  ${summary.width}x${summary.height} · ${summary.fps.toFixed(3)} FPS · ${summary.duration.toFixed(3)} 秒 · ${summary.videoCodec} · ${audio}`);
      for (const failure of result.failures) console.log(`  - ${failure}`);
      failed ||= !result.ok;
    } catch (error) {
      failed = true;
      const missing = error?.code === "ENOENT" ? "；系统未找到 ffprobe，请安装 FFmpeg 或使用 --ffprobe 指定路径" : "";
      console.error(`失败 ${path}\n  ${error.message}${missing}`);
    }
  }
  return failed ? 1 : 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  process.exitCode = main(process.argv.slice(2));
}

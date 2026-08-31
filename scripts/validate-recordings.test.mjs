import assert from "node:assert/strict";
import test from "node:test";

import { parseArguments, parseRate, validateProbe } from "./validate-recordings.mjs";

const validProbe = {
  format: { duration: "10.040", format_name: "mov,mp4,m4a,3gp,3g2,mj2" },
  streams: [
    { codec_type: "video", codec_name: "h264", width: 1920, height: 1080, r_frame_rate: "30/1", avg_frame_rate: "30/1", duration: "10.040" },
    { codec_type: "audio", codec_name: "aac", sample_rate: "48000", channels: 2, duration: "10.020" },
  ],
  audioPackets: [
    { pts_time: "0.000000", duration_time: "0.021333" },
    { pts_time: "0.021333", duration_time: "0.021333" },
  ],
};

test("parses rational frame rates", () => {
  assert.equal(parseRate("30000/1001").toFixed(3), "29.970");
  assert.ok(Number.isNaN(parseRate("0/0")));
});

test("accepts a valid H.264/AAC recording at the expected output size", () => {
  const result = validateProbe(validProbe, {
    width: 1920,
    height: 1080,
    fps: 30,
    duration: 10,
    audio: "required",
  });
  assert.equal(result.ok, true);
  assert.deepEqual(result.failures, []);
});

test("reports independent output contract violations", () => {
  const result = validateProbe(
    { format: { duration: "3" }, streams: [{ codec_type: "video", codec_name: "hevc", width: 960, height: 540, r_frame_rate: "20/1", avg_frame_rate: "20/1" }] },
    { width: 1920, height: 1080, fps: 30, duration: 10, audio: "required" },
  );
  assert.equal(result.ok, false);
  assert.equal(result.failures.length, 6);
});

test("accepts a nominal constant rate with a small real-time frame drop", () => {
  const result = validateProbe({
    format: { duration: "8.983" },
    streams: [
      { codec_type: "video", codec_name: "h264", width: 1082, height: 1030, r_frame_rate: "60/1", avg_frame_rate: "32280/539" },
      { codec_type: "audio", codec_name: "aac", sample_rate: "48000", channels: 2 },
    ],
  }, { fps: 60, audio: "required" });
  assert.equal(result.ok, true);
});

test("rejects sparse AAC packet timing that can make native players jump", () => {
  const result = validateProbe({
    format: { duration: "180.900" },
    streams: [
      { codec_type: "video", codec_name: "h264", width: 1920, height: 1080, r_frame_rate: "60/1", avg_frame_rate: "60/1", duration: "180.900" },
      { codec_type: "audio", codec_name: "aac", sample_rate: "48000", channels: 2, duration: "176.683" },
    ],
    audioPackets: [
      { pts_time: "0.000000", duration_time: "0.021333" },
      { pts_time: "0.021333", duration_time: "67.925333" },
      { pts_time: "67.946666", duration_time: "0.021333" },
    ],
  }, { audio: "required" });
  assert.equal(result.ok, false);
  assert.ok(result.failures.some((failure) => failure.includes("最长 AAC 包")));
  assert.ok(result.failures.some((failure) => failure.includes("音视频轨时长")));
});

test("parses explicit validation expectations", () => {
  const { files, options } = parseArguments([
    "capture.mp4",
    "--expected-size", "1920x1080",
    "--expected-fps", "60",
    "--expected-duration", "8",
    "--audio", "required",
  ]);
  assert.equal(files.length, 1);
  assert.equal(options.width, 1920);
  assert.equal(options.height, 1080);
  assert.equal(options.fps, 60);
  assert.equal(options.duration, 8);
  assert.equal(options.audio, "required");
});

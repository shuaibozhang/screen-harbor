import assert from "node:assert/strict";
import test from "node:test";

import { parseArguments, parseRate, validateProbe } from "./validate-recordings.mjs";

const validProbe = {
  format: { duration: "10.040", format_name: "mov,mp4,m4a,3gp,3g2,mj2" },
  streams: [
    { codec_type: "video", codec_name: "h264", width: 1920, height: 1080, avg_frame_rate: "30/1" },
    { codec_type: "audio", codec_name: "aac", sample_rate: "48000", channels: 2 },
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
    { format: { duration: "3" }, streams: [{ codec_type: "video", codec_name: "hevc", width: 960, height: 540, avg_frame_rate: "20/1" }] },
    { width: 1920, height: 1080, fps: 30, duration: 10, audio: "required" },
  );
  assert.equal(result.ok, false);
  assert.equal(result.failures.length, 6);
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

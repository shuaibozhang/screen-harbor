import { describe, expect, it } from "vitest";
import { normalizeOutputConfig, type OutputConfig } from "./output-config";

const config: OutputConfig = {
  main: { format: "mp4", fps: 24, includeAudio: true },
  preview: { format: "animated_avif", fps: 60, includeAudio: true },
  coverEnabled: true,
};

describe("normalizeOutputConfig", () => {
  it("caps preview fps and disables animated preview audio", () => {
    const normalized = normalizeOutputConfig(config);
    expect(normalized.preview).toMatchObject({ fps: 24, includeAudio: false });
  });
});

export type OutputFormat = "mp4" | "mov" | "webm" | "animated_webp" | "animated_avif";
export type OutputResolution =
  | { mode: "native" }
  | { mode: "fitWithin"; width: number; height: number };

export interface OutputSettings {
  format: OutputFormat;
  resolution: OutputResolution;
  fps: number;
  includeAudio: boolean;
}

export interface OutputConfig {
  main: OutputSettings;
  preview?: OutputSettings;
  coverEnabled: boolean;
}

export function normalizeOutputConfig(config: OutputConfig): OutputConfig {
  if (!config.preview) return config;
  const animated = config.preview.format === "animated_webp" || config.preview.format === "animated_avif";
  return {
    ...config,
    preview: {
      ...config.preview,
      fps: Math.min(config.preview.fps, config.main.fps),
      includeAudio: animated ? false : config.preview.includeAudio,
    },
  };
}

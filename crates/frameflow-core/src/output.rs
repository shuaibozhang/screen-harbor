use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Mp4,
    Mov,
    Webm,
    AnimatedWebp,
    AnimatedAvif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub fn encoder_safe(self) -> Self {
        Self {
            width: self.width.max(2) & !1,
            height: self.height.max(2) & !1,
        }
    }

    pub fn fit_within(self, maximum: Self) -> Self {
        let source = self.encoder_safe();
        let maximum = maximum.encoder_safe();
        if source.width <= maximum.width && source.height <= maximum.height {
            return source;
        }

        let width_limited = u64::from(maximum.width) * u64::from(source.height)
            <= u64::from(maximum.height) * u64::from(source.width);
        if width_limited {
            Self {
                width: maximum.width,
                height: ((u64::from(source.height) * u64::from(maximum.width))
                    / u64::from(source.width)) as u32,
            }
            .encoder_safe()
        } else {
            Self {
                width: ((u64::from(source.width) * u64::from(maximum.height))
                    / u64::from(source.height)) as u32,
                height: maximum.height,
            }
            .encoder_safe()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum OutputResolution {
    #[default]
    Native,
    FitWithin {
        width: u32,
        height: u32,
    },
}

impl OutputResolution {
    pub fn resolve(self, source: Resolution) -> Resolution {
        match self {
            Self::Native => source.encoder_safe(),
            Self::FitWithin { width, height } => source.fit_within(Resolution { width, height }),
        }
    }

    pub fn is_valid(self) -> bool {
        match self {
            Self::Native => true,
            Self::FitWithin { width, height } => width >= 2 && height >= 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSettings {
    pub format: OutputFormat,
    #[serde(default)]
    pub resolution: OutputResolution,
    pub fps: u16,
    pub include_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputConfig {
    pub main: OutputSettings,
    pub preview: Option<OutputSettings>,
    pub cover_enabled: bool,
}

impl OutputConfig {
    pub fn normalized(mut self) -> Self {
        if let Some(preview) = &mut self.preview {
            preview.fps = preview.fps.min(self.main.fps);
            if matches!(
                preview.format,
                OutputFormat::AnimatedWebp | OutputFormat::AnimatedAvif
            ) {
                preview.include_audio = false;
            }
        }
        self
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            main: OutputSettings {
                format: OutputFormat::Mp4,
                resolution: OutputResolution::Native,
                fps: 30,
                include_audio: true,
            },
            preview: Some(OutputSettings {
                format: OutputFormat::Webm,
                resolution: OutputResolution::FitWithin {
                    width: 854,
                    height: 480,
                },
                fps: 15,
                include_audio: true,
            }),
            cover_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_previews_never_contain_audio() {
        let mut config = OutputConfig::default();
        let preview = config.preview.as_mut().unwrap();
        preview.format = OutputFormat::AnimatedWebp;
        preview.include_audio = true;

        assert!(!config.normalized().preview.unwrap().include_audio);
    }

    #[test]
    fn preview_fps_is_capped_by_main_output() {
        let mut config = OutputConfig::default();
        config.main.fps = 24;
        config.preview.as_mut().unwrap().fps = 60;

        assert_eq!(config.normalized().preview.unwrap().fps, 24);
    }

    #[test]
    fn native_output_preserves_the_physical_source_size() {
        assert_eq!(
            OutputResolution::Native.resolve(Resolution {
                width: 3840,
                height: 2160,
            }),
            Resolution {
                width: 3840,
                height: 2160,
            }
        );
    }

    #[test]
    fn scaled_output_fits_without_stretching_or_upscaling() {
        let full_hd = OutputResolution::FitWithin {
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            full_hd.resolve(Resolution {
                width: 3840,
                height: 2160,
            }),
            Resolution {
                width: 1920,
                height: 1080,
            }
        );
        assert_eq!(
            full_hd.resolve(Resolution {
                width: 1200,
                height: 800,
            }),
            Resolution {
                width: 1200,
                height: 800,
            }
        );
        assert_eq!(
            full_hd.resolve(Resolution {
                width: 2400,
                height: 1600,
            }),
            Resolution {
                width: 1620,
                height: 1080,
            }
        );
    }
}

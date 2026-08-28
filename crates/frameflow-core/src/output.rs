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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSettings {
    pub format: OutputFormat,
    pub max_resolution: Resolution,
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
                max_resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                fps: 30,
                include_audio: true,
            },
            preview: Some(OutputSettings {
                format: OutputFormat::Webm,
                max_resolution: Resolution {
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
}

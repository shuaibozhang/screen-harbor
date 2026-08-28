use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub fn is_valid(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub id: String,
    pub bounds: PhysicalRect,
    pub primary: bool,
    pub dpi_x: u32,
    pub dpi_y: u32,
    /// Nominal desktop refresh rate reported by Windows. This is not a live
    /// measurement and may differ from the instantaneous VRR rate.
    pub refresh_rate_hz: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionSelection {
    pub display_id: String,
    /// Physical pixels in the Windows virtual-desktop coordinate space.
    pub desktop_rect: PhysicalRect,
    /// Physical pixels relative to the captured display texture.
    pub capture_rect: PhysicalRect,
}

impl RegionSelection {
    /// Builds a capture region from display-relative physical pixels. DPI is
    /// deliberately not applied: WGC textures and the encoder both operate in
    /// physical pixels.
    pub fn from_display_physical(
        display: &DisplayInfo,
        capture_rect: PhysicalRect,
    ) -> Result<Self, SelectionError> {
        let right = u32::try_from(capture_rect.x)
            .ok()
            .and_then(|x| x.checked_add(capture_rect.width));
        let bottom = u32::try_from(capture_rect.y)
            .ok()
            .and_then(|y| y.checked_add(capture_rect.height));
        if !capture_rect.is_valid() {
            return Err(SelectionError::InvalidRect);
        }
        if right.is_none_or(|right| right > display.bounds.width)
            || bottom.is_none_or(|bottom| bottom > display.bounds.height)
        {
            return Err(SelectionError::OutsideDisplay);
        }

        Ok(Self {
            display_id: display.id.clone(),
            desktop_rect: PhysicalRect {
                x: display.bounds.x + capture_rect.x,
                y: display.bounds.y + capture_rect.y,
                ..capture_rect
            },
            capture_rect,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    #[error("selection must have a positive size")]
    InvalidRect,
    #[error("selection must stay within one display")]
    OutsideDisplay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub id: String,
    pub title: String,
    pub bounds: PhysicalRect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureSource {
    Display {
        display_id: String,
    },
    Window {
        window_id: String,
    },
    Region {
        display_id: String,
        rect: PhysicalRect,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    pub fps: u16,
}

pub trait CaptureSession: Send {
    type Error;

    fn pause(&mut self) -> Result<(), Self::Error>;
    fn resume(&mut self) -> Result<(), Self::Error>;
    fn stop(self) -> Result<(), Self::Error>;
}

pub trait CaptureAdapter {
    type Error;
    type Session: CaptureSession<Error = Self::Error>;

    fn list_displays(&self) -> Result<Vec<DisplayInfo>, Self::Error>;
    fn list_windows(&self) -> Result<Vec<WindowInfo>, Self::Error>;
    fn start(
        &self,
        source: CaptureSource,
        request: CaptureRequest,
    ) -> Result<Self::Session, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_physical_regions() {
        assert!(!PhysicalRect {
            x: 0,
            y: 0,
            width: 0,
            height: 1080
        }
        .is_valid());
        assert!(PhysicalRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080
        }
        .is_valid());
    }

    fn display(dpi: u32) -> DisplayInfo {
        DisplayInfo {
            id: "display-1".to_owned(),
            bounds: PhysicalRect {
                x: -2560,
                y: 0,
                width: 2560,
                height: 1440,
            },
            primary: false,
            dpi_x: dpi,
            dpi_y: dpi,
            refresh_rate_hz: Some(60),
        }
    }

    #[test]
    fn preserves_physical_pixels_on_high_dpi_displays() {
        let selection = RegionSelection::from_display_physical(
            &display(168),
            PhysicalRect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
        )
        .unwrap();

        assert_eq!(
            selection.capture_rect,
            PhysicalRect {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            }
        );
        assert_eq!(selection.desktop_rect.x, -2550);
    }

    #[test]
    fn rejects_cross_display_selection_instead_of_silently_clamping() {
        let error = RegionSelection::from_display_physical(
            &display(192),
            PhysicalRect {
                x: 2500,
                y: 0,
                width: 100,
                height: 100,
            },
        )
        .unwrap_err();
        assert_eq!(error, SelectionError::OutsideDisplay);
    }
}

use frameflow_capture::{DisplayInfo, PhysicalRect};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaptureDisplayOption {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_percent: u32,
    pub(crate) primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_rate_hz: Option<u32>,
}

pub(crate) fn ordered_displays(mut displays: Vec<DisplayInfo>) -> Vec<DisplayInfo> {
    displays.sort_by_key(|display| {
        (
            !display.primary,
            display.bounds.x,
            display.bounds.y,
            display.id.clone(),
        )
    });
    displays
}

pub(crate) fn choose_display<'a>(
    displays: &'a [DisplayInfo],
    preferred_id: Option<&str>,
    main_window_bounds: Option<PhysicalRect>,
) -> Option<&'a DisplayInfo> {
    preferred_id
        .and_then(|id| displays.iter().find(|display| display.id == id))
        .or_else(|| {
            main_window_bounds
                .and_then(|bounds| displays.iter().find(|display| display.bounds == bounds))
        })
        .or_else(|| displays.iter().find(|display| display.primary))
        .or_else(|| displays.first())
}

pub(crate) fn display_options(displays: &[DisplayInfo]) -> Vec<CaptureDisplayOption> {
    displays
        .iter()
        .enumerate()
        .map(|(index, display)| CaptureDisplayOption {
            id: display.id.clone(),
            label: if display.primary {
                format!("屏幕 {}（主屏幕）", index + 1)
            } else {
                format!("屏幕 {}", index + 1)
            },
            width: display.bounds.width,
            height: display.bounds.height,
            scale_percent: ((display.dpi_x as f64 / 96.0) * 100.0).round() as u32,
            primary: display.primary,
            refresh_rate_hz: display.refresh_rate_hz,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, x: i32, primary: bool, dpi: u32) -> DisplayInfo {
        DisplayInfo {
            id: id.to_owned(),
            bounds: PhysicalRect {
                x,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary,
            dpi_x: dpi,
            dpi_y: dpi,
            refresh_rate_hz: Some(60),
        }
    }

    #[test]
    fn explicit_preference_wins_over_main_window_and_primary_display() {
        let displays = vec![
            display(r"\\.\DISPLAY1", 0, true, 96),
            display(r"\\.\DISPLAY2", -1920, false, 144),
        ];

        let chosen =
            choose_display(&displays, Some(r"\\.\DISPLAY2"), Some(displays[0].bounds)).unwrap();

        assert_eq!(chosen.id, r"\\.\DISPLAY2");
    }

    #[test]
    fn disconnected_preference_falls_back_to_main_window_display() {
        let displays = vec![
            display(r"\\.\DISPLAY1", 0, true, 96),
            display(r"\\.\DISPLAY2", -1920, false, 144),
        ];

        let chosen =
            choose_display(&displays, Some(r"\\.\DISPLAY3"), Some(displays[1].bounds)).unwrap();

        assert_eq!(chosen.id, r"\\.\DISPLAY2");
    }

    #[test]
    fn display_options_report_physical_pixels_and_effective_scale() {
        let displays = ordered_displays(vec![
            display(r"\\.\DISPLAY2", -1920, false, 144),
            display(r"\\.\DISPLAY1", 0, true, 168),
        ]);

        let options = display_options(&displays);

        assert_eq!(options[0].label, "屏幕 1（主屏幕）");
        assert_eq!(options[0].scale_percent, 175);
        assert_eq!((options[0].width, options[0].height), (1920, 1080));
        assert_eq!(options[1].label, "屏幕 2");
    }
}

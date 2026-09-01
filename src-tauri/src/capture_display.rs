use frameflow_capture::{DisplayInfo, PhysicalRect};

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

/// Chooses a display only for main-window context such as FPS guidance. Actual
/// capture display selection happens interactively when the user draws.
pub(crate) fn choose_reference_display(
    displays: &[DisplayInfo],
    main_window_bounds: Option<PhysicalRect>,
) -> Option<&DisplayInfo> {
    main_window_bounds
        .and_then(|bounds| displays.iter().find(|display| display.bounds == bounds))
        .or_else(|| displays.iter().find(|display| display.primary))
        .or_else(|| displays.first())
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
    fn main_window_display_wins_over_primary_display() {
        let displays = vec![
            display(r"\\.\DISPLAY1", 0, true, 96),
            display(r"\\.\DISPLAY2", -1920, false, 144),
        ];

        let chosen = choose_reference_display(&displays, Some(displays[1].bounds)).unwrap();

        assert_eq!(chosen.id, r"\\.\DISPLAY2");
    }

    #[test]
    fn missing_main_window_display_falls_back_to_primary() {
        let displays = vec![
            display(r"\\.\DISPLAY1", 0, true, 96),
            display(r"\\.\DISPLAY2", -1920, false, 144),
        ];

        let chosen = choose_reference_display(
            &displays,
            Some(PhysicalRect {
                x: 5000,
                y: 0,
                width: 800,
                height: 600,
            }),
        )
        .unwrap();

        assert_eq!(chosen.id, r"\\.\DISPLAY1");
    }

    #[test]
    fn displays_are_ordered_primary_then_by_desktop_position() {
        let displays = ordered_displays(vec![
            display(r"\\.\DISPLAY2", -1920, false, 144),
            display(r"\\.\DISPLAY1", 0, true, 168),
        ]);

        assert_eq!(displays[0].id, r"\\.\DISPLAY1");
        assert_eq!(displays[1].id, r"\\.\DISPLAY2");
    }
}

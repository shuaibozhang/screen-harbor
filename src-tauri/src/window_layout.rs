use frameflow_capture::PhysicalRect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureToolbar {
    Selection,
    Recording,
}

impl CaptureToolbar {
    fn logical_size(self) -> (u32, u32) {
        match self {
            Self::Selection => (520, 56),
            Self::Recording => (680, 76),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MonitorGeometry {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ToolbarLayout {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) fn capture_toolbar_layout(
    toolbar: CaptureToolbar,
    selection: PhysicalRect,
    monitor: MonitorGeometry,
    scale_factor: f64,
) -> ToolbarLayout {
    const LOGICAL_GAP: f64 = 12.0;

    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let (logical_width, logical_height) = toolbar.logical_size();
    let width =
        ((f64::from(logical_width) * scale_factor).round() as u32).clamp(1, monitor.width.max(1));
    let height =
        ((f64::from(logical_height) * scale_factor).round() as u32).clamp(1, monitor.height.max(1));
    let gap = (LOGICAL_GAP * scale_factor).round() as i64;

    let monitor_left = i64::from(monitor.x);
    let monitor_top = i64::from(monitor.y);
    let monitor_right = monitor_left + i64::from(monitor.width);
    let monitor_bottom = monitor_top + i64::from(monitor.height);
    let selection_left = i64::from(selection.x);
    let selection_top = i64::from(selection.y);
    let selection_right = selection_left + i64::from(selection.width);
    let selection_bottom = selection_top + i64::from(selection.height);
    let width_i64 = i64::from(width);
    let height_i64 = i64::from(height);

    let centered_x = selection_left + (selection_right - selection_left - width_i64) / 2;
    let x = centered_x.clamp(monitor_left, monitor_right - width_i64);
    let above = selection_top - height_i64 - gap;
    let below = selection_bottom + gap;
    let y = if above >= monitor_top {
        above
    } else if below + height_i64 <= monitor_bottom {
        below
    } else {
        (selection_top + gap).clamp(monitor_top, monitor_bottom - height_i64)
    };

    ToolbarLayout {
        x: x as i32,
        y: y as i32,
        width,
        height,
    }
}

pub(crate) fn selection_hint_layout(monitor: MonitorGeometry, scale_factor: f64) -> ToolbarLayout {
    const LOGICAL_TOP_MARGIN: f64 = 24.0;

    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let (logical_width, logical_height) = CaptureToolbar::Selection.logical_size();
    let width =
        ((f64::from(logical_width) * scale_factor).round() as u32).clamp(1, monitor.width.max(1));
    let height =
        ((f64::from(logical_height) * scale_factor).round() as u32).clamp(1, monitor.height.max(1));
    let margin = (LOGICAL_TOP_MARGIN * scale_factor).round() as i64;
    let monitor_left = i64::from(monitor.x);
    let monitor_top = i64::from(monitor.y);
    let centered_x = monitor_left + (i64::from(monitor.width) - i64::from(width)) / 2;
    let y = (monitor_top + margin).clamp(
        monitor_top,
        monitor_top + i64::from(monitor.height) - i64::from(height),
    );

    ToolbarLayout {
        x: centered_x as i32,
        y: y as i32,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_recording_toolbars_keep_independent_sizes() {
        let monitor = MonitorGeometry {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        let selection = PhysicalRect {
            x: 500,
            y: 500,
            width: 1920,
            height: 1080,
        };

        assert_eq!(
            capture_toolbar_layout(CaptureToolbar::Selection, selection, monitor, 1.75),
            ToolbarLayout {
                x: 1005,
                y: 381,
                width: 910,
                height: 98,
            }
        );
        assert_eq!(
            capture_toolbar_layout(CaptureToolbar::Recording, selection, monitor, 1.75),
            ToolbarLayout {
                x: 865,
                y: 346,
                width: 1190,
                height: 133,
            }
        );
    }

    #[test]
    fn layout_handles_a_monitor_left_of_the_primary_display() {
        let monitor = MonitorGeometry {
            x: -2560,
            y: -320,
            width: 2560,
            height: 1440,
        };
        let selection = PhysicalRect {
            x: -2400,
            y: -250,
            width: 1200,
            height: 900,
        };

        let layout = capture_toolbar_layout(CaptureToolbar::Selection, selection, monitor, 1.5);

        assert_eq!(layout.width, 780);
        assert_eq!(layout.height, 84);
        assert!(layout.x >= -2560);
        assert!(layout.x + layout.width as i32 <= 0);
        assert!(layout.y >= -320);
        assert!(layout.y + layout.height as i32 <= 1120);
    }

    #[test]
    fn layout_stays_inside_a_small_monitor_when_no_outer_space_exists() {
        let monitor = MonitorGeometry {
            x: 1920,
            y: 0,
            width: 640,
            height: 480,
        };
        let selection = PhysicalRect {
            x: 1920,
            y: 0,
            width: 640,
            height: 480,
        };

        let layout = capture_toolbar_layout(CaptureToolbar::Recording, selection, monitor, 2.0);

        assert_eq!(layout.width, 640);
        assert_eq!(layout.x, 1920);
        assert!(layout.y >= 0);
        assert!(layout.y + layout.height as i32 <= 480);
    }

    #[test]
    fn invalid_scale_factor_falls_back_to_one() {
        let monitor = MonitorGeometry {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let selection = PhysicalRect {
            x: 100,
            y: 100,
            width: 1000,
            height: 600,
        };

        let layout =
            capture_toolbar_layout(CaptureToolbar::Selection, selection, monitor, f64::NAN);

        assert_eq!((layout.width, layout.height), (520, 56));
    }

    #[test]
    fn selection_hint_is_centered_near_the_top_of_the_target_monitor() {
        let layout = selection_hint_layout(
            MonitorGeometry {
                x: -2560,
                y: -320,
                width: 2560,
                height: 1440,
            },
            1.5,
        );

        assert_eq!(
            layout,
            ToolbarLayout {
                x: -1670,
                y: -284,
                width: 780,
                height: 84,
            }
        );
    }
}

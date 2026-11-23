use windows::Win32::Foundation::RECT;

use crate::Position;

fn is_within_dp_region(point: (i32, i32), display: &RECT) -> bool {
    point.0 >= display.left
        && point.0 < display.right
        && point.1 >= display.top
        && point.1 < display.bottom
}

fn in_display_region(point: (i32, i32), displays: &[RECT]) -> bool {
    displays.iter().any(|d| is_within_dp_region(point, d))
}

pub(crate) fn entered_barrier(
    point: (i32, i32),
    delta: (i32, i32),
    displays: &[RECT],
) -> Option<Position> {
    let (x, y) = point;
    let (dx, dy) = delta;
    let display = displays.iter().find(|d| is_within_dp_region(point, d))?;
    let candidates = [
        (dx < 0 && x == display.left, (x - 1, y), Position::Left),
        (
            dx > 0 && x == display.right - 1,
            (x + 1, y),
            Position::Right,
        ),
        (dy < 0 && y == display.top, (x, y - 1), Position::Top),
        (
            dy > 0 && y == display.bottom - 1,
            (x, y + 1),
            Position::Bottom,
        ),
    ];
    candidates
        .into_iter()
        .find(|(outward, next, _)| *outward && !in_display_region(*next, displays))
        .map(|(_, _, side)| side)
}

pub(crate) fn covers_monitor(window: &RECT, monitor: &RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_requires_the_whole_monitor_including_negative_origins() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(covers_monitor(&monitor, &monitor));
        assert!(!covers_monitor(
            &RECT {
                bottom: 1040,
                ..monitor
            },
            &monitor
        ));
        assert!(!covers_monitor(
            &RECT {
                left: -1900,
                ..monitor
            },
            &monitor
        ));
        assert!(covers_monitor(
            &RECT {
                left: -1928,
                right: 8,
                ..monitor
            },
            &monitor
        ));
    }

    #[test]
    fn captures_outward_motion_at_the_last_pixel_but_not_between_monitors() {
        let left = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let right = RECT {
            left: 100,
            top: 0,
            right: 200,
            bottom: 100,
        };
        assert_eq!(
            entered_barrier((99, 50), (5, 0), &[left]),
            Some(Position::Right)
        );
        assert_eq!(entered_barrier((99, 50), (5, 0), &[left, right]), None);
        assert_eq!(
            entered_barrier((199, 50), (5, 0), &[left, right]),
            Some(Position::Right)
        );
        assert_eq!(entered_barrier((199, 50), (-5, 0), &[left, right]), None);
        assert_eq!(
            entered_barrier((100, 50), (-5, 0), &[right]),
            Some(Position::Left)
        );
        assert_eq!(
            entered_barrier((100, 0), (0, -5), &[right]),
            Some(Position::Top)
        );
        assert_eq!(
            entered_barrier((100, 99), (0, 5), &[right]),
            Some(Position::Bottom)
        );
        let shifted = RECT {
            left: -1920,
            right: 0,
            ..left
        };
        assert_eq!(
            entered_barrier((-1920, 50), (-5, 0), &[shifted]),
            Some(Position::Left)
        );
    }
}

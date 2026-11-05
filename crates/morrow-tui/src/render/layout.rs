//! Responsive composition.
//!
//! Breakpoints come from content pressure, not from device names: a terminal
//! only ever tells us how many cells it has, so the layout asks what the content
//! needs and adapts. Wider stages earn more structure rather than stretched
//! leftovers, and very wide stages cap the measure rather than letting prose run
//! off the side of the screen.

use ratatui::prelude::*;

/// Composition tier for a given viewport width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// Cramped: single column, status collapses, no path display.
    Compact,
    /// The everyday case: full status bar, standard transcript.
    Standard,
    /// Room for the full status line and a richer hint row.
    Wide,
    /// Wide enough that prose needs a measure cap.
    Ultrawide,
}

/// Vertical composition tier for a given viewport height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Height {
    /// Too short for the welcome hero or a multi-row input.
    Tight,
    /// Standard.
    Normal,
}

/// Everything the renderer needs to know about the available space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// Full frame area.
    pub area: Rect,
    /// The working area after outer margins.
    pub body: Rect,
    /// Content width available for prose, after the gutter.
    pub content: u16,
    /// Horizontal side margin in cells.
    pub margin: u16,
    pub width: Width,
    pub height: Height,
}

impl Metrics {
    /// Derive layout metrics from the frame area.
    pub fn new(area: Rect) -> Metrics {
        let width = area.width;
        let tier = if width < 40 {
            Width::Compact
        } else if width < 80 {
            Width::Standard
        } else if width < 120 {
            Width::Wide
        } else {
            Width::Ultrawide
        };

        // Margins breathe on wide terminals and collapse when space is scarce.
        let margin = match tier {
            Width::Compact => 0,
            Width::Standard => 2,
            Width::Wide | Width::Ultrawide => 4,
        };

        let body = Rect {
            x: area.x + margin,
            y: area.y,
            width: width.saturating_sub(margin * 2),
            height: area.height,
        };

        // The measure cap keeps line length readable on ultrawide screens. Below
        // the cap, prose uses the full width.
        let content = body.width.min(96).saturating_sub(2);

        Metrics {
            area,
            body,
            content,
            margin,
            width: tier,
            height: if area.height < 12 {
                Height::Tight
            } else {
                Height::Normal
            },
        }
    }

    /// Whether to show the working directory in the status bar.
    ///
    /// Only wide-enough tiers have room for it, and dropping it is better than
    /// truncating a path to an unreadable stub.
    pub fn shows_path(&self) -> bool {
        matches!(self.width, Width::Wide | Width::Ultrawide)
    }

    /// Height of the input area, including its border.
    pub fn input_height(&self) -> u16 {
        match self.height {
            Height::Tight => 1,
            Height::Normal => 3,
        }
    }
}

/// Rows of the vertical stack: status, transcript, hint, input, padding.
pub struct Rows {
    pub status: Rect,
    pub transcript: Rect,
    pub hint: Rect,
    pub input: Rect,
}

/// Split the body area into its vertical bands.
///
/// Outer margins are `Max(1)` so they collapse gracefully rather than squeezing
/// the transcript on a short terminal.
pub fn rows(m: &Metrics) -> Rows {
    // A one-row input needs no surrounding air.
    let gaps = match m.height {
        Height::Tight => [0, 0, 0],
        Height::Normal => [1, 1, 1],
    };
    let [status, _gap1, transcript, hint, _gap2, input, _gap3] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Max(gaps[0]),
        Constraint::Min(if m.height == Height::Tight { 1 } else { 3 }),
        Constraint::Length(if m.height == Height::Tight { 0 } else { 1 }),
        Constraint::Max(gaps[1]),
        Constraint::Length(m.input_height()),
        Constraint::Max(gaps[2]),
    ])
    .areas(m.body);

    Rows {
        status,
        transcript,
        hint,
        input,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(w: u16, h: u16) -> Metrics {
        Metrics::new(Rect::new(0, 0, w, h))
    }

    #[test]
    fn breakpoints_split_at_the_documented_widths() {
        assert_eq!(m(39, 24).width, Width::Compact);
        assert_eq!(m(40, 24).width, Width::Standard);
        assert_eq!(m(79, 24).width, Width::Standard);
        assert_eq!(m(80, 24).width, Width::Wide);
        assert_eq!(m(119, 24).width, Width::Wide);
        assert_eq!(m(120, 24).width, Width::Ultrawide);
    }

    #[test]
    fn path_only_appears_when_there_is_room() {
        assert!(!m(39, 24).shows_path());
        assert!(!m(79, 24).shows_path());
        assert!(m(80, 24).shows_path());
        assert!(m(200, 24).shows_path());
    }

    #[test]
    fn ultrawide_caps_the_measure_instead_of_stretching_prose() {
        let narrow = m(80, 24);
        let wide = m(300, 60);
        assert!(narrow.content > 0);
        assert!(
            wide.content <= 96,
            "measure ran away to {} cols",
            wide.content
        );
        assert!(
            wide.content < wide.body.width,
            "measure should be capped below the available width"
        );
    }

    #[test]
    fn margins_never_exceed_the_area_width() {
        for w in [1u16, 2, 5, 39, 40, 80, 200] {
            let met = m(w, 24);
            assert!(
                met.body.width <= met.area.width,
                "body wider than area at {w}"
            );
            assert_eq!(met.body.x, met.margin);
        }
    }

    #[test]
    fn rows_stay_inside_the_body() {
        for (w, h) in [(39u16, 10u16), (40, 12), (80, 24), (200, 60), (20, 5)] {
            let met = m(w, h);
            let r = rows(&met);
            assert!(r.input.y + r.input.height <= met.body.y + met.body.height + 1);
            assert!(r.status.height <= 1);
        }
    }

    #[test]
    fn tight_terminals_use_a_single_row_input() {
        let met = m(80, 10);
        assert_eq!(met.height, Height::Tight);
        assert_eq!(met.input_height(), 1);
        let met = m(80, 24);
        assert_eq!(met.height, Height::Normal);
        assert_eq!(met.input_height(), 3);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        for w in 0..12u16 {
            for h in 0..6u16 {
                let met = m(w, h);
                let _ = rows(&met);
            }
        }
    }
}

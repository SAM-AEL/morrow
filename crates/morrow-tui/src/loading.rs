//! Creative rainbow loading animations for morrow.
//!
//! Two signature pieces, both driven by a single `tick` counter that the TUI
//! bumps on a ~90ms interval while the character is working:
//!
//! * [`rainbow_line`] — a full-width flowing gradient beam with a bright
//!   "comet head" that travels along the line. Unique vs. a plain spinner.
//! * [`rainbow_grid`] — a small square woven out of mini rainbow cells
//!   (4 rows x 6 cols) whose hues drift diagonally and pulse in brightness,
//!   like a tiny living quilt the character is "dreaming" on.
//!
//! Plus [`thinking_verb`] — the character isn't just "loading", it is
//! remembering / reasoning / learning / dreaming / evolving.

use ratatui::prelude::*;
use ratatui::text::{Line, Span};

/// Verbs that reinforce the character-that-learns positioning.
const VERBS: &[&str] = &[
    "remembering",
    "reasoning",
    "learning",
    "dreaming",
    "evolving",
    "growing",
];

pub fn thinking_verb(tick: u64) -> &'static str {
    VERBS[(tick as usize / 12) % VERBS.len()]
}

/// Standard HSV -> RGB (h in [0,360), s/v in [0,1]).
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 % 360 / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub fn rainbow_rgb(pos: f64, tick: u64, glow: f64) -> Color {
    let hue = (pos * 360.0 + tick as f64 * 9.0) % 360.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.9, (0.55 + 0.45 * glow).clamp(0.15, 1.0));
    Color::Rgb(r, g, b)
}

/// Animated rainbow text for the intro title — one hue per character,
/// flowing with `tick`. Bold for extra pop.
pub fn rainbow_title(text: &str, tick: u64) -> Line<'static> {
    let n = text.chars().count().max(1);
    let mut spans = Vec::with_capacity(n);
    for (i, ch) in text.chars().enumerate() {
        let pos = i as f64 / n as f64;
        spans.push(Span::styled(
            ch.to_string(),
            Style::new()
                .fg(rainbow_rgb(pos, tick, 1.0))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// Animated header badge background — cycles through the rainbow slowly.
pub fn header_bg(tick: u64) -> Color {
    let (r, g, b) = hsv_to_rgb((tick as f64 * 6.0) % 360.0, 0.75, 0.55);
    Color::Rgb(r, g, b)
}

/// A full-width flowing rainbow beam with a travelling comet head.
///
/// `width` is in cells; each cell is one `━` span with its own color.
/// Brightness peaks at the comet head (`tick % width`) and trails behind it.
pub fn rainbow_line(width: usize, tick: u64) -> Line<'static> {
    let width = width.max(8);
    let head = (tick as usize) % width;
    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        // distance behind the head (wrapping), for the comet trail
        let behind = (head + width - i) % width;
        let trail = (-(behind as f64) / 9.0).exp(); // 1.0 at head -> ~0 far away
        let glow = 0.25 + 0.75 * trail;
        let color = rainbow_rgb(i as f64 / width as f64, tick, glow);
        let glyph = if i == head {
            "●"
        } else if trail > 0.55 {
            "━"
        } else if trail > 0.25 {
            "─"
        } else {
            "·"
        };
        spans.push(Span::styled(glyph.to_string(), Style::new().fg(color)));
    }
    Line::from(spans)
}

/// A little square woven from mini rainbow grids.
///
/// 4 rows x 6 cols of `◼` cells; hue drifts diagonally with time and each
/// cell breathes in brightness on a sine wave, so the quilt shimmers.
/// A 1-cell frame drawn with `▚` corners pulses in the complementary hue.
pub fn rainbow_grid(tick: u64) -> Vec<Line<'static>> {
    const ROWS: usize = 4;
    const COLS: usize = 6;
    let t = tick as f64;
    let mut out = Vec::with_capacity(ROWS + 2);

    // top frame
    out.push(Line::from(vec![Span::styled(
        format!("▚{}▞", "▬".repeat(COLS)),
        Style::new().fg(rainbow_rgb(0.5, tick, 0.8)),
    )]));

    for y in 0..ROWS {
        let mut spans = Vec::with_capacity(COLS + 2);
        spans.push(Span::styled(
            "▌".to_string(),
            Style::new().fg(rainbow_rgb(y as f64 * 0.13, tick, 0.8)),
        ));
        for x in 0..COLS {
            // diagonal drift + per-cell breathing
            let pos = (x as f64 * 0.09 + y as f64 * 0.16) % 1.0;
            let breathe = 0.5 + 0.5 * ((t / 4.0 + x as f64 * 0.9 + y as f64 * 1.3).sin());
            let color = rainbow_rgb(pos, tick, 0.35 + 0.65 * breathe);
            // checkerboard of two mini-grid glyphs for a woven feel
            let glyph = if (x + y) % 2 == 0 { "◼" } else { "◻" };
            spans.push(Span::styled(glyph.to_string(), Style::new().fg(color)));
        }
        spans.push(Span::styled(
            "▐".to_string(),
            Style::new().fg(rainbow_rgb(y as f64 * 0.13 + 0.5, tick, 0.8)),
        ));
        out.push(Line::from(spans));
    }

    // bottom frame
    out.push(Line::from(vec![Span::styled(
        format!("▚{}▞", "▬".repeat(COLS)),
        Style::new().fg(rainbow_rgb(0.0, tick, 0.8)),
    )]));
    out
}

/// One-line status: `✨ <verb><dots>` with rainbow word coloring.
pub fn thinking_line(tick: u64) -> Line<'static> {
    let verb = thinking_verb(tick);
    let dots = ".".repeat(1 + (tick as usize / 6) % 3);
    let mut spans = vec![
        Span::styled("✨ ".to_string(), Style::new().fg(Color::Yellow)),
        Span::styled(
            verb.to_string(),
            Style::new()
                .fg(rainbow_rgb(0.15, tick, 1.0))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(dots, Style::new().dim()),
        Span::styled(
            "  · each turn makes me stronger".to_string(),
            Style::new().dim(),
        ),
    ];
    // sparkle that orbits the line
    if tick % 8 == 0 {
        spans.push(Span::styled(" ✦".to_string(), Style::new().fg(Color::Cyan)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_has_requested_width() {
        let line = rainbow_line(24, 7);
        assert_eq!(line.spans.len(), 24);
    }

    #[test]
    fn grid_is_square_shaped() {
        let grid = rainbow_grid(3);
        assert_eq!(grid.len(), 6); // 4 rows + 2 frame lines
        assert_eq!(grid[1].spans.len(), 8); // 6 cells + 2 edges
    }

    #[test]
    fn verbs_cycle() {
        assert!(!thinking_verb(0).is_empty());
        assert!(!thinking_verb(100).is_empty());
    }
}

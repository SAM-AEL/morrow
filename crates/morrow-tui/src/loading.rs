//! Motion for the working state.
//!
//! Motion is deliberately minimal and functional: a spinner while the character
//! is busy and a slowly drifting wordmark on the welcome screen. Nothing here
//! animates during ordinary reading, because an interface that moves when nothing
//! is happening is an interface that costs attention it has not earned.

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

/// The current thinking verb, cycling slowly relative to the frame rate.
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

/// A color from the rainbow ramp at position `pos`, brightened by `glow`.
pub fn rainbow_rgb(pos: f64, tick: u64, glow: f64) -> Color {
    let hue = (pos * 360.0 + tick as f64 * 9.0) % 360.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.9, (0.55 + 0.45 * glow).clamp(0.15, 1.0));
    Color::Rgb(r, g, b)
}

/// Animated wordmark — one hue per character, drifting with `tick`.
pub fn rainbow_title(text: &str, tick: u64) -> Line<'static> {
    let n = text.chars().count().max(1);
    if text.is_empty() {
        // Keep the line non-empty so callers can rely on it occupying a row.
        return Line::from(Span::styled(
            String::new(),
            Style::new().fg(rainbow_rgb(0.0, tick, 1.0)),
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_preserves_its_text() {
        let line = rainbow_title("morrow", 7);
        let joined: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "morrow");
    }

    #[test]
    fn title_handles_wide_and_empty_input() {
        // An empty title still produces one span so the line has content.
        assert_eq!(rainbow_title("", 0).spans.len(), 1);
        let line = rainbow_title("日本", 3);
        assert_eq!(line.spans.len(), 2);
        let joined: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "日本");
    }

    #[test]
    fn verbs_cycle_and_are_never_empty() {
        let a = thinking_verb(0);
        let b = thinking_verb(12 * VERBS.len() as u64);
        assert_eq!(a, b, "cycle should wrap");
        assert!(VERBS.contains(&thinking_verb(999)));
    }

    #[test]
    fn rainbow_rgb_produces_valid_channels() {
        for i in 0..200u64 {
            // The cast into u8 is what clamps; the point is that every hue in
            // the ramp produces a color rather than panicking.
            let Color::Rgb(..) = rainbow_rgb(i as f64 / 7.0, i, 0.5) else {
                panic!("expected an Rgb color");
            };
        }
        // Glow stays within the clamp range the implementation assumes.
        for glow in [0.0f64, 0.5, 1.0, 5.0] {
            let Color::Rgb(..) = rainbow_rgb(0.3, 4, glow) else {
                panic!("expected an Rgb color");
            };
        }
    }
}

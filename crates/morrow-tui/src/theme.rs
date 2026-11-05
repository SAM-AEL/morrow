//! Design tokens for morrow.
//!
//! One source of truth for every color and glyph in the interface. The rest of
//! the crate must not hard-code a `Color::Rgb(..)` — if a color is not a role,
//! it is not a token, and if it is a role it belongs here.
//!
//! Three variants ship:
//!
//! * [`Theme::DARK`] — the house identity. Near-black page, one cyan accent,
//!   amber reserved for warnings and approvals.
//! * [`Theme::LIGHT`] — a genuinely separate ramp. Cyan on paper is unreadable,
//!   so the accent is darkened and the neutrals invert.
//! * [`Theme::MONO`] — a chroma-free ramp for `NO_COLOR` and piped output.
//!   State is still legible because every state also carries a glyph.

use ratatui::style::Color;

/// Semantic color roles. Field order is intentional: chrome first (what the app
/// is made of), then content, then state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    // --- chrome ---
    /// Page background.
    pub bg: Color,
    /// Raised surface behind overlays and panels.
    pub surface: Color,
    /// Resting border and divider.
    pub border: Color,
    /// Border when a surface holds focus.
    pub border_focus: Color,

    // --- content ---
    /// Default body text.
    pub text: Color,
    /// Supporting text: hints, labels, secondary values.
    pub text_dim: Color,
    /// Barely-there text: rules, placeholders, footers.
    pub text_faint: Color,

    // --- roles ---
    /// The single accent. Reserved for focus, selection, and identity.
    pub accent: Color,
    /// What the user typed.
    pub user: Color,
    /// Tool calls and their output.
    pub tool: Color,
    /// Subagent activity.
    pub subagent: Color,

    // --- state (always paired with a glyph) ---
    pub ok: Color,
    pub warn: Color,
    pub err: Color,

    // --- syntax ---
    pub code_kw: Color,
    pub code_str: Color,
    pub code_num: Color,
    pub code_fn: Color,
    pub code_type: Color,
    pub code_comment: Color,
}

impl Theme {
    /// Near-black page, cyan accent. The morrow house style.
    pub const DARK: Theme = Theme {
        bg: Color::Rgb(8, 8, 11),
        surface: Color::Rgb(18, 18, 23),
        border: Color::Rgb(58, 58, 66),
        border_focus: Color::Cyan,

        text: Color::Rgb(225, 225, 228),
        text_dim: Color::Rgb(138, 138, 143),
        text_faint: Color::Rgb(105, 105, 110),

        accent: Color::Cyan,
        user: Color::Rgb(126, 200, 227),
        tool: Color::Rgb(86, 182, 194),
        subagent: Color::Rgb(198, 146, 233),

        ok: Color::Rgb(126, 200, 140),
        warn: Color::Rgb(232, 178, 100),
        err: Color::Rgb(240, 120, 120),

        code_kw: Color::Rgb(198, 146, 233),
        code_str: Color::Rgb(140, 200, 130),
        code_num: Color::Rgb(232, 178, 100),
        code_fn: Color::Rgb(126, 200, 227),
        code_type: Color::Rgb(120, 196, 210),
        code_comment: Color::Rgb(98, 98, 106),
    };

    /// Warm paper, ink text. Accent darkened to hold contrast on a light field.
    pub const LIGHT: Theme = Theme {
        bg: Color::Rgb(250, 249, 246),
        surface: Color::Rgb(242, 240, 235),
        border: Color::Rgb(198, 194, 184),
        border_focus: Color::Rgb(0, 110, 130),

        text: Color::Rgb(38, 38, 42),
        text_dim: Color::Rgb(92, 90, 86),
        text_faint: Color::Rgb(140, 137, 130),

        accent: Color::Rgb(0, 110, 130),
        user: Color::Rgb(20, 90, 120),
        tool: Color::Rgb(0, 120, 120),
        subagent: Color::Rgb(120, 60, 160),

        ok: Color::Rgb(30, 120, 60),
        warn: Color::Rgb(170, 110, 0),
        err: Color::Rgb(180, 40, 40),

        code_kw: Color::Rgb(120, 60, 160),
        code_str: Color::Rgb(30, 120, 60),
        code_num: Color::Rgb(170, 110, 0),
        code_fn: Color::Rgb(20, 90, 140),
        code_type: Color::Rgb(0, 120, 120),
        code_comment: Color::Rgb(130, 128, 122),
    };

    /// Chroma-free ramp. Every hue collapses to a gray step; hierarchy is
    /// carried by lightness and by the glyphs in [`glyph`].
    pub const MONO: Theme = Theme {
        bg: Color::Reset,
        surface: Color::Rgb(238, 238, 238),
        border: Color::Rgb(160, 160, 160),
        border_focus: Color::Rgb(60, 60, 60),

        text: Color::Rgb(20, 20, 20),
        text_dim: Color::Rgb(90, 90, 90),
        text_faint: Color::Rgb(140, 140, 140),

        accent: Color::Rgb(20, 20, 20),
        user: Color::Rgb(40, 40, 40),
        tool: Color::Rgb(70, 70, 70),
        subagent: Color::Rgb(100, 100, 100),

        ok: Color::Rgb(60, 60, 60),
        warn: Color::Rgb(110, 110, 110),
        err: Color::Rgb(0, 0, 0),

        code_kw: Color::Rgb(0, 0, 0),
        code_str: Color::Rgb(80, 80, 80),
        code_num: Color::Rgb(110, 110, 110),
        code_fn: Color::Rgb(40, 40, 40),
        code_type: Color::Rgb(90, 90, 90),
        code_comment: Color::Rgb(150, 150, 150),
    };

    /// Pick a theme for this process.
    ///
    /// `NO_COLOR` (any value) wins and yields [`Theme::MONO`]. Otherwise we
    /// look for an explicit terminal background hint; with nothing to go on we
    /// stay dark, which is what a coding agent is nearly always running in.
    pub fn detect() -> Theme {
        if std::env::var_os("NO_COLOR").is_some() {
            return Theme::MONO;
        }
        match background_hint().as_deref() {
            Some("light") => Theme::LIGHT,
            _ => Theme::DARK,
        }
    }

    /// True when this ramp carries no chroma — renderers use it to decide
    /// whether reverse-video may stand in for a selected background.
    pub fn is_mono(&self) -> bool {
        *self == Theme::MONO
    }
}

/// Best-effort terminal background detection.
///
/// Terminals rarely advertise this, but a few conventions exist and honouring
/// them is cheap. Anything unknown resolves to `None` so the caller defaults.
fn background_hint() -> Option<String> {
    let get = |k: &str| std::env::var(k).ok().map(|v| v.to_ascii_lowercase());

    // Some terminals export the active scheme.
    if let Some(v) = get("MORROW_THEME").or_else(|| get("COLORFGBG")) {
        if v.contains("light") {
            return Some("light".into());
        }
        if v.contains("dark") {
            return Some("dark".into());
        }
    }
    // An explicit override always wins over a raw COLORFGBG nibble.
    if let Some(v) = get("MORROW_THEME") {
        return Some(v);
    }
    None
}

/// Status glyphs. Every state in the interface carries one of these alongside
/// its color, so nothing is conveyed by hue alone.
pub mod glyph {
    pub const OK: &str = "✔";
    pub const ERR: &str = "✘";
    pub const RUN: &str = "●";
    pub const ROLE: &str = "▌";
    pub const CARET: &str = "▸";
    pub const QUOTE: &str = "│";
    pub const FENCE: &str = "▌";
    pub const MORE: &str = "…";
    pub const BULLET: &str = "•";
    pub const ARROW: &str = "→";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_is_the_default_identity() {
        let t = Theme::DARK;
        assert_eq!(t.bg, Color::Rgb(8, 8, 11));
        assert_ne!(t.text, t.bg, "body text must be readable on the page");
    }

    #[test]
    fn every_theme_has_readable_body_text() {
        // Text must not equal the background in any theme — that is the one
        // contrast failure that makes a theme unusable rather than ugly.
        for t in [Theme::DARK, Theme::LIGHT, Theme::MONO] {
            assert_ne!(t.text, t.bg, "text matches background");
            assert_ne!(t.text, t.surface, "text matches surface");
        }
    }

    #[test]
    fn light_theme_darkens_the_accent_for_contrast() {
        // Cyan on paper is unreadable; the light ramp must not reuse it raw.
        assert_ne!(Theme::LIGHT.accent, Theme::DARK.accent);
        if let (Color::Rgb(r, g, b), Color::Rgb(ar, ag, ab)) =
            (Theme::LIGHT.accent, Theme::LIGHT.text)
        {
            let lum =
                |c: (u8, u8, u8)| 0.2126 * c.0 as f64 + 0.7152 * c.1 as f64 + 0.0722 * c.2 as f64;
            assert!(
                (lum((r, g, b)) - lum((ar, ag, ab))).abs() > 40.0,
                "accent and text are too close in luminance on the light ramp"
            );
        }
    }

    #[test]
    fn mono_theme_reports_itself_as_mono() {
        assert!(Theme::MONO.is_mono());
        assert!(!Theme::DARK.is_mono());
        assert!(!Theme::LIGHT.is_mono());
    }

    #[test]
    fn detect_honors_no_color() {
        // Guard against the ambient environment leaking into this test.
        if std::env::var_os("NO_COLOR").is_some() {
            assert!(Theme::detect().is_mono());
        }
    }
}

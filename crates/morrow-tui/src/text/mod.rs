//! The block model and content-aware text rendering.
//!
//! * [`Block`] — the semantic unit the transcript is made of.
//! * [`markdown`] — turns model prose into styled, structure-aware lines.
//! * [`highlight`] — small tokenizer for fenced code.
//!
//! Everything here takes a width at render time rather than baking it in, so the
//! same block renders correctly at any terminal size and the scroll math agrees
//! with what is actually drawn.

pub mod highlight;
pub mod markdown;

pub use markdown::render_markdown;

use ratatui::style::Color;
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// A semantic unit of transcript content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A prompt the user sent.
    Prompt { text: String },
    /// Assistant prose, possibly with markdown.
    Assistant { text: String },
    /// A request to call a tool.
    ToolCall { name: String, arguments: String },
    /// The outcome of a tool call. `result` is the raw, untruncated output.
    ToolResult {
        name: String,
        ok: bool,
        result: String,
    },
    /// A subagent starting or finishing.
    Subagent { name: String, done: bool },
    /// Local UI output: slash-command replies, notices, errors.
    System { text: String, level: Level },
    /// A visual divider announcing a session boundary.
    Divider { text: String },
}

/// Severity of a [`Block::System`] message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Ordinary informational text.
    Info,
    /// Something the user should notice.
    Notice,
    /// A failure worth reading.
    Error,
}

/// Which side of the conversation a block belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User,
    Assistant,
    Tool,
    System,
}

impl Block {
    pub fn speaker(&self) -> Speaker {
        match self {
            Block::Prompt { .. } => Speaker::User,
            Block::Assistant { .. } => Speaker::Assistant,
            Block::ToolCall { .. } | Block::ToolResult { .. } => Speaker::Tool,
            Block::Subagent { .. } | Block::System { .. } | Block::Divider { .. } => {
                Speaker::System
            }
        }
    }

    /// The gutter glyph and color for this block. Every block gets one, so a
    /// reader can tell speakers apart without relying on color alone.
    pub fn gutter(&self, theme: &Theme) -> (&'static str, Color) {
        match self {
            Block::Prompt { .. } => ("❯", theme.user),
            Block::Assistant { .. } => (crate::theme::glyph::ROLE, theme.accent),
            Block::ToolCall { .. } | Block::ToolResult { .. } => {
                (crate::theme::glyph::ROLE, theme.tool)
            }
            Block::Subagent { .. } => ("◆", theme.subagent),
            Block::System { level, .. } => match level {
                Level::Error => (crate::theme::glyph::ERR, theme.err),
                Level::Notice => ("•", theme.warn),
                Level::Info => ("·", theme.text_dim),
            },
            Block::Divider { .. } => ("", theme.text_faint),
        }
    }
}

/// How a tool's output should be presented, chosen from its *shape*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultShape {
    /// One line, fits inline.
    Short,
    /// Multiple lines: render as an indented block.
    Multiline,
    /// Diff-like: leading `+`/`-`/`@@` markers get colored prefixes.
    Diff,
    /// JSON: pretty-print and highlight.
    Json,
}

/// Classify tool output so the renderer can choose a presentation.
///
/// This is the "optimize for content" decision: a stack trace, a unified diff,
/// and a one-word `grep` hit are all just a `String`, but they want very
/// different layouts, and guessing wrong makes each of them harder to read.
pub fn classify_result(text: &str) -> ResultShape {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ResultShape::Short;
    }
    let lines: Vec<&str> = trimmed.lines().filter(|l| !l.trim().is_empty()).collect();
    // A diff needs a hunk header to be convincing, not merely a stray `+` line.
    if lines.len() > 1 && lines.iter().any(|l| l.starts_with("@@")) {
        return ResultShape::Diff;
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return ResultShape::Json;
    }
    if lines.len() == 1 {
        return if display_width(trimmed) <= 200 {
            ResultShape::Short
        } else {
            // One very long line: minified JSON, or a stack trace without breaks.
            ResultShape::Multiline
        };
    }
    // Several non-empty lines that are not a diff render as a block. Blank lines
    // alone do not make output "multi-line" for display purposes.
    ResultShape::Multiline
}

/// Word-wrap `text` to `width` display columns, breaking on whitespace.
///
/// Respects display width rather than bytes, so CJK and emoji do not overflow the
/// viewport. A word longer than the line is hard-split, which keeps a
/// pathological unbroken token from destroying the layout.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in para.split(' ') {
            if word.is_empty() {
                continue;
            }
            let wlen = display_width(word);
            if line.is_empty() {
                line.push_str(word);
            } else if display_width(&line) + 1 + wlen <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
            }
            while display_width(&line) > width {
                let cut = char_boundary(&line, width);
                let head = line[..cut].to_string();
                let tail = line[cut..].to_string();
                out.push(head);
                line = tail;
            }
        }
        out.push(line);
    }
    out
}

/// Byte index of the first character boundary at or before `cols` display cells.
///
/// Returns `0` when nothing fits and `s.len()` when everything does. This is the
/// safe alternative to slicing a `&str` at a byte offset, which panics when the
/// offset lands inside a multi-byte character.
pub fn char_boundary(s: &str, cols: usize) -> usize {
    let mut used = 0usize;
    for (i, ch) in s.char_indices() {
        let w = char_width(ch);
        if used + w > cols {
            return i;
        }
        used += w;
    }
    s.len()
}

/// Truncate to at most `max_cols` display columns, appending an ellipsis.
///
/// Safe on multi-byte input: the cut lands on a character boundary.
pub fn truncate_cols(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if display_width(s) <= max_cols {
        return s.to_string();
    }
    // Reserve one cell for the ellipsis.
    let cut = char_boundary(s, max_cols.saturating_sub(1));
    let mut out = s[..cut].trim_end().to_string();
    out.push('…');
    out
}

/// Display width of a single character (0 for combining marks, 2 for wide).
fn char_width(ch: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    ch.width().unwrap_or(0)
}

/// Display width of a string in terminal cells.
pub fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(s)
}

/// Flatten spans into a single styled line. Used by tests and simple blocks.
pub fn plain_line(spans: &[Span<'static>]) -> Line<'static> {
    Line::from(spans.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_routes_by_shape() {
        assert_eq!(classify_result(""), ResultShape::Short);
        assert_eq!(classify_result("ok"), ResultShape::Short);
        assert_eq!(
            classify_result("line one\nline two\nline three"),
            ResultShape::Multiline
        );
        assert_eq!(
            classify_result("@@ -1,3 +1,4 @@\n-old\n+new"),
            ResultShape::Diff
        );
        assert_eq!(classify_result("{\"a\": 1}"), ResultShape::Json);
        assert_eq!(classify_result("[1, 2, 3]"), ResultShape::Json);
        // Braces that are not valid JSON stay plain text, not a JSON block.
        assert_eq!(classify_result("not json {"), ResultShape::Short);
        assert_eq!(
            classify_result("not json {\nmore lines"),
            ResultShape::Multiline
        );
    }

    #[test]
    fn a_long_single_line_is_not_short() {
        assert_ne!(classify_result(&"x".repeat(500)), ResultShape::Short);
    }

    #[test]
    fn wrap_respects_display_width() {
        let w = wrap("the quick brown fox jumps over the lazy dog", 12);
        assert!(w.iter().all(|l| display_width(l) <= 12), "{w:?}");
        assert!(w.len() > 1);
    }

    #[test]
    fn wrap_hard_splits_an_unbreakable_word() {
        let w = wrap(&"a".repeat(50), 10);
        assert!(w.iter().all(|l| display_width(l) <= 10), "{w:?}");
        assert_eq!(w.concat().len(), 50, "no characters lost");
    }

    #[test]
    fn wrap_preserves_blank_lines() {
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn char_boundary_never_splits_a_character() {
        let s = "héllo wörld 日本語";
        for cols in 0..=display_width(s) + 2 {
            let cut = char_boundary(s, cols);
            assert!(s.is_char_boundary(cut), "cut {cut} splits a char");
        }
    }

    #[test]
    fn truncate_cols_is_multibyte_safe_and_bounded() {
        let s = "héllo wörld ünïcode";
        for max in 0..16 {
            let t = truncate_cols(s, max);
            let w = display_width(&t);
            assert!(w <= max, "max={max} produced width {w}: {t:?}");
        }
        assert_eq!(truncate_cols("abc", 10), "abc");
    }

    #[test]
    fn truncate_wide_chars_by_display_width() {
        // Two-cell CJK: three chars occupy six cells, so a five-cell budget
        // fits two plus the ellipsis.
        let t = truncate_cols("日本語表示", 5);
        assert!(display_width(&t) <= 5, "got {t:?}");
        assert!(t.ends_with('…'));
    }

    #[test]
    fn every_block_has_a_gutter_glyph_except_dividers() {
        let blocks = [
            Block::Prompt { text: "hi".into() },
            Block::Assistant { text: "yo".into() },
            Block::ToolCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
            Block::Subagent {
                name: "explore".into(),
                done: false,
            },
            Block::System {
                text: "boom".into(),
                level: Level::Error,
            },
        ];
        for b in blocks {
            let (g, _) = b.gutter(&Theme::DARK);
            assert!(!g.is_empty(), "{b:?} has no gutter glyph");
        }
        let (g, _) = Block::Divider { text: "x".into() }.gutter(&Theme::DARK);
        assert!(g.is_empty(), "a divider is a rule, not a speaker");
    }
}

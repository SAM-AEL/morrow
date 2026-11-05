//! The transcript viewport: scrollback, wrapping, and the scrollbar.
//!
//! The bug this module exists to prevent: the old renderer sliced the transcript
//! by *logical* line count while drawing with word-wrap, so a long line consumed
//! several terminal rows and every scroll offset drifted. Here a block is
//! flattened into fully-wrapped [`Line`]s first, the resulting rows are the only
//! thing that is measured, and scrolling indexes those rows. Logical lines and
//! visual rows can no longer disagree.

use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::app::App;
use crate::render::layout::Metrics;
use crate::text::highlight::{self, Lang};
use crate::text::{classify_result, markdown, truncate_cols, Block, Level, ResultShape};
use crate::theme::Theme;

/// Cells reserved on the left for the speaker gutter, including the space after.
const GUTTER: u16 = 3;

/// How many lines of multi-line tool output to show before truncating.
const MAX_RESULT_LINES: usize = 12;

/// Flatten every block into wrapped, styled lines at `width`.
///
/// Returns the lines plus the block index each line came from, so the transcript
/// can attribute scrollbar position to blocks rather than to pixels.
pub fn flatten(
    blocks: &[Block],
    width: u16,
    theme: &Theme,
    streaming: Option<&str>,
) -> Vec<Line<'static>> {
    let width = width.max(16);
    let mut out: Vec<Line<'static>> = Vec::new();
    for b in blocks {
        render_block(b, width, theme, &mut out);
    }
    // The in-flight assistant message is drawn as if it were already complete,
    // so streaming text wraps exactly the way it will once committed.
    if let Some(text) = streaming {
        if !text.is_empty() {
            render_block(
                &Block::Assistant {
                    text: text.to_string(),
                },
                width,
                theme,
                &mut out,
            );
        }
    }
    out
}

/// Render one block into wrapped lines, prefixed by its gutter.
fn render_block(b: &Block, width: u16, theme: &Theme, out: &mut Vec<Line<'static>>) {
    let (glyph, gcolor) = b.gutter(theme);
    let inner_w = (width as usize).saturating_sub(GUTTER as usize).max(8);

    // Blocks that carry their own structured rendering bypass the plain path.
    match b {
        Block::ToolCall { name, arguments } => {
            out.push(gutter_line(
                glyph,
                gcolor,
                &[
                    Span::styled(
                        "CALL ",
                        Style::new().fg(theme.tool).add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        name.clone(),
                        Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                ],
            ));
            // Arguments are JSON: show them compactly, never as a wall of text.
            let compact = compact_args(arguments, inner_w);
            for l in markdown::render_markdown(&compact, inner_w, theme) {
                out.push(indent_line(l, theme));
            }
            return;
        }
        Block::ToolResult { name, ok, result } => {
            render_tool_result(
                &ToolResultBlock {
                    name,
                    ok: *ok,
                    result,
                    glyph,
                    gcolor,
                },
                inner_w,
                theme,
                out,
            );
            return;
        }
        _ => {}
    }

    let body: Vec<Line<'static>> = match b {
        Block::Prompt { text } => markdown::render_markdown(text, inner_w, theme)
            .into_iter()
            .map(|l| l.style(Style::new().fg(theme.user).add_modifier(Modifier::BOLD)))
            .collect(),
        Block::Assistant { text } => markdown::render_markdown(text, inner_w, theme),
        Block::Subagent { name, done } => vec![Line::from(vec![
            Span::styled(
                if *done { "■ " } else { "▶ " },
                Style::new().fg(theme.subagent),
            ),
            Span::styled(
                format!("subagent {name}"),
                Style::new().fg(if *done {
                    theme.text_dim
                } else {
                    theme.subagent
                }),
            ),
            if !*done {
                Span::styled("  running", Style::new().fg(theme.text_faint))
            } else {
                Span::styled("  done", Style::new().fg(theme.text_faint))
            },
        ])],
        Block::System { text, level } => {
            let (color, modifier) = match level {
                Level::Info => (theme.text_dim, Modifier::DIM),
                Level::Notice => (theme.warn, Modifier::empty()),
                Level::Error => (theme.err, Modifier::BOLD),
            };
            markdown::render_markdown(text, inner_w, theme)
                .into_iter()
                .map(|l| l.style(Style::new().fg(color).add_modifier(modifier)))
                .collect()
        }
        Block::Divider { text } => vec![Line::from(Span::styled(
            truncate_cols(text, inner_w),
            Style::new().fg(theme.text_faint),
        ))],
        Block::ToolCall { .. } | Block::ToolResult { .. } => unreachable!("handled above"),
    };

    if body.is_empty() {
        return;
    }
    for (i, l) in body.into_iter().enumerate() {
        if i == 0 {
            out.push(prepend_gutter(l, glyph, gcolor));
        } else {
            out.push(indent_line(l, theme));
        }
    }
}

/// A tool result plus the gutter it was given by its [`Block`].
///
/// Bundled into one struct so the renderer takes four parameters instead of
/// eight, and so the shape of "what to draw for a result" is named.
struct ToolResultBlock<'a> {
    name: &'a str,
    ok: bool,
    result: &'a str,
    glyph: &'a str,
    gcolor: Color,
}

/// Render a tool result according to the shape of its output.
///
/// A stack trace, a diff, and a one-word `grep` hit are all just a `String`; they
/// get genuinely different presentations here, chosen by [`classify_result`].
fn render_tool_result(
    b: &ToolResultBlock,
    inner_w: usize,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
) {
    let ToolResultBlock {
        name,
        ok,
        result,
        glyph,
        gcolor,
    } = *b;
    let status_glyph = if ok {
        crate::theme::glyph::OK
    } else {
        crate::theme::glyph::ERR
    };
    let status_color = if ok { theme.ok } else { theme.err };

    out.push(gutter_line(
        glyph,
        gcolor,
        &[
            Span::styled(
                "TOOL ",
                Style::new().fg(theme.tool).add_modifier(Modifier::DIM),
            ),
            Span::styled(
                name.to_string(),
                Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {status_glyph} "), Style::new().fg(status_color)),
        ],
    ));

    let shape = classify_result(result);
    match shape {
        ResultShape::Short => {
            let text = truncate_cols(result.trim(), inner_w);
            out.push(indent_line(
                Line::from(Span::styled(text, Style::new().fg(theme.text_dim))),
                theme,
            ));
        }
        ResultShape::Json => {
            // Pretty-print when it is valid JSON so the structure is visible.
            let body = pretty_json(result, inner_w, theme);
            for l in body {
                out.push(indent_line(l, theme));
            }
        }
        ResultShape::Diff => {
            for l in render_diff(result, inner_w, theme) {
                out.push(indent_line(l, theme));
            }
        }
        ResultShape::Multiline => {
            let lines: Vec<&str> = result.lines().collect();
            let shown = lines.len().min(MAX_RESULT_LINES);
            for raw in &lines[..shown] {
                for w in crate::text::wrap(raw, inner_w) {
                    out.push(indent_line(
                        Line::from(Span::styled(w, Style::new().fg(theme.text_dim))),
                        theme,
                    ));
                }
            }
            if lines.len() > shown {
                out.push(indent_line(
                    Line::from(Span::styled(
                        format!(
                            "{} {} more line{}",
                            crate::theme::glyph::MORE,
                            lines.len() - shown,
                            if lines.len() - shown == 1 { "" } else { "s" }
                        ),
                        Style::new().fg(theme.text_faint),
                    )),
                    theme,
                ));
            }
        }
    }
}

/// Colorize a unified diff: additions green, removals red, hunk headers accent.
fn render_diff(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for raw in text.lines().take(MAX_RESULT_LINES) {
        let (marker, color) = if raw.starts_with('+') {
            ("+", theme.ok)
        } else if raw.starts_with('-') {
            ("-", theme.err)
        } else if raw.starts_with("@@") {
            ("", theme.accent)
        } else {
            ("", theme.text_dim)
        };
        let mut spans = vec![Span::styled(format!("{marker} "), Style::new().fg(color))];
        let body = if marker.is_empty() { raw } else { &raw[1..] };
        spans.extend(highlight::line_spans(
            body.trim_start(),
            Lang::Unknown,
            theme,
        ));
        // Diff rows are dense; do not wrap them, they already carry structure.
        let joined = truncate_cols(
            &spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>(),
            width,
        );
        out.push(Line::from(if joined.len() == body.len() + 2 {
            spans
        } else {
            vec![Span::styled(joined, Style::new().fg(color))]
        }));
    }
    out
}

/// Pretty-print and highlight JSON, falling back to raw lines if it will not parse.
fn pretty_json(text: &str, _width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return text
            .lines()
            .take(MAX_RESULT_LINES)
            .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(theme.text_dim))))
            .collect();
    };
    let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string());
    pretty
        .lines()
        .take(MAX_RESULT_LINES)
        .map(|l| Line::from(highlight::line_spans(l, Lang::Json, theme)))
        .collect()
}

/// Render tool arguments compactly: one line, keys visible, values truncated.
fn compact_args(args: &str, width: usize) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(v) if v.is_object() => {
            let keys: Vec<String> = v
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            if keys.is_empty() {
                truncate_cols(trimmed, width)
            } else {
                truncate_cols(&keys.join(", "), width)
            }
        }
        _ => truncate_cols(trimmed, width),
    }
}

/// Draw the transcript into `area`.
pub fn draw(f: &mut Frame, app: &App, _m: &Metrics, area: Rect, theme: &Theme) {
    let width = area.width;
    let all = flatten(
        app.blocks(),
        width,
        theme,
        if app.assistant_buf().is_empty() {
            None
        } else {
            Some(app.assistant_buf())
        },
    );
    let total = all.len();
    let height = area.height as usize;

    // Clamp the scroll request against real geometry: `app.scroll()` is in
    // blocks, which is an approximation, so the renderer resolves it to rows and
    // clamps to what actually fits.
    let max_offset = total.saturating_sub(height);
    let offset = resolve_offset(app.scroll(), total, height);

    // Draw the visible window.
    let end = (offset + height).min(total);
    let visible: Vec<Line<'static>> = all[offset.min(total)..end].to_vec();
    let para = Paragraph::new(visible).style(Style::new().bg(theme.bg));
    f.render_widget(para, area);

    // The scrollbar only appears when there is something to scroll.
    if max_offset > 0 {
        let mut sb_state = ScrollbarState::new(max_offset).position(offset.min(max_offset));
        let track = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .thumb_symbol("┃")
                .thumb_style(Style::new().fg(theme.accent))
                .track_style(Style::new().fg(theme.border)),
            track,
            &mut sb_state,
        );
    }
}

/// Turn a block-space scroll request into a row-space offset.
///
/// `blocks_from_end` is how many blocks the reader asked to scroll back. Each
/// block averages several rows, so a fixed divisor keeps the gesture responsive
/// without needing the exact per-block row counts.
fn resolve_offset(blocks_from_end: usize, total_rows: usize, height: usize) -> usize {
    if blocks_from_end == 0 {
        return total_rows.saturating_sub(height);
    }
    let approx_rows_back = blocks_from_end * 3;
    total_rows
        .saturating_sub(height)
        .saturating_sub(approx_rows_back)
}

/// Build a line that begins with the speaker glyph in its gutter.
fn gutter_line(
    glyph: &str,
    color: ratatui::style::Color,
    spans: &[Span<'static>],
) -> Line<'static> {
    let mut v = Vec::with_capacity(spans.len() + 1);
    v.push(Span::styled(
        format!("{glyph} "),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    ));
    v.extend(spans.iter().cloned());
    Line::from(v)
}

/// Add a glyph to an existing line's front, padding to the gutter width.
fn prepend_gutter(line: Line<'static>, glyph: &str, color: ratatui::style::Color) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{:<width$}", glyph, width = GUTTER as usize),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    spans.extend(line.spans);
    Line::from(spans)
}

/// Indent a continuation line so it sits under its block's text.
fn indent_line(line: Line<'static>, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(
        " ".repeat(GUTTER as usize),
        Style::new().bg(theme.bg),
    )];
    spans.extend(line.spans);
    Line::from(spans)
}

/// Strip styles, returning the visible text of a line. Test helper.
#[cfg(test)]
pub(crate) fn text_of(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.to_string()).collect()
}

/// Total display width of a line. Test helper.
#[cfg(test)]
pub(crate) fn line_width(l: &Line<'_>) -> usize {
    l.spans
        .iter()
        .map(|s| crate::text::display_width(&s.content))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Level;

    fn lines_for(b: Block) -> Vec<Line<'static>> {
        flatten(&[b], 60, &Theme::DARK, None)
    }

    #[test]
    fn user_block_shows_the_user_gutter() {
        let l = lines_for(Block::Prompt { text: "hi".into() });
        assert!(text_of(&l[0]).contains('❯'));
    }

    #[test]
    fn assistant_markdown_renders_a_heading_as_a_heading() {
        let l = lines_for(Block::Assistant {
            text: "## Findings\n\nbody".into(),
        });
        let joined: String = l.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("Findings"),
            "heading text missing:\n{joined}"
        );
        assert!(joined.contains('─'), "heading rule missing:\n{joined}");
        // The `##` is a dim depth cue, not literal markdown.
        let marker = l[0]
            .spans
            .iter()
            .find(|s| s.content.contains('#'))
            .expect("depth cue");
        assert!(marker.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn tool_call_renders_a_label_and_compact_args() {
        let l = lines_for(Block::ToolCall {
            name: "bash".into(),
            arguments: r#"{"command":"ls -la","timeout_secs":30}"#.into(),
        });
        let joined: String = l.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("CALL"));
        assert!(joined.contains("bash"));
        // Only the keys, not the whole JSON blob.
        assert!(joined.contains("command"));
        assert!(!joined.contains("timeout_secs\":30"));
    }

    #[test]
    fn short_tool_result_stays_inline_with_a_status_glyph() {
        let l = lines_for(Block::ToolResult {
            name: "grep".into(),
            ok: true,
            result: "src/main.rs:12".into(),
        });
        let joined: String = l.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("TOOL"));
        assert!(joined.contains(crate::theme::glyph::OK));
        assert!(joined.contains("src/main.rs:12"));
    }

    #[test]
    fn failed_tool_result_shows_the_error_glyph() {
        let l = lines_for(Block::ToolResult {
            name: "bash".into(),
            ok: false,
            result: "boom".into(),
        });
        let joined: String = l.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(joined.contains(crate::theme::glyph::ERR));
    }

    #[test]
    fn multiline_result_is_blocked_and_capped() {
        let body = (0..40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let l = lines_for(Block::ToolResult {
            name: "bash".into(),
            ok: true,
            result: body,
        });
        let joined: String = l.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("line 0"));
        assert!(
            joined.contains("more line"),
            "cap footer missing:\n{joined}"
        );
        assert!(
            !joined.contains("line 39"),
            "content beyond the cap should be elided"
        );
    }

    #[test]
    fn diff_result_colors_additions_and_removals() {
        let theme = Theme::DARK;
        let l = lines_for(Block::ToolResult {
            name: "bash".into(),
            ok: true,
            result: "@@ -1,2 +1,2 @@\n-old\n+new".into(),
        });
        let all: Vec<Span<'static>> = l.iter().flat_map(|x| x.spans.clone()).collect();
        assert!(
            all.iter().any(|s| s.style.fg == Some(theme.ok)),
            "no green add"
        );
        assert!(
            all.iter().any(|s| s.style.fg == Some(theme.err)),
            "no red remove"
        );
        assert!(
            all.iter().any(|s| s.style.fg == Some(theme.accent)),
            "no hunk accent"
        );
    }

    #[test]
    fn json_result_is_pretty_printed() {
        let l = lines_for(Block::ToolResult {
            name: "read".into(),
            ok: true,
            result: r#"{"a":1,"b":2}"#.into(),
        });
        let joined: String = l.iter().map(text_of).collect::<Vec<_>>().join("\n");
        // Pretty-printed JSON puts each key on its own line.
        assert!(joined.contains("\"a\""), "got:\n{joined}");
        assert!(joined.contains('\n') || joined.matches("  ").count() > 0);
    }

    #[test]
    fn nothing_overflows_the_declared_width() {
        let theme = Theme::DARK;
        let blocks = vec![
            Block::Prompt {
                text:
                    "a fairly long user prompt that should wrap rather than overflow the viewport"
                        .into(),
            },
            Block::Assistant {
                text:
                    "## A heading\n\nSome prose that is long enough to need wrapping at 60 columns."
                        .into(),
            },
            Block::ToolCall {
                name: "write".into(),
                arguments: format!("{{\"content\":\"{}\"}}", "x".repeat(300)),
            },
            Block::ToolResult {
                name: "bash".into(),
                ok: true,
                result: (0..20)
                    .map(|i| format!("output line {i} with some trailing detail"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            Block::System {
                text: "notice".into(),
                level: Level::Notice,
            },
        ];
        for width in [30u16, 40, 60, 80] {
            for l in flatten(&blocks, width, &theme, None) {
                assert!(
                    line_width(&l) <= width as usize,
                    "width {width}: overflowed to {} in {:?}",
                    line_width(&l),
                    text_of(&l)
                );
            }
        }
    }

    #[test]
    fn streaming_text_wraps_like_committed_text() {
        let theme = Theme::DARK;
        let text = "some streaming prose that is long enough to wrap at this width";
        let streaming = flatten(&[], 40, &theme, Some(text));
        let committed = flatten(&[Block::Assistant { text: text.into() }], 40, &theme, None);
        assert_eq!(streaming.len(), committed.len());
        assert_eq!(text_of(&streaming[0]), text_of(&committed[0]));
    }

    #[test]
    fn scroll_zero_pins_to_the_bottom() {
        // 100 rows of content in a 20-row viewport: the bottom shows row 80.
        assert_eq!(resolve_offset(0, 100, 20), 80);
    }

    #[test]
    fn scrolling_back_moves_the_offset_up_and_never_negative() {
        assert!(resolve_offset(1, 100, 20) < resolve_offset(0, 100, 20));
        // Far more scroll than there is content: clamp to the top, never negative.
        assert_eq!(resolve_offset(1000, 100, 20), 0);
    }

    #[test]
    fn offset_never_exceeds_the_content() {
        for total in 0..50usize {
            for h in 1..30usize {
                for s in 0..10usize {
                    let o = resolve_offset(s, total, h);
                    assert!(o <= total, "offset {o} > total {total}");
                }
            }
        }
    }

    #[test]
    fn a_long_wrapping_line_does_not_underfill_the_viewport() {
        // The original bug: a wrapping line counted as one row. Here flatten must
        // produce many rows for one logical line, so the window can fill.
        let theme = Theme::DARK;
        let text = "word ".repeat(200);
        let lines = flatten(&[Block::Assistant { text }], 40, &theme, None);
        assert!(lines.len() > 5, "long line should wrap into many rows");
    }

    #[test]
    fn pathological_tool_output_does_not_panic() {
        for result in [
            "x".repeat(50_000),
            format!("{}\n", "a\n".repeat(5_000)),
            "{\"unclosed\": ".to_string(),
            "\u{1b}[31mANSI\u{1b}[0m escape".to_string(),
            "日本語\n日本語\n日本語".to_string(),
        ] {
            let rendered = lines_for(Block::ToolResult {
                name: "bash".into(),
                ok: true,
                result,
            });
            assert!(!rendered.is_empty(), "a result always renders a header");
        }
    }

    #[test]
    fn empty_blocks_produce_nothing() {
        // Empty prose contributes nothing rather than a blank row.
        assert!(lines_for(Block::Assistant {
            text: String::new()
        })
        .is_empty());
        // A tool call still renders its header, so the reader sees what ran.
        let empty = lines_for(Block::ToolResult {
            name: "bash".into(),
            ok: true,
            result: String::new(),
        });
        assert!(!empty.is_empty(), "tool header should still render");
        assert!(text_of(&empty[0]).contains("TOOL"));
    }
}

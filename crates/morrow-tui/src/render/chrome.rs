//! Persistent chrome: the status bar, the hint row, the input box, and the
//! welcome hero.
//!
//! Every state here carries a glyph or a label as well as a color, so nothing in
//! the interface depends on hue alone to be readable.

use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::render::layout::{Height, Metrics, Width};
use crate::text::display_width;
use crate::theme::Theme;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Shorten a path from the left so the tail stays visible.
///
/// The ellipsis occupies one cell, so the tail gets `max - 1` cells. Cutting on a
/// character boundary keeps multi-byte paths intact.
fn elide_path(path: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(path) <= max {
        return path.to_string();
    }
    // Reserve the ellipsis cell, then walk forward until the remaining tail would
    // exceed the budget. Landing on `tail_start` guarantees a char boundary.
    let budget = max - 1;
    let mut tail_start = path.len();
    let mut w = 0usize;
    for (i, ch) in path.char_indices().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > budget {
            tail_start = i + ch.len_utf8();
            break;
        }
        w += cw;
        tail_start = i;
    }
    let mut out = String::from("…");
    out.push_str(&path[tail_start..]);
    out
}

/// Shorten a span list to `max` display columns, preserving each span's style.
///
/// Spans are dropped or elided from the right, so the leftmost content (the
/// brand, the state) survives and only the tail is lost.
fn fit_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    if total <= max {
        return spans;
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for s in spans {
        let w = display_width(&s.content);
        if used + w <= max {
            used += w;
            out.push(s);
        } else {
            // The first span that does not fit gets elided into what remains.
            let room = max.saturating_sub(used);
            if room > 1 {
                out.push(Span::styled(
                    crate::text::truncate_cols(&s.content, room),
                    s.style,
                ));
            }
            break;
        }
    }
    out
}

/// The status line: identity and state on the left, working directory on the right.
///
/// The two sides are budgeted explicitly rather than measured after the fact:
/// the path yields space first, and the status text is elided into whatever is
/// left, so neither side can push the other off the row.
pub fn draw_status(f: &mut Frame, app: &App, m: &Metrics, area: Rect, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let avail = area.width as usize;

    let cwd = if m.shows_path() {
        std::env::current_dir()
            .map(|p| {
                let home = std::env::var("HOME").unwrap_or_default();
                let s = p.display().to_string();
                if !home.is_empty() && s.starts_with(&home) {
                    format!("~{}", &s[home.len()..])
                } else {
                    s
                }
            })
            .ok()
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    // The path gets at most half the row, and never so much that the status has
    // no room at all.
    let path_w = match cwd.as_deref() {
        Some(c) => display_width(c)
            .min(avail.saturating_sub(16))
            .min(avail / 2),
        None => 0,
    };
    let path = cwd.as_deref().map(|c| elide_path(c, path_w));
    let path_cells = path.as_deref().map(display_width).unwrap_or(0);

    let left_max = avail.saturating_sub(path_cells + if path_cells > 0 { 2 } else { 0 });

    let mut spans = vec![Span::styled("✦ ", Style::new().fg(theme.accent).bold())];
    if m.width == Width::Compact {
        // No room for a model line: identity and state only.
        spans.push(Span::styled(
            app.status_right(),
            Style::new().fg(theme.text_dim),
        ));
    } else {
        spans.push(Span::styled("morrow", Style::new().fg(theme.text).bold()));
        spans.push(Span::styled(
            format!(" · {}", app.status_right()),
            Style::new().fg(theme.text_dim),
        ));
    }

    let left: Vec<Span<'static>> = fit_spans(spans, left_max);
    f.render_widget(
        Paragraph::new(Line::from(left)).style(Style::new().bg(theme.bg)),
        area,
    );

    if let Some(p) = path {
        let r = Rect {
            x: area.x + (area.width.saturating_sub(path_cells as u16)),
            y: area.y,
            width: path_cells as u16,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::styled(p, Style::new().fg(theme.text_faint)))
                .alignment(Alignment::Right),
            r,
        );
    }
}

/// The hint row: a spinner and a thinking verb while working, guidance otherwise.
pub fn draw_hint(f: &mut Frame, app: &App, m: &Metrics, area: Rect, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = if app.is_busy() {
        let tick = app.spinner_tick();
        let frame = SPINNER[(tick as usize) % SPINNER.len()];
        Line::from(vec![
            Span::styled(format!("{frame} "), Style::new().fg(theme.accent)),
            Span::styled(
                format!("{}…", crate::loading::thinking_verb(tick)),
                Style::new().fg(theme.text_dim),
            ),
        ])
    } else if !app.input().trim_start().starts_with('/') || app.completions().is_empty() {
        if app.transcript_len() > 0 && m.width != Width::Compact {
            Line::from(Span::styled(
                "/help · /switch · /mode · /clear · ctrl+q quit",
                Style::new().fg(theme.text_faint),
            ))
        } else if app.transcript_len() > 0 {
            Line::from(Span::styled(
                "/help · ctrl+q quit",
                Style::new().fg(theme.text_faint),
            ))
        } else {
            Line::from("")
        }
    } else {
        Line::from(Span::styled(
            "Tab complete · ↑↓ navigate · Enter send",
            Style::new().fg(theme.text_dim),
        ))
    };
    let padded = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(line).style(Style::new().bg(theme.bg)),
        padded,
    );
}

/// The input box, with a real cursor positioned at the end of the text.
pub fn draw_input(f: &mut Frame, app: &App, m: &Metrics, area: Rect, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // A tight terminal gets a single borderless row rather than a box we cannot
    // fit. Degrading structure is better than refusing to render.
    if m.height == Height::Tight {
        let mut spans = vec![Span::styled("› ", Style::new().fg(theme.accent).bold())];
        if app.input().is_empty() {
            spans.push(Span::styled(
                "ask anything…",
                Style::new().fg(theme.text_faint).italic(),
            ));
        } else {
            spans.push(Span::styled(
                app.input().to_string(),
                Style::new().fg(theme.text),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        // Cursor goes right after the prompt marker.
        let x = area.x + 2 + display_width(app.input()) as u16;
        if x < area.x + area.width {
            f.set_cursor_position((x, area.y));
        }
        return;
    }

    let focused = app.pending_approval().is_none();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if app.is_busy() {
            theme.accent
        } else if focused {
            theme.border_focus
        } else {
            theme.border
        }))
        .title(Span::styled(
            " prompt ",
            Style::new()
                .fg(if app.is_busy() {
                    theme.accent
                } else {
                    theme.text_dim
                })
                .bold(),
        ))
        .title_bottom(
            Line::from(Span::styled(
                " ↵ send · ctrl+c exit · ctrl+q quit ",
                Style::new().fg(theme.text_faint),
            ))
            .alignment(Alignment::Right),
        )
        .style(Style::new().bg(theme.bg));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let has_text = !app.input().is_empty();
    let mut spans = vec![Span::styled("› ", Style::new().fg(theme.accent).bold())];
    if has_text {
        spans.push(Span::styled(
            app.input().to_string(),
            Style::new().fg(theme.text),
        ));
    } else {
        spans.push(Span::styled(
            "ask anything…",
            Style::new().fg(theme.text_faint).italic(),
        ));
    }
    let rect = Rect {
        x: inner.x + 1,
        y: inner.y + (inner.height.saturating_sub(1)) / 2,
        width: inner.width.saturating_sub(2),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.bg)),
        rect,
    );

    // Place the cursor at the true end of the input, measured in display cells so
    // wide characters and emoji do not drift it.
    if focused {
        let inner_w = inner.width.saturating_sub(3) as usize;
        let shown: String = if has_text {
            app.input().chars().take(inner_w).collect()
        } else {
            String::new()
        };
        let x = inner.x + 1 + 2 + display_width(&shown) as u16;
        let y = rect.y;
        if x < area.x + area.width {
            f.set_cursor_position((x, y));
        }
    }
}

/// The welcome hero shown on a fresh thread.
pub fn draw_welcome(f: &mut Frame, app: &App, m: &Metrics, area: Rect, theme: &Theme) {
    let tick = app.spinner_tick();
    let version: &str = env!("CARGO_PKG_VERSION");

    let mut title = crate::loading::rainbow_title("morrow", tick);
    title.alignment = Some(Alignment::Center);

    let centered = |s: Line<'static>| {
        let mut l = s;
        l.alignment = Some(Alignment::Center);
        l
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    // Short terminals drop the tagline rather than overflow.
    if area.height >= 14 {
        lines.push(Line::from(""));
        lines.push(title);
        lines.push(centered(Line::styled(
            format!("v{version}"),
            Style::new().fg(theme.text_faint),
        )));
        lines.push(Line::from(""));
        lines.push(centered(Line::styled(
            "Character-based agents that learn with every turn",
            Style::new().fg(theme.text_dim),
        )));
        lines.push(Line::from(""));
    } else {
        lines.push(title);
        lines.push(centered(Line::styled(
            format!("v{version}"),
            Style::new().fg(theme.text_faint),
        )));
    }

    let rule_w = 44.min(area.width.saturating_sub(10) as usize).max(8);
    lines.push(centered(Line::styled(
        "─".repeat(rule_w),
        Style::new().fg(theme.border),
    )));
    lines.push(Line::from(""));

    // Command chips: enough to orient, not a wall of text.
    let chips: &[&str] = if m.width == Width::Compact {
        &["/help", "/model"]
    } else {
        &["/help", "/model", "/switch", "/mode", "/clear"]
    };
    let mut chip_spans = Vec::new();
    for (i, name) in chips.iter().enumerate() {
        if i > 0 {
            chip_spans.push(Span::styled("  ·  ", Style::new().fg(theme.text_faint)));
        }
        chip_spans.push(Span::styled(
            name.to_string(),
            Style::new().fg(theme.accent),
        ));
    }
    lines.push(centered(Line::from(chip_spans)));
    lines.push(centered(Line::styled(
        "Type to chat  ·  / for commands",
        Style::new().fg(theme.text_faint),
    )));

    if let Some(banner) = app.session_banner() {
        if (lines.len() + 2) <= area.height as usize {
            lines.push(Line::from(""));
            lines.push(centered(Line::styled(
                banner.to_string(),
                Style::new().fg(theme.text_dim),
            )));
        }
    }

    if lines.len() > area.height as usize {
        lines.truncate(area.height as usize);
    }

    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect {
        x: area.x + 1,
        y,
        width: area.width.saturating_sub(2),
        height: h,
    };
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(theme.bg)), rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_spans_keeps_styles_and_respects_the_budget() {
        let spans = vec![
            Span::styled("aaa", Style::new().fg(Color::Red)),
            Span::styled("bbb", Style::new().fg(Color::Blue)),
            Span::styled("ccc", Style::new().fg(Color::Green)),
        ];
        let out = fit_spans(spans, 7);
        let w: usize = out.iter().map(|s| display_width(&s.content)).sum();
        assert!(w <= 7, "width {w} exceeded budget");
        // The leading span keeps its own style rather than being flattened.
        assert_eq!(out[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn fit_spans_is_a_no_op_when_everything_fits() {
        let spans = vec![Span::raw("abc")];
        let out = fit_spans(spans.clone(), 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "abc");
    }

    #[test]
    fn fit_spans_handles_a_zero_budget() {
        let spans = vec![Span::raw("abcdef")];
        let out = fit_spans(spans, 0);
        assert_eq!(
            out.iter().map(|s| display_width(&s.content)).sum::<usize>(),
            0
        );
    }

    #[test]
    fn elide_path_keeps_the_tail_and_marks_the_cut() {
        let p = "/home/user/projects/deeply/nested";
        let e = elide_path(p, 20);
        assert!(display_width(&e) <= 20, "got {e:?}");
        assert!(e.starts_with('…'));
        assert!(e.ends_with("nested"), "tail should survive: {e:?}");
    }

    #[test]
    fn elide_path_is_a_no_op_when_it_fits() {
        assert_eq!(elide_path("/tmp", 20), "/tmp");
        assert_eq!(elide_path("/tmp", 0), "");
    }

    #[test]
    fn elide_path_is_multibyte_safe() {
        let p = "/home/user/日本語/プロジェクト";
        for max in 0..20 {
            let e = elide_path(p, max);
            assert!(display_width(&e) <= max.max(1), "max={max} got {e:?}");
        }
    }
}

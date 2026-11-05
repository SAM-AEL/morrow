//! Overlays: the autocomplete dropdown and the approval modal.
//!
//! Both are attention-plane surfaces drawn above the transcript with a `Clear`
//! behind them, so they read as floating rather than as part of the page.

use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::app::App;
use crate::render::layout::Metrics;
use crate::text::truncate_cols;
use crate::theme::Theme;

/// Rows of the dropdown to show at once.
const MAX_ROWS: usize = 8;

/// The autocomplete dropdown, anchored above the input.
///
/// Returns nothing when there is nothing to complete — an empty popup is worse
/// than no popup.
pub fn draw_completions(
    f: &mut Frame,
    app: &App,
    m: &Metrics,
    area: Rect,
    input: Rect,
    theme: &Theme,
) {
    if app.is_busy() || !app.input().trim_start().starts_with('/') {
        return;
    }
    let items = app.completions();
    if items.is_empty() {
        return;
    }
    let sel = app.completion_sel().min(items.len() - 1);
    let shown = items.len().min(MAX_ROWS);
    let start = if items.len() <= MAX_ROWS {
        0
    } else {
        sel.saturating_sub(MAX_ROWS / 2).min(items.len() - MAX_ROWS)
    };

    let width = 60.min(area.width.saturating_sub(4)).max(24);
    // The popup grows upward from the input; clamp to the space available.
    let avail_above = input.y.saturating_sub(area.y);
    let height = (shown as u16 + 2).min(avail_above);
    if height < 3 {
        return;
    }
    let popup = Rect {
        x: input.x.min(area.x + area.width.saturating_sub(width)),
        y: input.y.saturating_sub(height),
        width,
        height,
    };
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(" tab ↵ ", Style::new().fg(theme.text_faint)))
        .style(Style::new().bg(theme.surface));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Command names complete bare; value lists (models, characters, threads)
    // already carry their display form.
    let is_value = app.input().trim_start().contains(char::is_whitespace);
    let hint_w = inner.width.saturating_sub(6) as usize;

    let rows: Vec<Line<'static>> = items
        .iter()
        .skip(start)
        .take(shown)
        .enumerate()
        .map(|(i, c)| {
            let idx = start + i;
            let label = if is_value {
                c.display.clone()
            } else {
                format!("/{}", c.display)
            };
            let caret = if idx == sel {
                crate::theme::glyph::CARET
            } else {
                " "
            };
            let label_style = if idx == sel {
                Style::new().fg(theme.bg).bg(theme.accent).bold()
            } else {
                Style::new().fg(theme.text)
            };
            let hint_style = if idx == sel {
                Style::new().fg(theme.bg).bg(theme.accent)
            } else {
                Style::new().fg(theme.text_faint)
            };
            Line::from(vec![
                Span::styled(format!("{caret} "), Style::new().fg(theme.accent)),
                Span::styled(truncate_cols(&label, hint_w), label_style),
                Span::styled(format!("  {}", c.hint), hint_style),
            ])
        })
        .collect();
    let _ = m;
    f.render_widget(Paragraph::new(rows), inner);
}

/// The approval modal.
///
/// This is the one place where the interface stops the user's work, so it states
/// the tool, shows the arguments, and names both choices in words rather than
/// leaving bare `y`/`n` keys to be guessed.
pub fn draw_approval(f: &mut Frame, tool: &str, arguments: &str, area: Rect, theme: &Theme) {
    if area.width < 24 || area.height < 7 {
        // Too small for a modal: state the requirement plainly.
        f.render_widget(
            Paragraph::new(Line::styled(
                "terminal too small to show approval",
                Style::new().fg(theme.warn),
            ))
            .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let w = (area.width as f32 * 0.8) as u16;
    let h = 11.min(area.height);
    let modal = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, modal);

    let inner_w = modal.width.saturating_sub(4) as usize;
    let mut text: Vec<Line<'static>> = vec![
        Line::styled("  permission required", Style::new().bold().fg(theme.warn)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  tool   ", Style::new().fg(theme.text_faint)),
            Span::styled(
                truncate_cols(tool, inner_w.saturating_sub(9)),
                Style::new().fg(theme.text).bold(),
            ),
        ]),
    ];

    // Arguments are the part that decides the answer, so they get two lines and
    // are wrapped rather than clipped mid-token.
    let args: String = arguments.replace('\n', " ↵ ");
    for (i, w2) in crate::text::wrap(&args, inner_w.saturating_sub(9))
        .iter()
        .take(2)
        .enumerate()
    {
        text.push(Line::from(vec![
            Span::styled(
                if i == 0 { "  args   " } else { "         " },
                Style::new().fg(theme.text_faint),
            ),
            Span::styled(w2.clone(), Style::new().fg(theme.text_dim)),
        ]));
    }

    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled("  [y] allow  ", Style::new().fg(theme.ok).bold()),
        Span::styled("  [n] deny  ", Style::new().fg(theme.err).bold()),
        Span::styled("  [esc] cancel", Style::new().fg(theme.text_faint)),
    ]));

    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(theme.warn))
                    .title(Span::styled(" morrow ", Style::new().fg(theme.warn).bold()))
                    .style(Style::new().bg(theme.surface)),
            )
            .wrap(Wrap { trim: false }),
        modal,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::display_width;

    #[test]
    fn hint_width_stays_within_the_popup() {
        // The dropdown must not overflow its own inner width.
        let inner_w = 40usize;
        let hint_w = inner_w.saturating_sub(6);
        let label = "a".repeat(200);
        let shown = truncate_cols(&label, hint_w);
        assert!(display_width(&shown) <= hint_w);
    }

    #[test]
    fn approval_text_fits_a_narrow_modal() {
        // 24 cols is the narrowest modal we render; nothing should panic and the
        // tool name must survive truncation rather than splitting a character.
        let tool = "bash";
        let args = "x".repeat(500);
        let inner_w = 24usize.saturating_sub(4);
        let t = truncate_cols(tool, inner_w.saturating_sub(9));
        assert!(display_width(&t) <= inner_w.saturating_sub(9));
        let _ = crate::text::wrap(&args, inner_w.saturating_sub(9));
    }
}

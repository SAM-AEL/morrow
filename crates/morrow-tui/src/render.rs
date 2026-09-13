use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::app::App;
use crate::loading;

// Own morrow palette — near-black page, whisper borders, one cyan accent,
// amber reserved for warnings/approvals. Nothing pure-white, nothing loud.
const BORDER: Color = Color::Rgb(58, 58, 66);
const BORDER_BUSY: Color = Color::Cyan;
const TEXT: Color = Color::Rgb(225, 225, 228);
const DIM: Color = Color::Rgb(138, 138, 143);
const FAINT: Color = Color::Rgb(105, 105, 110);
const ACCENT: Color = Color::Cyan;
const WARN: Color = Color::Rgb(232, 178, 100);

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::new().bg(Color::Black), area);

    // Approval modal overlays everything.
    if let Some((tool, arguments)) = app.pending_approval() {
        draw_main(f, app, area);
        draw_approval(f, tool, arguments, area);
        return;
    }

    draw_main(f, app, area);
}

fn draw_main(f: &mut Frame, app: &App, area: Rect) {
    // Degrade gracefully instead of panicking on tiny terminals.
    if area.height < 12 || area.width < 30 {
        f.render_widget(
            Paragraph::new("terminal too small — morrow needs 30×12").wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    // Breathe: outer side margins (wider on wide terminals) and a bottom pad
    // so the input floats instead of sitting on the terminal edge.
    // Gaps are Max(1): present when there's room, first to go when crushed.
    let mx = if area.width > 120 { 4 } else { 2 };
    let body = Rect {
        x: area.x + mx,
        y: area.y,
        width: area.width.saturating_sub(mx * 2),
        height: area.height,
    };
    let [top, _gap1, main, tip, _gap2, input, _pad] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Max(1),
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Max(1),
        Constraint::Length(3),
        Constraint::Max(1),
    ])
    .areas(body);

    draw_top(f, app, top);

    if app.transcript_len() == 0 && !app.is_busy() {
        draw_welcome(f, app, main);
    } else {
        draw_transcript(f, app, main);
    }

    draw_tip(f, app, tip);
    draw_input(f, app, input);
    draw_completions(f, app, body, input);
}

/// Autocomplete dropdown anchored above the input, Claude-Code style.
/// Shows matching slash commands or, after `/model `, available models.
fn draw_completions(f: &mut Frame, app: &App, area: Rect, input: Rect) {
    if app.is_busy() || !app.input().trim_start().starts_with('/') {
        return;
    }
    let items = app.completions();
    if items.is_empty() {
        return;
    }
    let sel = app.completion_sel().min(items.len() - 1);
    let shown = items.len().min(8);
    // If the value list is long, window around the selection.
    let start = if items.len() <= 8 {
        0
    } else {
        sel.saturating_sub(4).min(items.len() - 8)
    };

    let width = 58.min(area.width.saturating_sub(4)).max(30);
    let height = (shown as u16 + 2).min(input.y.saturating_sub(area.y).max(3));
    if height < 3 {
        return;
    }
    let popup = Rect {
        x: input.x.min(area.x + area.width.saturating_sub(width)),
        y: input.y.saturating_sub(height + 1),
        width,
        height,
    };
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(Span::styled(" tab ↵ ", Style::new().fg(FAINT)))
        .bg(Color::Black);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let rows: Vec<Line> = items
        .iter()
        .skip(start)
        .take(shown)
        .enumerate()
        .map(|(i, c)| {
            let idx = start + i;
            // Command names complete bare; values (models, characters, sessions)
            // already carry their display form.
            let trimmed = app.input().trim_start();
            let is_value = trimmed.contains(char::is_whitespace);
            let label = if is_value {
                c.display.clone()
            } else {
                format!("/{}", c.display)
            };
            if idx == sel {
                Line::from(vec![
                    Span::styled("› ", Style::new().fg(ACCENT).bold()),
                    Span::styled(label, Style::new().fg(TEXT).bold().reversed()),
                    Span::styled(format!("  {}", c.hint), Style::new().fg(DIM)),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(label, Style::new().fg(TEXT)),
                    Span::styled(format!("  {}", c.hint), Style::new().fg(FAINT)),
                ])
            }
        })
        .collect();
    f.render_widget(Paragraph::new(rows), inner);
}

/// Own status bar: `✦ morrow · nova · approval · idle` left, cwd right.
fn draw_top(f: &mut Frame, app: &App, area: Rect) {
    let mut cwd = std::env::current_dir()
        .map(|p| {
            let home = std::env::var("HOME").unwrap_or_default();
            let s = p.display().to_string();
            if !home.is_empty() && s.starts_with(&home) {
                format!("~{}", &s[home.len()..])
            } else {
                s
            }
        })
        .unwrap_or_default();

    let left = Line::from(vec![
        Span::styled("✦ ", Style::new().fg(ACCENT).bold()),
        Span::styled("morrow", Style::new().fg(TEXT).bold()),
        Span::styled(format!(" · {}", app.status_right()), Style::new().fg(DIM)),
    ]);
    // Shorten long paths from the left so the status never gets pushed out.
    let left_len = 2 + "morrow".len() + 3 + app.status_right().len();
    let max_cwd = (area.width as usize).saturating_sub(left_len + 4);
    if cwd.len() > max_cwd && max_cwd > 8 {
        cwd = format!(
            "…{}",
            cwd.chars()
                .skip(cwd.chars().count() - max_cwd + 1)
                .collect::<String>()
        );
    }
    // Split left/right: status left, cwd right dim.
    let [l, r] = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(cwd.len().max(1) as u16 + 2),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(left), l);
    f.render_widget(
        Paragraph::new(Line::styled(format!("{cwd} "), Style::new().fg(FAINT)))
            .alignment(Alignment::Right),
        r,
    );
}

/// Own intro — centered rainbow wordmark, tagline, static hairline, hints.
/// No spinners or beams here: motion is reserved for the working state.
fn draw_welcome(f: &mut Frame, app: &App, area: Rect) {
    let tick = app.spinner_tick();
    let version: &str = env!("CARGO_PKG_VERSION");

    let mut title = loading::rainbow_title("morrow", tick);
    title.alignment = Some(Alignment::Center);

    let ver =
        Line::styled(format!("v{version}"), Style::new().fg(FAINT)).alignment(Alignment::Center);

    let tag = Line::styled(
        "Character-based agents that learn with every turn",
        Style::new().fg(DIM),
    )
    .alignment(Alignment::Center);

    // Static hairline — deliberately not animated.
    let rule_w = 44.min(area.width.saturating_sub(10) as usize).max(16);
    let rule =
        Line::styled("─".repeat(rule_w), Style::new().fg(FAINT)).alignment(Alignment::Center);

    // Command chips with quiet separators — scannable, not shouty.
    let mut chip_spans = Vec::new();
    for (i, name) in ["/help", "/model", "/switch", "/mode", "/clear"]
        .iter()
        .enumerate()
    {
        if i > 0 {
            chip_spans.push(Span::styled("   ·   ", Style::new().fg(FAINT)));
        }
        chip_spans.push(Span::styled(name.to_string(), Style::new().fg(ACCENT)));
    }
    let chips = Line::from(chip_spans).alignment(Alignment::Center);

    let hint = Line::styled("Type to chat  ·  / for commands", Style::new().fg(FAINT))
        .alignment(Alignment::Center);

    let mut lines = if area.height >= 14 {
        vec![
            Line::from(""),
            title,
            ver,
            Line::from(""),
            Line::from(""),
            tag,
            Line::from(""),
            Line::from(""),
            rule,
            Line::from(""),
            Line::from(""),
            chips,
            hint,
        ]
    } else {
        // Short terminal: essentials only, never overflow.
        vec![title, ver, tag, chips, hint]
    };
    // Fresh-launch pointer to past threads (kept out of the transcript so the
    // hero stays visible).
    if let Some(banner) = app.session_banner() {
        if (lines.len() + 2) as u16 <= area.height {
            lines.push(Line::from(""));
            lines.push(
                Line::styled(banner.to_string(), Style::new().fg(DIM)).alignment(Alignment::Center),
            );
        }
    }
    if lines.len() as u16 > area.height {
        lines.truncate(area.height.max(1) as usize);
    }

    // Center block vertically inside main area.
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect {
        x: area.x + 2,
        y,
        width: area.width.saturating_sub(4),
        height: h,
    };
    f.render_widget(Paragraph::new(lines), rect);
}

/// Borderless transcript with breathing room — messages carry the color.
fn draw_transcript(f: &mut Frame, app: &App, area: Rect) {
    let padded = Rect {
        x: area.x + 2,
        y: area.y,
        width: area.width.saturating_sub(4),
        height: area.height,
    };
    let height = padded.height as usize;
    let (visible, _start) = app.visible_lines(height);
    let lines: Vec<Line> = visible.iter().map(clone_line).collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), padded);
}

/// Dim hint normally; slim spinner status while the character works.
fn draw_tip(f: &mut Frame, app: &App, area: Rect) {
    let padded = Rect {
        x: area.x + 3,
        y: area.y,
        width: area.width.saturating_sub(6),
        height: 1,
    };
    if app.is_busy() {
        let tick = app.spinner_tick();
        let frame = SPINNER[(tick as usize) % SPINNER.len()];
        let line = Line::from(vec![
            Span::styled(format!("{frame} "), Style::new().fg(ACCENT)),
            Span::styled(
                format!("{}…", loading::thinking_verb(tick)),
                Style::new().fg(DIM),
            ),
            Span::styled("  · each turn makes me stronger", Style::new().fg(FAINT)),
        ]);
        f.render_widget(Paragraph::new(line), padded);
    } else if !app.input().trim_start().starts_with('/') || app.completions().is_empty() {
        if app.transcript_len() > 0 {
            let line = Line::from(vec![Span::styled(
                "/help · /switch · /mode · /clear · ctrl+q quit",
                Style::new().fg(FAINT),
            )]);
            f.render_widget(Paragraph::new(line), padded);
        } else {
            // Welcome already shows hints — keep this row empty for air.
            f.render_widget(Paragraph::new(Line::from("")), padded);
        }
    } else {
        let line = Line::from(vec![Span::styled(
            "Tab complete · ↑↓ navigate · Enter send",
            Style::new().fg(DIM),
        )]);
        f.render_widget(Paragraph::new(line), padded);
    }
}

/// Own input: rounded, `› text▊` with placeholder, hints docked bottom-right.
fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if app.is_busy() { BORDER_BUSY } else { BORDER }))
        .title(Span::styled(" prompt ", Style::new().fg(DIM).bold()))
        .title_bottom(
            Line::from(vec![Span::styled(
                " ↵ send · ctrl+c exit · ctrl+q quit ",
                Style::new().fg(FAINT),
            )])
            .alignment(Alignment::Right),
        )
        .bg(Color::Black);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let has_text = !app.input().is_empty();
    let mut spans = vec![Span::styled("› ", Style::new().fg(ACCENT).bold())];
    if has_text {
        spans.push(Span::styled(app.input().to_string(), Style::new().fg(TEXT)));
        spans.push(Span::styled("▊", Style::new().fg(TEXT)));
    } else {
        spans.push(Span::styled(
            "ask anything…",
            Style::new().fg(FAINT).italic(),
        ));
    }
    let rect = Rect {
        x: inner.x + 1,
        y: inner.y + (inner.height.saturating_sub(1)) / 2,
        width: inner.width.saturating_sub(2),
        height: 1,
    };
    f.render_widget(Paragraph::new(Line::from(spans)), rect);
}

fn draw_approval(f: &mut Frame, tool: &str, arguments: &str, area: Rect) {
    let modal = centered_box(area.width * 60 / 100, 11, area);
    f.render_widget(Clear, modal);
    let text = vec![
        Line::styled(" permission required ", Style::new().bold().fg(WARN)),
        Line::from(""),
        Line::from(vec![
            Span::styled("tool  ", Style::new().fg(FAINT)),
            Span::styled(tool.to_string(), Style::new().fg(TEXT).bold()),
        ]),
        Line::from(vec![
            Span::styled("args  ", Style::new().fg(FAINT)),
            Span::styled(truncate(arguments, 300), Style::new().fg(DIM)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("[y] allow", Style::new().fg(Color::Green).bold()),
            Span::styled("    [n] deny    [esc]", Style::new().fg(FAINT)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(WARN))
                    .title(" morrow "),
            )
            .wrap(Wrap { trim: false }),
        modal,
    );
}

fn clone_line(l: &Line) -> Line<'static> {
    Line {
        style: l.style,
        alignment: l.alignment,
        spans: l
            .spans
            .iter()
            .map(|s| Span::styled(s.content.to_string(), s.style))
            .collect(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.replace('\n', " ↵ ")
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{}…", t.replace('\n', " ↵ "))
    }
}

fn centered_box(width: u16, height: u16, r: Rect) -> Rect {
    let w = width.min(r.width);
    let h = height.min(r.height);
    Rect {
        x: r.x + r.width.saturating_sub(w) / 2,
        y: r.y + r.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;

    fn screen(app: &App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn welcome_hero_renders_without_panic() {
        let mut app = App::new();
        app.header_info = "nova · no model · approval".into();
        let out = screen(&app, 100, 30);
        assert!(out.contains("morrow"), "hero wordmark missing");
    }

    #[test]
    fn narrow_terminal_renders_without_panic() {
        let mut app = App::new();
        app.header_info = "nova".into();
        let out = screen(&app, 40, 12);
        assert!(out.contains("morrow"));
    }

    #[test]
    fn transcript_and_completions_render() {
        use crossterm::event::{
            Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        };
        let mut app = App::new();
        app.header_info = "nova".into();
        app.set_models(vec![("fast".into(), "m1".into(), "p".into())]);
        app.push_system("hello");
        for c in "/mod".chars() {
            app.on_key(Event::Key(KeyEvent {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }));
        }
        let out = screen(&app, 100, 30);
        assert!(out.contains("/model"), "completion dropdown missing");
        assert!(out.contains("hello"));
    }
}

//! Frame composition: the entry point the event loop calls each tick.
//!
//! Layout decisions live in [`layout`], so this module stays a thin conductor:
//! measure, delegate to a band renderer, then draw overlays on top.

pub mod chrome;
pub mod layout;
pub mod overlay;
pub mod transcript;

use ratatui::prelude::*;
use ratatui::widgets::Block;

use crate::app::App;
use crate::render::layout::{rows, Metrics};
use crate::theme::Theme;

/// Draw one complete frame.
pub fn draw(f: &mut Frame, app: &App, theme: &Theme) {
    let area = f.area();
    // Paint the page first so every later widget composites over a known base.
    f.render_widget(Block::new().style(Style::new().bg(theme.bg)), area);

    let m = Metrics::new(area);

    // An approval modal owns the whole frame: draw a minimal backdrop so the
    // popup reads as focused, then return.
    if let Some((tool, args)) = app.pending_approval() {
        overlay::draw_approval(f, tool, args, area, theme);
        return;
    }

    let r = rows(&m);
    chrome::draw_status(f, app, &m, r.status, theme);

    if app.transcript_len() == 0 && !app.is_busy() && app.assistant_buf().is_empty() {
        chrome::draw_welcome(f, app, &m, r.transcript, theme);
    } else {
        transcript::draw(f, app, &m, r.transcript, theme);
    }

    if r.hint.height > 0 {
        chrome::draw_hint(f, app, &m, r.hint, theme);
    }
    chrome::draw_input(f, app, &m, r.input, theme);
    overlay::draw_completions(f, app, &m, m.body, r.input, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Block, Level};
    use ratatui::backend::TestBackend;

    fn screen(app: &App, theme: &Theme, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app, theme)).unwrap();
        terminal.backend().to_string()
    }

    fn populated() -> App {
        let mut app = App::new();
        app.header_info = "nova · grok-fast · approval".into();
        app.push_blocks([
            Block::Assistant {
                text: "## Findings\n\nThe loader builds a client each turn.\n\n- one\n- two".into(),
            },
            Block::ToolCall {
                name: "bash".into(),
                arguments: r#"{"command":"cargo test"}"#.into(),
            },
            Block::ToolResult {
                name: "bash".into(),
                ok: true,
                result: (0..5)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            Block::System {
                text: "done".into(),
                level: Level::Notice,
            },
        ]);
        app
    }

    #[test]
    fn welcome_hero_renders() {
        let app = App::new();
        let out = screen(&app, &Theme::DARK, 100, 30);
        assert!(out.contains("morrow"), "wordmark missing");
    }

    #[test]
    fn populated_transcript_renders_across_every_breakpoint() {
        let app = populated();
        // Every tier from compact to ultrawide must render without panicking and
        // without losing the transcript content.
        for w in [20u16, 39, 40, 80, 120, 200] {
            let out = screen(&app, &Theme::DARK, w, 24);
            assert!(!out.trim().is_empty(), "blank frame at width {w}");
        }
        // At a width with room for the status bar, identity must be present.
        for w in [40u16, 80, 120] {
            let out = screen(&app, &Theme::DARK, w, 24);
            assert!(out.contains("morrow"), "identity lost at width {w}");
        }
    }

    #[test]
    fn narrow_terminals_keep_the_transcript_visible() {
        let app = populated();
        // Even at 20 columns the tool output must reach the screen: losing the
        // content entirely would be worse than a cramped layout.
        let out = screen(&app, &Theme::DARK, 20, 40);
        assert!(
            out.contains("TOOL") || out.contains("CALL"),
            "tool content lost at 20 cols:\n{out}"
        );
    }

    #[test]
    fn renders_in_every_theme() {
        let app = populated();
        for theme in [Theme::DARK, Theme::LIGHT, Theme::MONO] {
            let out = screen(&app, &theme, 100, 30);
            assert!(!out.is_empty());
        }
    }

    #[test]
    fn tool_output_is_visible_not_flattened() {
        let app = populated();
        let out = screen(&app, &Theme::DARK, 100, 30);
        assert!(out.contains("TOOL"), "tool label missing");
        assert!(out.contains("bash"), "tool name missing");
    }

    #[test]
    fn short_terminals_degrade_instead_of_panicking() {
        let app = populated();
        for w in [1u16, 2, 8, 20] {
            for h in [1u16, 2, 5, 11] {
                let _ = screen(&app, &Theme::DARK, w, h);
            }
        }
    }

    #[test]
    fn streaming_text_appears_while_busy() {
        let mut app = App::new();
        app.header_info = "nova".into();
        app.set_busy(true);
        app.apply(crate::app::AppEvent::Text("streaming answer".into()));
        let out = screen(&app, &Theme::DARK, 80, 24);
        assert!(out.contains("streaming"), "buffered text not drawn");
    }

    #[test]
    fn empty_transcript_shows_the_hero_not_a_blank_page() {
        let app = App::new();
        let out = screen(&app, &Theme::DARK, 80, 24);
        assert!(out.contains("ask anything") || out.contains("morrow"));
    }
}

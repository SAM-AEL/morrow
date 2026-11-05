//! The interactive terminal UI.
//!
//! `App` holds state and knows nothing about I/O; this module owns the terminal,
//! the event loop, and the translation from slash commands into runner calls.
//! The loop's job is to stay out of the way: coalesce a burst of streaming
//! deltas into one redraw, animate only while something is happening, and redraw
//! only when something actually changed.

mod app;
mod approval;
pub mod loading;
mod render;
pub mod text;
pub mod theme;

use anyhow::Result;
use crossterm::event::{Event as TermEvent, EventStream};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use std::panic::AssertUnwindSafe;
use tokio::sync::mpsc;
use tokio::time::Duration;

use futures::{FutureExt, StreamExt};
use morrow_agent::{AgentEvent, Runner};

pub use approval::TuiApproval;
pub use theme::Theme;

/// Frame interval while the character is working — smooth enough for a spinner.
const TICK_BUSY: Duration = Duration::from_millis(90);
/// Frame interval when idle. Nothing is animating, so this only needs to be
/// responsive to the rare state change.
const TICK_IDLE: Duration = Duration::from_millis(400);

/// Run the interactive TUI until the user quits.
pub async fn run(runner: Runner) -> Result<()> {
    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            // Teardown before propagating, or the terminal is left in raw mode.
            let _ = disable_raw_mode();
            let _ = std::io::stdout().execute(LeaveAlternateScreen);
            return Err(e.into());
        }
    };

    // A panic inside the loop would otherwise leave the user with a raw-mode,
    // alternate-screen terminal and no shell. Restore on every exit path.
    let outcome = AssertUnwindSafe(tui_main(&mut terminal, runner))
        .catch_unwind()
        .await;

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;

    match outcome {
        Ok(res) => res,
        Err(_) => Err(anyhow::anyhow!("morrow tui panicked; terminal restored")),
    }
}

async fn tui_main(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    initial_runner: Runner,
) -> Result<()> {
    let theme = Theme::detect();
    let mut runner: Option<Runner> = Some(initial_runner);
    let mut busy = false;

    // Project threads (.morrow/sessions). Fresh launch: preserve any live
    // conversation into its thread file, then start blank.
    let project_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    if let Some(r) = runner.as_mut() {
        r.attach_project_sessions(project_root.clone());
        if !r.character.messages.is_empty() {
            let _ = r.snapshot_session();
        }
        if r.list_sessions().map(|s| !s.is_empty()).unwrap_or(false) {
            let _ = r.start_new_session(None);
        }
    }

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<app::AppEvent>();
    let approval = std::sync::Arc::new(TuiApproval::new());
    let mut approval_rx = approval
        .take_receiver()
        .expect("approval channel taken twice");
    let mut term_events = EventStream::new();
    // The tick period is owned here rather than by `tokio::time::Interval`,
    // whose `set_interval` is still unstable. Recreating a sleep per iteration
    // is cheap and lets the idle and busy rates differ.
    let tick_sleep = tokio::time::sleep(TICK_BUSY);
    tokio::pin!(tick_sleep);

    let mut app = app::App::new();
    if let Some(r) = runner.as_ref() {
        app.header_info = r.status_line();
        refresh_context(&mut app, r);
    }
    app.set_project_root(project_root.clone());
    // Character names for `/switch` completion (best effort — empty is fine).
    if let Ok(chars) = morrow_core::CharacterStore::new().list() {
        app.set_characters(chars.iter().map(|c| c.name.clone()).collect());
    }

    // Signature of the last drawn frame, so an idle loop costs nothing.
    let mut last_signature = u64::MAX;

    'outer: loop {
        // --- dispatch a submitted command ---------------------------------
        if !busy && runner.is_some() {
            if let Some(cmd) = app.take_submitted() {
                handle_command(cmd, &mut app, &mut runner, &mut busy, &ev_tx).await;
            }
        }

        // --- drain every queued agent event before drawing ---------------
        // Streaming produces deltas far faster than we can draw. Coalescing them
        // into a single redraw is the difference between a smooth transcript and
        // a UI that redraws thousands of times a second.
        let mut drained = false;
        while let Ok(ev) = ev_rx.try_recv() {
            drained = true;
            match ev {
                app::AppEvent::Done(r) => {
                    busy = false;
                    app.on_turn_done();
                    let r = *r;
                    refresh_sessions(&mut app, &r);
                    runner = Some(r);
                }
                other => app.apply(other),
            }
        }
        let _ = drained;

        // --- draw only when something changed ----------------------------
        let signature = app.frame_signature();
        if signature != last_signature {
            terminal.draw(|f| render::draw(f, &app, &theme))?;
            last_signature = signature;
        }

        enum Ev {
            Agent(app::AppEvent),
            Term(TermEvent),
            Approval(approval::PendingApproval),
            Tick,
        }

        // Re-arm the animation clock at whichever rate the current state needs.
        let period = if app.is_busy() { TICK_BUSY } else { TICK_IDLE };
        tick_sleep
            .as_mut()
            .reset(tokio::time::Instant::now() + period);

        let ev = tokio::select! {
            e = ev_rx.recv() => match e { Some(e) => Ev::Agent(e), None => break 'outer },
            e = approval_rx.recv() => match e {
                Some(r) => Ev::Approval(r),
                // The approval channel closing would otherwise spin this loop
                // at 100% CPU; treat it as "no more approvals".
                None => { approval_rx.close(); continue 'outer; }
            },
            _ = &mut tick_sleep => Ev::Tick,
            e = term_events.next() => match e {
                Some(Ok(TermEvent::Key(k))) if k.kind == crossterm::event::KeyEventKind::Press => {
                    Ev::Term(TermEvent::Key(k))
                }
                Some(Ok(other)) => Ev::Term(other),
                None => break 'outer,
                _ => continue 'outer,
            },
        };

        match ev {
            Ev::Tick => {
                // Animate only when there is something to animate.
                let animating = app.is_busy() || (app.transcript_len() == 0 && !app.should_quit());
                if animating {
                    app.bump_tick();
                }
            }
            Ev::Approval(req) => app.begin_approval(req),
            Ev::Term(term_ev) => {
                app.on_key(term_ev);
                if app.should_quit() {
                    break 'outer;
                }
            }
            Ev::Agent(agent_ev) => {
                if let app::AppEvent::Done(r) = agent_ev {
                    busy = false;
                    app.on_turn_done();
                    refresh_sessions(&mut app, &r);
                    runner = Some(*r);
                } else {
                    app.apply(agent_ev);
                }
            }
        }
    }

    Ok(())
}

/// Execute one submitted command against the live runner.
async fn handle_command(
    cmd: app::Command,
    app: &mut app::App,
    runner: &mut Option<Runner>,
    busy: &mut bool,
    ev_tx: &mpsc::UnboundedSender<app::AppEvent>,
) {
    let Some(r) = runner.as_mut() else { return };
    match cmd {
        app::Command::Agent(prompt) => {
            *busy = true;
            app.set_busy(true);
            launch_turn(
                runner.take().expect("runner checked"),
                prompt,
                ev_tx.clone(),
            );
        }
        app::Command::Switch(name) => {
            let label = match r.switch_character(&name) {
                Ok(()) => r.status_line(),
                Err(e) => format!("switch failed: {e}"),
            };
            app.header_info = label;
            if let Ok(chars) = morrow_core::CharacterStore::new().list() {
                app.set_characters(chars.iter().map(|c| c.name.clone()).collect());
            }
            refresh_context(app, r);
        }
        app::Command::NewSession(title) => {
            match r.start_new_session(title) {
                Ok(s) => app.push_system(&format!(
                    "── new session {} · past threads kept (/sessions) ──",
                    app::short_id(&s.id)
                )),
                Err(e) => app.push_error(&format!("new session failed: {e}")),
            }
            app.header_info = r.status_line();
            refresh_context(app, r);
        }
        app::Command::Resume(query) => {
            match r.resume_session(&query) {
                Ok(s) => {
                    app.replay_transcript(&s.messages);
                    app.push_system(&format!(
                        "── resumed {} · {} ──",
                        app::short_id(&s.id),
                        s.title
                    ));
                    app.header_info = r.status_line();
                }
                Err(e) => app.push_error(&format!("resume failed: {e}")),
            }
            refresh_context(app, r);
        }
        app::Command::Rename(arg) => {
            if arg.is_none() {
                let s = r.current_session_view();
                app.push_system(&format!(
                    "thread {} · \"{}\"{}{}",
                    app::short_id(&s.id),
                    s.title,
                    if s.title_manual { " (manual)" } else { "" },
                    " — /rename <title> or /rename --auto"
                ));
            } else {
                match r.rename_session(arg) {
                    Ok(s) => app.push_system(&format!("renamed to \"{}\"", s.title)),
                    Err(e) => app.push_error(&format!("rename failed: {e}")),
                }
                refresh_context(app, r);
            }
        }
        app::Command::Compact(note) => {
            // Compaction is a round trip: show the working state around it.
            app.set_busy(true);
            let outcome = r
                .compact_now(note, &mut |ev| {
                    if let AgentEvent::Notice(n) = ev {
                        app.push_system(&format!("· {n}"));
                    }
                })
                .await;
            app.set_busy(false);
            match outcome {
                Ok(reclaimed) => {
                    app.push_system(&format!("compacted — reclaimed ~{reclaimed} tokens"))
                }
                Err(e) => app.push_error(&format!("compact skipped: {e}")),
            }
            refresh_context(app, r);
        }
        app::Command::Flush(note) => {
            app.set_busy(true);
            let outcome = r
                .flush_memory(note, &mut |ev| {
                    if let AgentEvent::Notice(n) = ev {
                        app.push_system(&format!("· {n}"));
                    }
                })
                .await;
            app.set_busy(false);
            match outcome {
                Ok(chars) => app.push_system(&format!(
                    "flushed ~{chars} chars of learnings to .morrow/MEMORY.md"
                )),
                Err(e) => app.push_error(&format!("flush failed: {e}")),
            }
        }
        app::Command::Info => {
            let s = r.current_session_view();
            let user_turns = r
                .character
                .messages
                .iter()
                .filter(|m| matches!(m.role, morrow_core::Role::User))
                .count();
            for l in [
                format!("thread: {} ({})", app::short_id(&s.id), s.title),
                format!(
                    "character: {} · model: {}",
                    r.character_name(),
                    r.model_key().unwrap_or("no model")
                ),
                format!(
                    "messages: {} ({} user turns) · updated {}",
                    s.messages.len(),
                    user_turns,
                    s.updated_at.format("%m-%d %H:%M")
                ),
                format!(
                    "tokens this run: {} in / {} out",
                    app.usage().input_tokens,
                    app.usage().output_tokens
                ),
            ] {
                app.push_system(&l);
            }
        }
        app::Command::Context => {
            for l in r.context_breakdown() {
                app.push_system(&l);
            }
        }
        app::Command::Mcp => {
            let live = r.mcp_status();
            if r.config.mcp_servers.is_empty() {
                app.push_system("no MCP servers configured — see [mcp_servers.*] in config");
            } else {
                for (name, cfg) in &r.config.mcp_servers {
                    let state = live
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, n)| format!("connected · {n} tools"))
                        .unwrap_or_else(|| {
                            if cfg.enabled {
                                "connects on next turn".into()
                            } else {
                                "disabled".into()
                            }
                        });
                    app.push_system(&format!("mcp {name}: {state} ({})", cfg.command));
                }
            }
        }
        app::Command::ToggleMode => {
            r.toggle_mode();
            app.header_info = r.status_line();
            let mode = if r.mode() == morrow_core::Mode::Autonomous {
                "AUTONOMOUS (acting without asking)"
            } else {
                "APPROVAL (confirming mutations)"
            };
            app.push_notice(&format!("mode: {mode} — {}", r.status_line()));
        }
        app::Command::Local(blocks) => {
            app.push_blocks(blocks);
            app.apply_local_command(r);
        }
    }
}

/// Refresh everything `/help`-style commands read from the runner.
fn refresh_context(app: &mut app::App, r: &Runner) {
    refresh_sessions(app, r);
    app.set_custom_commands(morrow_core::load_custom_commands(
        &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    ));
    app.set_rules(
        r.project_rules()
            .files
            .iter()
            .map(|(p, c)| (p.clone(), c.len()))
            .collect(),
    );
    app.set_models(
        r.config
            .models
            .iter()
            .map(|(k, m)| (k.clone(), m.id.clone(), m.provider.clone()))
            .collect(),
    );
    app.set_agents(
        r.config
            .agents
            .iter()
            .filter(|(_, a)| !a.hidden)
            .map(|(n, a)| (n.clone(), a.description.clone().unwrap_or_default()))
            .collect(),
    );
    let n = r.list_sessions().map(|s| s.len()).unwrap_or(0);
    app.set_session_banner(if n > 0 {
        Some(format!(
            "{n} past session{} for {} here — /resume to continue",
            if n == 1 { "" } else { "s" },
            r.character_name()
        ))
    } else {
        None
    });
}

fn refresh_sessions(app: &mut app::App, r: &Runner) {
    let items: Vec<(String, String, String, usize)> = r
        .list_sessions()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                s.title.clone(),
                s.updated_at.format("%m-%d %H:%M").to_string(),
                s.messages.len(),
            )
        })
        .collect();
    app.set_sessions(items);
}

fn launch_turn(mut runner: Runner, prompt: String, ev_tx: mpsc::UnboundedSender<app::AppEvent>) {
    tokio::spawn(async move {
        let mut on_event = |ev: AgentEvent| {
            let _ = ev_tx.send(app::AppEvent::from_agent(ev));
        };
        if let Err(e) = runner.chat(&prompt, &mut on_event).await {
            let _ = ev_tx.send(app::AppEvent::error(format!("{e}")));
        }
        let _ = ev_tx.send(app::AppEvent::from_agent(AgentEvent::Notice(
            "turn complete".into(),
        )));
        let _ = ev_tx.send(app::AppEvent::Done(Box::new(runner)));
    });
}

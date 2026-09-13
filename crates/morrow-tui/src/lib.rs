mod app;
mod approval;
pub mod loading;
mod render;

use anyhow::Result;
use crossterm::event::EventStream;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use tokio::sync::mpsc;

use futures::StreamExt;
use morrow_agent::{AgentEvent, Runner};

pub use approval::TuiApproval;

/// Run the interactive TUI until the user quits.
pub async fn run(runner: Runner) -> Result<()> {
    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let res = tui_main(&mut terminal, runner).await;

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;
    res
}

async fn tui_main(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    initial_runner: Runner,
) -> Result<()> {
    let mut runner: Option<Runner> = Some(initial_runner);
    let status = runner.as_ref().map(|r| r.status_line()).unwrap_or_default();
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
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(90));

    let mut app = app::App::new();
    app.header_info = status;
    app.set_project_root(project_root.clone());
    // Character names for `/switch` completion (best effort — empty is fine).
    if let Ok(chars) = morrow_core::CharacterStore::new().list() {
        app.set_characters(chars.iter().map(|c| c.name.clone()).collect());
    }
    if let Some(r) = runner.as_ref() {
        refresh_sessions(&mut app, r);
        app.set_custom_commands(morrow_core::load_custom_commands(&project_root));
        app.set_rules(
            r.project_rules()
                .files
                .iter()
                .map(|(p, c)| (p.clone(), c.len()))
                .collect(),
        );
        let n = r.list_sessions().map(|s| s.len()).unwrap_or(0);
        if n > 0 {
            app.set_session_banner(Some(format!(
                "{n} past session{} for {} here — /resume to continue",
                if n == 1 { "" } else { "s" },
                r.character_name()
            )));
        }
    }
    if let Some(r) = runner.as_ref() {
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
    }

    loop {
        // Idle + submitted message + runner available => handle or launch.
        if !busy && runner.is_some() {
            match app.take_submitted() {
                Some(app::Command::Agent(prompt)) => {
                    busy = true;
                    app.set_busy(true);
                    launch_turn(runner.take().unwrap(), prompt, ev_tx.clone(), &approval);
                }
                Some(app::Command::Switch(name)) => {
                    let r = runner.as_mut().unwrap();
                    let label = match r.switch_character(&name) {
                        Ok(()) => r.status_line(),
                        Err(e) => format!("switch failed: {e}"),
                    };
                    app.header_info = label;
                    // New character may be brand-new — refresh completion names.
                    if let Ok(chars) = morrow_core::CharacterStore::new().list() {
                        app.set_characters(chars.iter().map(|c| c.name.clone()).collect());
                    }
                    refresh_sessions(&mut app, runner.as_ref().unwrap());
                }
                Some(app::Command::NewSession(title)) => {
                    let r = runner.as_mut().unwrap();
                    match r.start_new_session(title) {
                        Ok(s) => app.push_system(&format!(
                            "── new session {} · past threads kept (/sessions) ──",
                            short_id(&s.id)
                        )),
                        Err(e) => app.push_system(&format!("new session failed: {e}")),
                    }
                    app.header_info = runner.as_ref().unwrap().status_line();
                    refresh_sessions(&mut app, runner.as_ref().unwrap());
                }
                Some(app::Command::Resume(query)) => {
                    let outcome = runner.as_mut().unwrap().resume_session(&query);
                    match outcome {
                        Ok(s) => {
                            let r = runner.as_ref().unwrap();
                            app.replay_transcript(&s.messages);
                            app.push_system(&format!(
                                "── resumed {} · {} ──",
                                short_id(&s.id),
                                s.title
                            ));
                            app.header_info = r.status_line();
                        }
                        Err(e) => app.push_system(&format!("resume failed: {e}")),
                    }
                    refresh_sessions(&mut app, runner.as_ref().unwrap());
                }
                Some(app::Command::Rename(arg)) => {
                    if arg.is_none() {
                        let s = runner.as_ref().unwrap().current_session_view();
                        app.push_system(&format!(
                            "thread {} · \"{}\"{}{}",
                            short_id(&s.id),
                            s.title,
                            if s.title_manual { " (manual)" } else { "" },
                            " — /rename <title> or /rename --auto"
                        ));
                    } else {
                        match runner.as_mut().unwrap().rename_session(arg) {
                            Ok(s) => app.push_system(&format!("renamed to \"{}\"", s.title)),
                            Err(e) => app.push_system(&format!("rename failed: {e}")),
                        }
                        refresh_sessions(&mut app, runner.as_ref().unwrap());
                    }
                }
                Some(app::Command::Compact(note)) => {
                    app.set_busy(true);
                    let mut notices = Vec::new();
                    let mut on_event = |ev: AgentEvent| {
                        if let AgentEvent::Notice(n) = ev {
                            notices.push(n);
                        }
                    };
                    let outcome = runner
                        .as_mut()
                        .unwrap()
                        .compact_now(note, &mut on_event)
                        .await;
                    for n in notices {
                        app.push_system(&format!("· {n}"));
                    }
                    match outcome {
                        Ok(reclaimed) => {
                            app.push_system(&format!("compacted — reclaimed ~{reclaimed} tokens"))
                        }
                        Err(e) => app.push_system(&format!("compact skipped: {e}")),
                    }
                    app.set_busy(false);
                    refresh_sessions(&mut app, runner.as_ref().unwrap());
                }
                Some(app::Command::Flush(note)) => {
                    app.set_busy(true);
                    let mut notices = Vec::new();
                    let mut on_event = |ev: AgentEvent| {
                        if let AgentEvent::Notice(n) = ev {
                            notices.push(n);
                        }
                    };
                    let outcome = runner
                        .as_mut()
                        .unwrap()
                        .flush_memory(note, &mut on_event)
                        .await;
                    for n in notices {
                        app.push_system(&format!("· {n}"));
                    }
                    match outcome {
                        Ok(chars) => app.push_system(&format!(
                            "flushed ~{chars} chars of learnings to .morrow/MEMORY.md"
                        )),
                        Err(e) => app.push_system(&format!("flush failed: {e}")),
                    }
                    app.set_busy(false);
                }
                Some(app::Command::Info) => {
                    let r = runner.as_ref().unwrap();
                    let s = r.current_session_view();
                    let user_turns = r
                        .character
                        .messages
                        .iter()
                        .filter(|m| matches!(m.role, morrow_core::Role::User))
                        .count();
                    for l in [
                        format!("thread: {} ({})", short_id(&s.id), s.title),
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
                Some(app::Command::Context) => {
                    for l in runner.as_ref().unwrap().context_breakdown() {
                        app.push_system(&l);
                    }
                }
                Some(app::Command::Mcp) => {
                    let r = runner.as_ref().unwrap();
                    let live = r.mcp_status();
                    if r.config.mcp_servers.is_empty() {
                        app.push_system(
                            "no MCP servers configured — see [mcp_servers.*] in config",
                        );
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
                Some(app::Command::ToggleMode) => {
                    let r = runner.as_mut().unwrap();
                    r.toggle_mode();
                    app.header_info = r.status_line();
                    app.push_system(&format!(
                        "mode: {} — {}",
                        if r.mode() == morrow_core::Mode::Autonomous {
                            "AUTONOMOUS (acting without asking)"
                        } else {
                            "APPROVAL (confirming mutations)"
                        },
                        r.status_line()
                    ));
                }
                Some(app::Command::Local(lines)) => {
                    for l in lines {
                        app.push_line(l);
                    }
                    if let Some(r) = runner.as_mut() {
                        app.apply_local_command(r);
                    }
                }
                None => {}
            }
        }

        // Reclaim finished runners.
        while let Ok(ev) = ev_rx.try_recv() {
            if let app::AppEvent::Done(r) = ev {
                busy = false;
                app.on_turn_done();
                let r = *r;
                refresh_sessions(&mut app, &r);
                runner = Some(r);
            } else {
                app.apply(ev);
            }
        }

        terminal.draw(|f| render::draw(f, &app))?;

        enum Ev {
            Agent(app::AppEvent),
            Term(crossterm::event::Event),
            Approval(approval::PendingApproval),
            Tick,
        }
        use crossterm::event::Event as TermEvent;

        let ev = tokio::select! {
            e = ev_rx.recv() => match e { Some(e) => Ev::Agent(e), None => break },
            e = approval_rx.recv() => match e { Some(r) => Ev::Approval(r), None => continue },
            _ = ticker.tick() => Ev::Tick,
            e = term_events.next() => match e {
                Some(Ok(TermEvent::Key(k))) if k.kind == crossterm::event::KeyEventKind::Press => Ev::Term(TermEvent::Key(k)),
                Some(Ok(other)) => Ev::Term(other),
                _ => continue,
            },
        };

        match ev {
            Ev::Tick => {
                // Always animate (intro rainbow + busy spinner).
                app.bump_tick();
            }
            Ev::Approval(req) => app.begin_approval(req),
            Ev::Term(term_ev) => {
                app.on_key(term_ev);
                if app.should_quit() {
                    break;
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

fn short_id(id: &str) -> &str {
    &id[..8.min(id.len())]
}

fn launch_turn(
    mut runner: Runner,
    prompt: String,
    ev_tx: mpsc::UnboundedSender<app::AppEvent>,
    _approval: &std::sync::Arc<TuiApproval>,
) {
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

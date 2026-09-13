use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::text::Line;

use morrow_core::message::{Message, Role, Usage};

use crate::approval::PendingApproval;

/// All slash commands with short descriptions for the autocomplete dropdown.
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("help", "show all commands"),
    ("new", "start a fresh session (current thread is kept)"),
    ("resume", "continue a past session — Tab lists threads"),
    ("sessions", "list past sessions for this character"),
    (
        "rename",
        "rename this thread (/rename --auto restores auto-titles)",
    ),
    (
        "compact",
        "compress history now (/compact [note on what to keep])",
    ),
    ("info", "show thread details: model, turns, usage"),
    ("context", "show context-window breakdown"),
    ("memory", "show project memory stats"),
    ("flush", "save this thread's learnings to .morrow/MEMORY.md"),
    ("mcp", "show connected external tool servers"),
    ("model", "switch model — Tab completes available models"),
    ("models", "list saved models"),
    ("switch", "switch character — Tab completes names"),
    ("mode", "toggle approval <-> autonomous"),
    ("agents", "list subagents"),
    ("rules", "show loaded project rule files"),
    ("clear", "clear transcript"),
    ("usage", "token totals"),
    ("quit", "exit morrow"),
];

/// Commands that take a free-text argument (get trailing space on complete).
fn command_takes_arg(name: &str) -> bool {
    matches!(name, "model" | "switch" | "agent" | "resume" | "new")
}

/// One completion row: what gets inserted vs. what is shown.
pub struct Completion {
    pub insert: String,
    pub display: String,
    pub hint: String,
}

/// What the user submitted: a prompt for the agent or a local command output.
pub enum Command {
    Agent(String),
    Local(Vec<Line<'static>>),
    /// Switch to another persistent character (/switch <name>).
    Switch(String),
    /// Toggle Approval <-> Autonomous for the active character.
    ToggleMode,
    /// Start a fresh session thread (/new [title]).
    NewSession(Option<String>),
    /// Resume a past session thread (/resume <id-prefix>).
    Resume(String),
    /// Rename the active thread (/rename [title|--auto]).
    Rename(Option<String>),
    /// Compress history now (/compact [note]).
    Compact(Option<String>),
    /// Save thread learnings to project memory (/flush [note]).
    Flush(Option<String>),
    /// Show active thread details.
    Info,
    /// Show context-window breakdown.
    Context,
    /// Show MCP server status.
    Mcp,
}

/// Events flowing from agent tasks into the UI.
pub enum AppEvent {
    Text(String),
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        ok: bool,
        result: String,
    },
    Usage(Usage),
    Notice(String),
    SubagentStart(String),
    SubagentEnd(String),
    Error(String),
    /// Turn finished; runner returned for reuse.
    Done(Box<morrow_agent::Runner>),
}

impl AppEvent {
    pub fn from_agent(ev: morrow_agent::AgentEvent) -> Self {
        match ev {
            morrow_agent::AgentEvent::TextDelta(d) => Self::Text(d),
            morrow_agent::AgentEvent::ToolCall { name, arguments } => {
                Self::ToolCall { name, arguments }
            }
            morrow_agent::AgentEvent::ToolResult { name, ok, result } => {
                Self::ToolResult { name, ok, result }
            }
            morrow_agent::AgentEvent::Usage(u) => Self::Usage(u),
            morrow_agent::AgentEvent::Notice(n) => Self::Notice(n),
            morrow_agent::AgentEvent::SubagentStart { name } => Self::SubagentStart(name),
            morrow_agent::AgentEvent::SubagentEnd { name } => Self::SubagentEnd(name),
            morrow_agent::AgentEvent::CharacterSwitched(name) => {
                Self::Notice(format!("switched to {name}"))
            }
            morrow_agent::AgentEvent::ModeChanged(auto) => Self::Notice(format!(
                "mode: {}",
                if auto { "autonomous" } else { "approval" }
            )),
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self::Error(msg.into())
    }
}

pub struct App {
    lines: Vec<Line<'static>>,
    input: String,
    scroll: usize, // lines from bottom
    submitted: Option<Command>,
    quit: bool,
    pending_approval: Option<PendingApproval>,
    usage: Usage,
    busy: bool,
    /// In-progress assistant text (streaming deltas accumulate here).
    assistant_buf: String,
    /// /model <key> applied when the runner is handed back.
    pending_model_switch: Option<String>,
    /// Header status line (character · model · mode).
    pub header_info: String,
    /// Animation frame for the rainbow loader (bumped on a timer while busy).
    tick: u64,
    /// Saved models for `/models` (key, id, provider) — injected at startup.
    models: Vec<(String, String, String)>,
    /// Subagents for `/agents` (name, description) — injected at startup.
    agents: Vec<(String, String)>,
    /// Known character names for `/switch` completion — injected at startup.
    characters: Vec<String>,
    /// Loaded project rule files for `/rules` — (display path, chars).
    rules: Vec<(String, usize)>,
    /// User-defined slash commands (.morrow/commands/*.md).
    custom_commands: Vec<morrow_core::CustomCommand>,
    /// Project root for project-scoped reads (/memory).
    project_root: std::path::PathBuf,
    /// Past threads for `/resume` completion + `/sessions` — (id, title, updated, msgs).
    sessions: Vec<(String, String, String, usize)>,
    /// Fresh-launch banner shown in the welcome hero ("3 past sessions — /resume").
    session_banner: Option<String>,
    /// Selected index into the live completion list (Tab/Up/Down).
    complete_sel: usize,
    /// How many transcript lines currently show the streaming assistant buffer.
    assistant_shown: usize,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            input: String::new(),
            scroll: 0,
            submitted: None,
            quit: false,
            pending_approval: None,
            usage: Usage::default(),
            busy: false,
            assistant_buf: String::new(),
            pending_model_switch: None,
            header_info: String::new(),
            tick: 0,
            models: Vec::new(),
            agents: Vec::new(),
            characters: Vec::new(),
            rules: Vec::new(),
            custom_commands: Vec::new(),
            project_root: std::path::PathBuf::from("."),
            sessions: Vec::new(),
            session_banner: None,
            complete_sel: 0,
            assistant_shown: 0,
        }
    }

    /// Inject the saved-model list shown by `/models`.
    pub fn set_models(&mut self, models: Vec<(String, String, String)>) {
        self.models = models;
    }

    /// Inject the subagent list shown by `/agents`.
    pub fn set_agents(&mut self, agents: Vec<(String, String)>) {
        self.agents = agents;
    }

    /// Inject known character names for `/switch` completion.
    pub fn set_characters(&mut self, characters: Vec<String>) {
        self.characters = characters;
    }

    /// Inject loaded project rule files for `/rules`.
    pub fn set_rules(&mut self, rules: Vec<(String, usize)>) {
        self.rules = rules;
    }

    /// Inject user-defined slash commands.
    pub fn set_custom_commands(&mut self, cmds: Vec<morrow_core::CustomCommand>) {
        self.custom_commands = cmds;
        self.clamp_completion_sel();
    }

    /// Inject the project root for project-scoped reads.
    pub fn set_project_root(&mut self, root: std::path::PathBuf) {
        self.project_root = root;
    }

    /// Inject past threads for `/resume` completion + `/sessions`.
    /// Items: (id, title, updated-display, message-count).
    pub fn set_sessions(&mut self, sessions: Vec<(String, String, String, usize)>) {
        self.sessions = sessions;
        self.clamp_completion_sel();
    }

    /// Fresh-launch banner rendered in the welcome hero (None = hide).
    pub fn set_session_banner(&mut self, banner: Option<String>) {
        self.session_banner = banner;
    }

    pub fn session_banner(&self) -> Option<&str> {
        self.session_banner.as_deref()
    }

    /// Replace the visible transcript with a resumed thread's messages.
    /// Streaming state is dropped; the notice caller adds goes on top.
    pub fn replay_transcript(&mut self, messages: &[Message]) {
        self.lines.clear();
        self.assistant_buf.clear();
        self.assistant_shown = 0;
        self.scroll = 0;
        for m in messages {
            let text = m.plain_text();
            if text.trim().is_empty() {
                continue;
            }
            match m.role {
                Role::User => self.lines.push(Line::styled(
                    format!("❯ {text}"),
                    Style::new().add_modifier(ratatui::style::Modifier::BOLD),
                )),
                Role::Assistant => {
                    for l in text.lines() {
                        self.lines.push(Line::from(l.to_string()));
                    }
                }
                _ => self.lines.push(Line::styled(
                    format!("· {}", text.lines().next().unwrap_or("")),
                    Style::new().add_modifier(ratatui::style::Modifier::DIM),
                )),
            }
        }
    }

    /// Rendered `/memory` summary.
    pub fn memory_lines(&self) -> Vec<Line<'static>> {
        match morrow_core::memory_stats(&self.project_root) {
            None => vec![Line::from(
                "no project memory yet — /flush saves this thread, remember(scope=project) too",
            )],
            Some((path, entries, bytes)) => vec![
                Line::from(format!("project memory: {path}")),
                Line::from(format!(
                    "  {entries} entries · {bytes} bytes · injected every turn"
                )),
            ],
        }
    }

    /// Rendered `/sessions` list (also used for bare `/resume`).
    pub fn sessions_lines(&self) -> Vec<Line<'static>> {
        if self.sessions.is_empty() {
            return vec![Line::from(
                "no past sessions here yet — chat away, /new starts a thread",
            )];
        }
        let mut out = vec![Line::from("past sessions (/resume <id>):")];
        for (id, title, updated, msgs) in &self.sessions {
            out.push(Line::from(format!(
                "  {}  {title}  · {updated} · {msgs} msgs",
                short_id(id)
            )));
        }
        out
    }

    /// Live completion items for the current input.
    pub fn completions(&self) -> Vec<Completion> {
        let input = self.input.trim_start();
        let Some(stripped) = input.strip_prefix('/') else {
            return Vec::new();
        };
        // "/model <partial>" / "/switch <partial>" / "/resume <partial>" — values.
        if let Some((cmd, rest)) = stripped.split_once(char::is_whitespace) {
            let partial = rest.trim_start().to_ascii_lowercase();
            match cmd {
                "model" => {
                    return self
                        .models
                        .iter()
                        .filter(|(key, id, provider)| {
                            partial.is_empty()
                                || key.to_ascii_lowercase().starts_with(&partial)
                                || id.to_ascii_lowercase().starts_with(&partial)
                                || provider.to_ascii_lowercase().starts_with(&partial)
                        })
                        .map(|(key, id, provider)| Completion {
                            insert: key.clone(),
                            display: key.clone(),
                            hint: format!("{id} · {provider}"),
                        })
                        .collect();
                }
                "switch" | "agent" => {
                    return self
                        .characters
                        .iter()
                        .filter(|n| {
                            partial.is_empty() || n.to_ascii_lowercase().starts_with(&partial)
                        })
                        .map(|n| Completion {
                            insert: n.clone(),
                            display: n.clone(),
                            hint: String::from("character"),
                        })
                        .collect();
                }
                "resume" => {
                    return self
                        .sessions
                        .iter()
                        .filter(|(id, title, _, _)| {
                            partial.is_empty()
                                || id.to_ascii_lowercase().starts_with(&partial)
                                || title.to_ascii_lowercase().contains(&partial)
                        })
                        .map(|(id, title, updated, msgs)| Completion {
                            insert: id.clone(),
                            display: format!("{} {}", short_id(id), title),
                            hint: format!("{updated} · {msgs} msgs"),
                        })
                        .collect();
                }
                _ => return Vec::new(),
            }
        }
        // "/<partial>" — complete command names (builtins + custom commands).
        let partial = stripped.to_ascii_lowercase();
        let mut out: Vec<Completion> = SLASH_COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(partial.as_str()))
            .map(|(name, desc)| Completion {
                insert: name.to_string(),
                display: name.to_string(),
                hint: desc.to_string(),
            })
            .collect();
        for c in &self.custom_commands {
            if c.name.starts_with(partial.as_str())
                && !SLASH_COMMANDS.iter().any(|(n, _)| *n == c.name)
            {
                out.push(Completion {
                    insert: c.name.clone(),
                    display: c.name.clone(),
                    hint: format!("{} · {}", c.description, c.scope),
                });
            }
        }
        out
    }

    /// Currently highlighted completion index (clamped by caller via completions()).
    pub fn completion_sel(&self) -> usize {
        self.complete_sel
    }

    fn clamp_completion_sel(&mut self) {
        let n = self.completions().len();
        if n == 0 {
            self.complete_sel = 0;
        } else {
            self.complete_sel = self.complete_sel.min(n - 1);
        }
    }

    /// Apply the highlighted completion to the input (Tab).
    fn apply_completion(&mut self) {
        let items = self.completions();
        if items.is_empty() {
            return;
        }
        let sel = self.complete_sel.min(items.len() - 1);
        let picked = items[sel].insert.clone();
        let is_command = {
            let input = self.input.trim_start();
            match input.strip_prefix('/') {
                Some(stripped) => !stripped.contains(char::is_whitespace),
                None => false,
            }
        };
        if is_command {
            // Command completion.
            self.input = format!("/{picked}");
            if command_takes_arg(&picked) {
                self.input.push(' ');
            }
        } else {
            // Value completion: replace text after the last whitespace.
            let pos = self
                .input
                .rfind(|c: char| c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(1);
            self.input.truncate(pos);
            self.input.push_str(&picked);
        }
        self.complete_sel = 0;
    }

    /// Advance the loader animation one frame.
    pub fn bump_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn spinner_tick(&self) -> u64 {
        self.tick
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// Called when a turn's runner is reclaimed: flush text + clear busy.
    pub fn on_turn_done(&mut self) {
        self.flush_assistant();
        self.finish_turn();
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn take_submitted(&mut self) -> Option<Command> {
        self.submitted.take()
    }

    pub fn push_line(&mut self, line: Line<'static>) {
        self.lines.push(line);
    }

    pub fn push_system(&mut self, text: &str) {
        self.lines.push(Line::styled(
            text.to_string(),
            Style::new().add_modifier(ratatui::style::Modifier::DIM),
        ));
    }

    /// Apply a local slash command against the live runner.
    pub fn apply_local_command(&mut self, runner: &mut morrow_agent::Runner) {
        if let Some(key) = self.pending_model_switch.take() {
            let msg = format!("model switched to '{key}'");
            runner.set_model(key);
            self.push_system(&msg);
        }
    }

    pub fn begin_approval(&mut self, req: PendingApproval) {
        self.pending_approval = Some(req);
    }

    pub fn on_key(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => self.on_key_event(key),
            Event::Mouse(m) => match m.kind {
                crossterm::event::MouseEventKind::ScrollUp => {
                    self.scroll = self.scroll.saturating_add(3)
                }
                crossterm::event::MouseEventKind::ScrollDown => {
                    self.scroll = self.scroll.saturating_sub(3)
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    fn on_key_event(&mut self, key: KeyEvent) {
        if key.kind == crossterm::event::KeyEventKind::Release {
            return;
        }
        if let Some(_req) = self.pending_approval.as_ref() {
            // modal active
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.resolve_approval(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.resolve_approval(false)
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.busy {
                    // cancel current input; real interruption of the model is future work
                    self.push_system("note: interrupting mid-turn is not supported yet");
                    self.input.clear();
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
                self.complete_sel = 0;
            }
            KeyCode::Up => {
                let n = self.completions().len();
                if n > 0 {
                    self.complete_sel = self.complete_sel.checked_sub(1).unwrap_or(n - 1);
                } else {
                    self.scroll = self.scroll.saturating_add(3);
                }
            }
            KeyCode::Down => {
                let n = self.completions().len();
                if n > 0 {
                    self.complete_sel = (self.complete_sel + 1) % n;
                } else {
                    self.scroll = self.scroll.saturating_sub(3);
                }
            }
            KeyCode::Tab => {
                self.clamp_completion_sel();
                self.apply_completion();
            }
            KeyCode::Esc => self.complete_sel = 0,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(20),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(20),
            KeyCode::Char(c) => {
                self.input.push(c);
                self.clamp_completion_sel();
            }
            _ => {}
        }
    }

    fn resolve_approval(&mut self, allow: bool) {
        if let Some(req) = self.pending_approval.take() {
            self.lines.push(Line::styled(
                format!("  approval: {}", if allow { "allowed" } else { "denied" }),
                ratatui::style::Style::new().fg(if allow {
                    ratatui::style::Color::Green
                } else {
                    ratatui::style::Color::Red
                }),
            ));
            req.answer(allow);
        }
    }

    fn submit(&mut self) {
        // Enter with the dropdown open completes instead of sending "/" alone.
        if !self.busy && self.input.trim_start().starts_with('/') {
            let items = self.completions();
            if !items.is_empty() {
                let raw_trim = self.input.trim().to_string();
                let stripped = raw_trim.strip_prefix('/').unwrap_or("").to_string();
                let sel = self.complete_sel.min(items.len() - 1);
                let should_complete = if stripped.contains(char::is_whitespace) {
                    // "/cmd <partial>": complete unless the partial exactly
                    // equals the highlighted value.
                    let partial = stripped.split(char::is_whitespace).last().unwrap_or("");
                    !partial.eq_ignore_ascii_case(&items[sel].insert)
                } else {
                    // "/<partial>": complete unless it exactly names a command.
                    !SLASH_COMMANDS
                        .iter()
                        .any(|(n, _)| stripped.eq_ignore_ascii_case(n))
                };
                if should_complete {
                    self.clamp_completion_sel();
                    self.apply_completion();
                    return;
                }
            }
        }
        let raw = self.input.trim().to_string();
        self.input.clear();
        self.complete_sel = 0;
        if raw.is_empty() || self.busy {
            return;
        }
        self.lines.push(Line::styled(
            format!("❯ {raw}"),
            Style::new().add_modifier(ratatui::style::Modifier::BOLD),
        ));
        if let Some(cmd) = raw.strip_prefix('/') {
            self.submitted = Some(self.handle_slash(cmd));
        } else {
            self.busy = true;
            self.submitted = Some(Command::Agent(raw));
        }
    }

    fn handle_slash(&mut self, cmd: &str) -> Command {
        let mut parts = cmd.split_whitespace();
        let name = parts.next().unwrap_or("");
        let rest = parts.collect::<Vec<_>>().join(" ");
        match name {
            "help" => Command::Local(vec![
                Line::from("/help            show this help (Tab lists commands)"),
                Line::from("/new [title]     start a fresh session (current thread is kept)"),
                Line::from("/resume <id>    continue a past session (Tab lists threads)"),
                Line::from("/sessions       list past sessions for this character"),
                Line::from("/rename [title] rename this thread (--auto restores auto-titles)"),
                Line::from("/compact [note] compress history now, keeping note"),
                Line::from("/info           thread details: model, turns, usage"),
                Line::from("/context        context-window breakdown"),
                Line::from("/memory         project memory stats"),
                Line::from("/flush [note]   save this thread's learnings to MEMORY.md"),
                Line::from("/mcp            external tool servers (MCP) status"),
                Line::from("/quit | /q      exit morrow"),
                Line::from("/model <key>    switch model (Tab shows available models)"),
                Line::from("/models         list saved models"),
                Line::from("/switch <name>  switch character (Tab shows names)"),
                Line::from("/mode           toggle approval <-> fully autonomous"),
                Line::from("/agents         list subagents"),
                Line::from(
                    "/rules           show loaded project rule files (MORROW.md, .morrow/rules/)",
                ),
                Line::from("/clear          clear transcript"),
                Line::from("/usage          token totals"),
            ]),
            "new" => Command::NewSession(if rest.is_empty() { None } else { Some(rest) }),
            "rename" => Command::Rename(if rest.is_empty() { None } else { Some(rest) }),
            "compact" => Command::Compact(if rest.is_empty() { None } else { Some(rest) }),
            "flush" => Command::Flush(if rest.is_empty() { None } else { Some(rest) }),
            "info" => Command::Info,
            "context" => Command::Context,
            "mcp" => Command::Mcp,
            "memory" => Command::Local(self.memory_lines()),
            "resume" => {
                if rest.is_empty() {
                    Command::Local(self.sessions_lines())
                } else {
                    let id = rest.split_whitespace().next().unwrap_or("").to_string();
                    Command::Resume(id)
                }
            }
            "sessions" => Command::Local(self.sessions_lines()),
            "quit" | "q" => {
                self.quit = true;
                Command::Local(vec![])
            }
            "clear" => {
                self.lines.clear();
                Command::Local(vec![])
            }
            "usage" => Command::Local(vec![Line::from(format!(
                "tokens: {} in / {} out",
                self.usage.input_tokens, self.usage.output_tokens
            ))]),
            "models" => {
                if self.models.is_empty() {
                    Command::Local(vec![Line::from(
                        "no models yet — define them in ~/.config/morrow/config.toml [models.*]",
                    )])
                } else {
                    let mut out = vec![Line::from("saved models:")];
                    for (key, id, provider) in &self.models {
                        out.push(Line::from(format!("  {key:<16} {id:<28} {provider}")));
                    }
                    Command::Local(out)
                }
            }
            "switch" | "agent" => {
                if rest.is_empty() {
                    Command::Local(vec![Line::from("usage: /switch <character-name>")])
                } else {
                    let name = rest.split_whitespace().next().unwrap_or("").to_string();
                    Command::Switch(name)
                }
            }
            "mode" => Command::ToggleMode,
            "agents" => {
                if self.agents.is_empty() {
                    Command::Local(vec![Line::from("no subagents defined")])
                } else {
                    let mut out = vec![Line::from("subagents:")];
                    for (name, desc) in &self.agents {
                        out.push(Line::from(format!("  {name:<12} {desc}")));
                    }
                    Command::Local(out)
                }
            }
            "rules" => {
                if self.rules.is_empty() {
                    Command::Local(vec![Line::from(
                        "no project rules loaded — add MORROW.md or .morrow/rules/*.md",
                    )])
                } else {
                    let mut out = vec![Line::from("project rules in effect:")];
                    for (path, chars) in &self.rules {
                        out.push(Line::from(format!("  {path}  ({chars} chars)")));
                    }
                    Command::Local(out)
                }
            }
            "model" => {
                if rest.is_empty() {
                    Command::Local(vec![Line::from("usage: /model <alias|id|provider/id>")])
                } else {
                    let msg = format!("switching model to '{rest}'...");
                    self.pending_model_switch = Some(rest);
                    Command::Local(vec![Line::from(msg)])
                }
            }
            other => {
                // User-defined commands (.morrow/commands/<name>.md) run as prompts.
                // Builtins always win the bare name.
                let key = other.to_ascii_lowercase();
                if let Some(custom) = self.custom_commands.iter().find(|c| c.name == key) {
                    self.busy = true;
                    Command::Agent(custom.render(&rest))
                } else {
                    Command::Local(vec![Line::from(format!("unknown command '/{other}'"))])
                }
            }
        }
    }

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Text(t) => {
                // append streaming text to the last assistant buffer
                self.assistant_buf.push_str(&t);
                self.redraw_assistant_tail();
            }
            AppEvent::ToolCall { name, arguments } => {
                self.flush_assistant();
                self.lines.push(Line::styled(
                    format!("⚙ tool: {name} {arguments}"),
                    ratatui::style::Style::new().fg(ratatui::style::Color::Cyan),
                ));
            }
            AppEvent::ToolResult { name, ok, result } => {
                let preview: String = result.chars().take(300).collect();
                self.lines.push(Line::styled(
                    format!(
                        "  {} {name} → {}",
                        if ok { "✔" } else { "✘" },
                        preview.replace('\n', " ↵ ")
                    ),
                    ratatui::style::Style::new().fg(if ok {
                        ratatui::style::Color::DarkGray
                    } else {
                        ratatui::style::Color::Red
                    }),
                ));
            }
            AppEvent::Usage(u) => self.usage = u,
            AppEvent::Notice(n) => self.push_system(&format!("· {n}")),
            AppEvent::SubagentStart(name) => {
                self.flush_assistant();
                self.lines.push(Line::styled(
                    format!("▶ subagent: {name}"),
                    ratatui::style::Style::new().fg(ratatui::style::Color::Magenta),
                ));
            }
            AppEvent::SubagentEnd(name) => self.push_system(&format!("■ subagent done: {name}")),
            AppEvent::Error(e) => {
                self.flush_assistant();
                self.lines.push(Line::styled(
                    format!("error: {e}"),
                    ratatui::style::Style::new().fg(ratatui::style::Color::Red),
                ));
            }
            AppEvent::Done(_) => {
                self.flush_assistant();
                self.finish_turn();
            }
        }
    }

    fn finish_turn(&mut self) {
        self.busy = false;
    }

    fn flush_assistant(&mut self) {
        if self.assistant_shown > 0 {
            for _ in 0..self.assistant_shown {
                self.lines.pop();
            }
            self.assistant_shown = 0;
        }
        if !self.assistant_buf.is_empty() {
            for l in self.assistant_buf.lines() {
                self.lines.push(Line::from(l.to_string()));
            }
            self.assistant_buf.clear();
        }
    }

    /// Re-render the streaming buffer in place: remove exactly the lines we
    /// previously showed for it, then show all of the buffer's lines.
    fn redraw_assistant_tail(&mut self) {
        for _ in 0..self.assistant_shown {
            self.lines.pop();
        }
        let shown: Vec<String> = self.assistant_buf.lines().map(|s| s.to_string()).collect();
        self.assistant_shown = shown.len().max(1);
        if shown.is_empty() {
            self.lines.push(Line::from(""));
        } else {
            for l in shown {
                self.lines.push(Line::from(l));
            }
        }
    }

    // ---- accessors for render -------------------------------------------

    pub fn visible_lines(&self, height: usize) -> (Vec<Line<'static>>, usize) {
        let total = self.lines.len();
        let end = total.saturating_sub(self.scroll);
        let start = end.saturating_sub(height.min(total));
        let slice: Vec<Line<'static>> = self.lines[start..end]
            .iter()
            .map(|l| clone_line_static(l))
            .collect();
        (slice, start)
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn usage(&self) -> Usage {
        self.usage.clone()
    }

    pub fn status_right(&self) -> String {
        format!(
            "{} · {} tokens · {}",
            if self.header_info.is_empty() {
                "morrow"
            } else {
                &self.header_info
            },
            self.usage.total(),
            if self.busy { "working…" } else { "idle" }
        )
    }

    pub fn pending_approval(&self) -> Option<(&String, &String)> {
        self.pending_approval
            .as_ref()
            .map(|r| (&r.tool, &r.arguments))
    }

    pub fn transcript_len(&self) -> usize {
        self.lines.len()
    }
}

/// First 8 chars of a session id for display / prefix matching.
pub fn short_id(id: &str) -> &str {
    &id[..8.min(id.len())]
}

fn clone_line_static(l: &Line<'_>) -> Line<'static> {
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

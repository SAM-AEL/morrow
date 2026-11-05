//! Application state and the reducer that drives it.
//!
//! `App` holds no I/O and no `Runner`: it receives [`AppEvent`]s, mutates, and
//! exposes what the renderer needs. That separation is what makes the interface
//! testable with `TestBackend` and keeps the event loop in `lib.rs` small.
//!
//! The transcript is a `Vec<Block>` rather than pre-rendered lines, so styling
//! happens at draw time against the real terminal width. Baking styles at append
//! time was what made responsive rendering impossible.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use morrow_core::message::{Message, Role, Usage};

use crate::approval::PendingApproval;
use crate::text::{Block, Level};

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

/// Commands that take a free-text argument (get a trailing space on complete).
fn command_takes_arg(name: &str) -> bool {
    matches!(name, "model" | "switch" | "agent" | "resume" | "new")
}

/// One completion row: what gets inserted vs. what is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub insert: String,
    pub display: String,
    pub hint: String,
}

/// What the user submitted: a prompt for the agent or a local command result.
#[derive(Debug)]
pub enum Command {
    Agent(String),
    Local(Vec<Block>),
    /// Switch to another persistent character (`/switch <name>`).
    Switch(String),
    /// Toggle Approval <-> Autonomous for the active character.
    ToggleMode,
    /// Start a fresh session thread (`/new [title]`).
    NewSession(Option<String>),
    /// Resume a past session thread (`/resume <id-prefix>`).
    Resume(String),
    /// Rename the active thread (`/rename [title|--auto]`).
    Rename(Option<String>),
    /// Compress history now (`/compact [note]`).
    Compact(Option<String>),
    /// Save thread learnings to project memory (`/flush [note]`).
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
    /// Turn finished; the runner is handed back for reuse.
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
    blocks: Vec<Block>,
    input: String,
    /// Where the user has scrolled, in block-space from the bottom.
    ///
    /// `0` means pinned to the newest content. While the user has scrolled away
    /// from the bottom, streaming updates must not drag the view back — that is
    /// the whole reason this is explicit state rather than a draw-time default.
    scroll: usize,
    submitted: Option<Command>,
    quit: bool,
    pending_approval: Option<PendingApproval>,
    usage: Usage,
    busy: bool,
    /// In-progress assistant text (streaming deltas accumulate here).
    assistant_buf: String,
    /// `/model <key>` applied when the runner is handed back.
    pending_model_switch: Option<String>,
    /// Header status line (character · model · mode).
    pub header_info: String,
    /// Animation frame for the loader (bumped on a timer while busy).
    tick: u64,
    /// Saved models for `/models` — (key, id, provider).
    models: Vec<(String, String, String)>,
    /// Subagents for `/agents` — (name, description).
    agents: Vec<(String, String)>,
    /// Known character names for `/switch` completion.
    characters: Vec<String>,
    /// Loaded project rule files for `/rules` — (display path, chars).
    rules: Vec<(String, usize)>,
    /// User-defined slash commands (.morrow/commands/*.md).
    custom_commands: Vec<morrow_core::CustomCommand>,
    /// Project root for project-scoped reads (/memory).
    project_root: std::path::PathBuf,
    /// Past threads for `/resume` completion + `/sessions` — (id, title, updated, msgs).
    sessions: Vec<(String, String, String, usize)>,
    /// Fresh-launch banner shown in the welcome hero.
    session_banner: Option<String>,
    /// Selected index into the live completion list.
    complete_sel: usize,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
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
        }
    }

    // ---- injected context ------------------------------------------------

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

    // ---- transcript ------------------------------------------------------

    /// Replace the visible transcript with a resumed thread's messages.
    pub fn replay_transcript(&mut self, messages: &[Message]) {
        self.blocks.clear();
        self.assistant_buf.clear();
        self.scroll = 0;
        for m in messages {
            let text = m.plain_text();
            if text.trim().is_empty() {
                continue;
            }
            match m.role {
                Role::User => self.blocks.push(Block::Prompt { text }),
                Role::Assistant => self.blocks.push(Block::Assistant { text }),
                _ => self.blocks.push(Block::System {
                    text: text.lines().next().unwrap_or("").to_string(),
                    level: Level::Info,
                }),
            }
        }
    }

    /// Rendered `/memory` summary.
    pub fn memory_lines(&self) -> Vec<Block> {
        match morrow_core::memory_stats(&self.project_root) {
            None => vec![info_block(
                "no project memory yet — /flush saves this thread, remember(scope=project) too",
            )],
            Some((path, entries, bytes)) => vec![
                info_block(&format!("project memory: {path}")),
                info_block(&format!(
                    "  {entries} entries · {bytes} bytes · injected every turn"
                )),
            ],
        }
    }

    /// Rendered `/sessions` list (also used for a bare `/resume`).
    pub fn sessions_lines(&self) -> Vec<Block> {
        if self.sessions.is_empty() {
            return vec![info_block(
                "no past sessions here yet — chat away, /new starts a thread",
            )];
        }
        let mut out = vec![info_block("past sessions (/resume <id>):")];
        for (id, title, updated, msgs) in &self.sessions {
            out.push(info_block(&format!(
                "  {}  {title}  · {updated} · {msgs} msgs",
                short_id(id)
            )));
        }
        out
    }

    // ---- completion ------------------------------------------------------

    /// Live completion items for the current input.
    pub fn completions(&self) -> Vec<Completion> {
        let input = self.input.trim_start();
        let Some(stripped) = input.strip_prefix('/') else {
            return Vec::new();
        };
        // `/model <partial>` / `/switch <partial>` / `/resume <partial>` — values.
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
        // `/<partial>` — command names (builtins + custom commands).
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

    /// Currently highlighted completion index.
    pub fn completion_sel(&self) -> usize {
        self.complete_sel
    }

    fn clamp_completion_sel(&mut self) {
        let n = self.completions().len();
        self.complete_sel = if n == 0 {
            0
        } else {
            self.complete_sel.min(n - 1)
        };
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
            self.input = format!("/{picked}");
            if command_takes_arg(&picked) {
                self.input.push(' ');
            }
        } else {
            // Value completion: replace the text after the last whitespace.
            let pos = self
                .input
                .rfind(char::is_whitespace)
                .map(|i| i + 1)
                .unwrap_or(1);
            self.input.truncate(pos);
            self.input.push_str(&picked);
        }
        self.complete_sel = 0;
    }

    // ---- animation -------------------------------------------------------

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

    /// Called when a turn's runner is reclaimed: flush text and clear busy.
    pub fn on_turn_done(&mut self) {
        self.flush_assistant();
        self.busy = false;
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn take_submitted(&mut self) -> Option<Command> {
        self.submitted.take()
    }

    // ---- block append ----------------------------------------------------

    pub fn push_blocks(&mut self, blocks: impl IntoIterator<Item = Block>) {
        self.blocks.extend(blocks);
    }

    /// Append a dim informational block.
    pub fn push_system(&mut self, text: &str) {
        self.blocks.push(info_block(text));
    }

    /// Append a dim informational block without the caller's formatting.
    pub fn push_notice(&mut self, text: &str) {
        self.blocks.push(Block::System {
            text: text.to_string(),
            level: Level::Notice,
        });
    }

    /// Append an error block.
    pub fn push_error(&mut self, text: &str) {
        self.blocks.push(Block::System {
            text: text.to_string(),
            level: Level::Error,
        });
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

    // ---- input -----------------------------------------------------------

    pub fn on_key(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => self.on_key_event(key),
            Event::Mouse(m) => match m.kind {
                crossterm::event::MouseEventKind::ScrollUp => self.scroll_by(3),
                crossterm::event::MouseEventKind::ScrollDown => self.scroll_by(-3),
                _ => {}
            },
            _ => {}
        }
    }

    fn on_key_event(&mut self, key: KeyEvent) {
        if key.kind == crossterm::event::KeyEventKind::Release {
            return;
        }
        if self.pending_approval.is_some() {
            // Modal is active: it owns the keyboard.
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
                if !self.move_selection(-1) {
                    self.scroll_by(3);
                }
            }
            KeyCode::Down => {
                if !self.move_selection(1) {
                    self.scroll_by(-3);
                }
            }
            KeyCode::Tab => {
                self.clamp_completion_sel();
                self.apply_completion();
            }
            KeyCode::Esc => self.complete_sel = 0,
            KeyCode::Home => self.scroll_to_end(),
            KeyCode::PageUp => self.scroll_by(20),
            KeyCode::PageDown => self.scroll_by(-20),
            KeyCode::Char(c) => {
                self.input.push(c);
                self.clamp_completion_sel();
            }
            _ => {}
        }
    }

    /// Move the completion selection. Returns false when no list is open, so the
    /// caller can fall through to scrolling the transcript.
    fn move_selection(&mut self, delta: isize) -> bool {
        let n = self.completions().len();
        if n == 0 {
            return false;
        }
        if delta < 0 {
            self.complete_sel = self.complete_sel.checked_sub(1).unwrap_or(n - 1);
        } else {
            self.complete_sel = (self.complete_sel + 1) % n;
        }
        true
    }

    fn resolve_approval(&mut self, allow: bool) {
        if let Some(req) = self.pending_approval.take() {
            self.blocks.push(Block::System {
                text: format!("approval: {}", if allow { "allowed" } else { "denied" }),
                level: if allow { Level::Info } else { Level::Error },
            });
            req.answer(allow);
        }
    }

    fn submit(&mut self) {
        // Enter with the dropdown open completes instead of sending a bare "/".
        if !self.busy && self.input.trim_start().starts_with('/') {
            let items = self.completions();
            if !items.is_empty() {
                let raw_trim = self.input.trim().to_string();
                let stripped = raw_trim.strip_prefix('/').unwrap_or("").to_string();
                let sel = self.complete_sel.min(items.len() - 1);
                let should_complete = if stripped.contains(char::is_whitespace) {
                    // `/cmd <partial>`: complete unless the partial exactly
                    // equals the highlighted value.
                    let partial = stripped
                        .split(char::is_whitespace)
                        .next_back()
                        .unwrap_or("");
                    !partial.eq_ignore_ascii_case(&items[sel].insert)
                } else {
                    // `/<partial>`: complete unless it exactly names a command.
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
        // A new prompt pulls the reader back to the live edge.
        self.scroll = 0;
        if let Some(cmd) = raw.strip_prefix('/') {
            self.submitted = Some(self.handle_slash(cmd));
        } else {
            self.blocks.push(Block::Prompt { text: raw.clone() });
            self.busy = true;
            self.submitted = Some(Command::Agent(raw));
        }
    }

    fn handle_slash(&mut self, cmd: &str) -> Command {
        let mut parts = cmd.split_whitespace();
        let name = parts.next().unwrap_or("");
        let rest = parts.collect::<Vec<_>>().join(" ");
        match name {
            "help" => Command::Local(help_blocks()),
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
                Command::Local(Vec::new())
            }
            "clear" => {
                self.blocks.clear();
                self.scroll = 0;
                Command::Local(Vec::new())
            }
            "usage" => Command::Local(vec![info_block(&format!(
                "tokens: {} in / {} out",
                self.usage.input_tokens, self.usage.output_tokens
            ))]),
            "models" => {
                if self.models.is_empty() {
                    Command::Local(vec![info_block(
                        "no models yet — define them in ~/.config/morrow/config.toml [models.*]",
                    )])
                } else {
                    let mut out = vec![info_block("saved models:")];
                    for (key, id, provider) in &self.models {
                        out.push(info_block(&format!("  {key}  ·  {id}  ·  {provider}")));
                    }
                    Command::Local(out)
                }
            }
            "switch" | "agent" => {
                if rest.is_empty() {
                    Command::Local(vec![info_block("usage: /switch <character-name>")])
                } else {
                    let name = rest.split_whitespace().next().unwrap_or("").to_string();
                    Command::Switch(name)
                }
            }
            "mode" => Command::ToggleMode,
            "agents" => {
                if self.agents.is_empty() {
                    Command::Local(vec![info_block("no subagents defined")])
                } else {
                    let mut out = vec![info_block("subagents:")];
                    for (name, desc) in &self.agents {
                        out.push(info_block(&format!("  {name}  ·  {desc}")));
                    }
                    Command::Local(out)
                }
            }
            "rules" => {
                if self.rules.is_empty() {
                    Command::Local(vec![info_block(
                        "no project rules loaded — add MORROW.md or .morrow/rules/*.md",
                    )])
                } else {
                    let mut out = vec![info_block("project rules in effect:")];
                    for (path, chars) in &self.rules {
                        out.push(info_block(&format!("  {path}  ({chars} chars)")));
                    }
                    Command::Local(out)
                }
            }
            "model" => {
                if rest.is_empty() {
                    Command::Local(vec![info_block("usage: /model <alias|id|provider/id>")])
                } else {
                    let msg = format!("switching model to '{rest}'...");
                    self.pending_model_switch = Some(rest);
                    Command::Local(vec![info_block(&msg)])
                }
            }
            other => {
                // User-defined commands (.morrow/commands/<name>.md) run as
                // prompts. Builtins always win the bare name.
                let key = other.to_ascii_lowercase();
                if let Some(custom) = self.custom_commands.iter().find(|c| c.name == key) {
                    self.busy = true;
                    Command::Agent(custom.render(&rest))
                } else {
                    Command::Local(vec![info_block(&format!("unknown command '/{other}'"))])
                }
            }
        }
    }

    // ---- events ----------------------------------------------------------

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Text(t) => {
                self.assistant_buf.push_str(&t);
            }
            AppEvent::ToolCall { name, arguments } => {
                self.flush_assistant();
                self.blocks.push(Block::ToolCall { name, arguments });
            }
            AppEvent::ToolResult { name, ok, result } => {
                self.flush_assistant();
                self.blocks.push(Block::ToolResult { name, ok, result });
            }
            AppEvent::Usage(u) => self.usage = u,
            AppEvent::Notice(n) => self.blocks.push(Block::System {
                text: n,
                level: Level::Notice,
            }),
            AppEvent::SubagentStart(name) => {
                self.flush_assistant();
                self.blocks.push(Block::Subagent { name, done: false });
            }
            AppEvent::SubagentEnd(name) => self.blocks.push(Block::Subagent { name, done: true }),
            AppEvent::Error(e) => {
                self.flush_assistant();
                self.blocks.push(Block::System {
                    text: format!("error: {e}"),
                    level: Level::Error,
                });
            }
            AppEvent::Done(_) => self.on_turn_done(),
        }
    }

    /// Commit the streaming buffer as a real assistant block.
    ///
    /// Streaming text is held in `assistant_buf` rather than pushed per delta:
    /// appending a block per token would make the transcript grow unbounded and
    /// force a re-wrap on every event.
    fn flush_assistant(&mut self) {
        if !self.assistant_buf.is_empty() {
            let text = std::mem::take(&mut self.assistant_buf);
            self.blocks.push(Block::Assistant { text });
        }
    }

    /// A cheap signature of everything visible. The loop redraws only when this
    /// changes, which is what keeps an idle terminal at zero CPU.
    pub fn frame_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.blocks.len().hash(&mut h);
        self.assistant_buf.len().hash(&mut h);
        self.scroll.hash(&mut h);
        self.tick.hash(&mut h);
        self.busy.hash(&mut h);
        self.input.hash(&mut h);
        self.complete_sel.hash(&mut h);
        self.usage.total().hash(&mut h);
        self.header_info.hash(&mut h);
        self.pending_approval.is_some().hash(&mut h);
        h.finish()
    }

    // ---- scroll ----------------------------------------------------------

    /// Scroll by `delta` blocks. Negative moves toward the newest content.
    fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll();
        if delta < 0 {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.scroll = self.scroll.saturating_add(delta as usize);
        }
        self.scroll = self.scroll.min(max);
    }

    fn scroll_to_end(&mut self) {
        self.scroll = 0;
    }

    /// How many blocks the reader may scroll back through.
    fn max_scroll(&self) -> usize {
        // Without knowing the rendered height this is approximate; the renderer
        // clamps against real geometry. Bounding by block count keeps the
        // reducer from accumulating an absurd offset.
        self.blocks.len().saturating_sub(1)
    }

    // ---- accessors for the renderer ---------------------------------------

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Streaming assistant text not yet committed to a block.
    pub fn assistant_buf(&self) -> &str {
        &self.assistant_buf
    }

    /// How many blocks the reader has scrolled back from the newest content.
    pub fn scroll(&self) -> usize {
        self.scroll
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
        self.blocks.len()
    }
}

/// A dim informational block.
pub fn info_block(text: &str) -> Block {
    Block::System {
        text: text.to_string(),
        level: Level::Info,
    }
}

/// The `/help` listing, generated from the single source of truth so the two
/// can never drift.
fn help_blocks() -> Vec<Block> {
    let mut out = vec![info_block("commands:")];
    for (name, desc) in SLASH_COMMANDS {
        out.push(info_block(&format!("  /{name:<12} {desc}")));
    }
    out
}

/// First few characters of a session id, for display and prefix matching.
///
/// Character-boundary safe: a non-ASCII id would panic a byte slice.
pub fn short_id(id: &str) -> &str {
    match id.char_indices().nth(8) {
        Some((i, _)) => &id[..i],
        None => id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(Event::Key(key(KeyCode::Char(c))));
        }
    }

    #[test]
    fn typing_a_prompt_sends_it_and_echoes_it() {
        let mut app = App::new();
        type_text(&mut app, "hello");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        match app.take_submitted() {
            Some(Command::Agent(p)) => assert_eq!(p, "hello"),
            other => panic!("expected Agent, got {other:?}"),
        }
        assert!(matches!(app.blocks().first(), Some(Block::Prompt { .. })));
        assert!(app.is_busy());
    }

    #[test]
    fn enter_completes_a_partial_command_instead_of_sending_it() {
        let mut app = App::new();
        type_text(&mut app, "/mod");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        assert_eq!(app.input(), "/model ", "should complete, not submit");
        assert!(app.take_submitted().is_none());
    }

    #[test]
    fn enter_sends_an_exact_command_name() {
        let mut app = App::new();
        type_text(&mut app, "/models");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        assert!(matches!(app.take_submitted(), Some(Command::Local(_))));
    }

    #[test]
    fn tab_completes_a_command_with_an_argument() {
        let mut app = App::new();
        type_text(&mut app, "/mod");
        app.on_key(Event::Key(key(KeyCode::Tab)));
        assert_eq!(app.input(), "/model ");
    }

    #[test]
    fn tab_completes_a_model_value() {
        let mut app = App::new();
        app.set_models(vec![("fast".into(), "grok-fast".into(), "grok".into())]);
        type_text(&mut app, "/model fa");
        app.on_key(Event::Key(key(KeyCode::Tab)));
        assert_eq!(app.input(), "/model fast");
    }

    #[test]
    fn unknown_slash_command_reports_instead_of_sending() {
        let mut app = App::new();
        type_text(&mut app, "/nope");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        match app.take_submitted() {
            Some(Command::Local(blocks)) => {
                let text = blocks
                    .iter()
                    .map(|b| match b {
                        Block::System { text, .. } => text.clone(),
                        _ => String::new(),
                    })
                    .collect::<String>();
                assert!(text.contains("unknown command"), "got {text:?}");
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn streaming_text_commits_as_one_block_not_many() {
        let mut app = App::new();
        for chunk in ["Hel", "lo ", "wor", "ld"] {
            app.apply(AppEvent::Text(chunk.to_string()));
            assert!(
                !app.assistant_buf().is_empty(),
                "buffer should hold the delta"
            );
        }
        app.on_turn_done();
        assert_eq!(app.transcript_len(), 1, "delta-per-block regression");
        match &app.blocks()[0] {
            Block::Assistant { text } => assert_eq!(text, "Hello world"),
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_call_flushes_pending_assistant_text_first() {
        let mut app = App::new();
        app.apply(AppEvent::Text("thinking".into()));
        // The streamed text must be committed before the tool call is recorded,
        // or the transcript would show a tool call with no reasoning above it.
        app.apply(AppEvent::ToolCall {
            name: "bash".into(),
            arguments: "{}".into(),
        });
        assert_eq!(app.transcript_len(), 2, "assistant text + tool call");
        assert!(matches!(app.blocks()[0], Block::Assistant { .. }));
        assert!(matches!(app.blocks()[1], Block::ToolCall { .. }));

        app.apply(AppEvent::ToolResult {
            name: "bash".into(),
            ok: true,
            result: "ok".into(),
        });
        assert_eq!(app.transcript_len(), 3);
        assert!(matches!(app.blocks()[2], Block::ToolResult { .. }));
    }

    #[test]
    fn scrolled_up_state_survives_streaming() {
        let mut app = App::new();
        for i in 0..10 {
            app.push_system(&format!("line {i}"));
        }
        app.on_key(Event::Key(key(KeyCode::PageUp)));
        assert!(
            app.scroll() > 0,
            "should have scrolled away from the bottom"
        );
        let before = app.scroll();
        app.apply(AppEvent::Text("new content".into()));
        app.on_turn_done();
        assert_eq!(
            app.scroll(),
            before,
            "streaming must not drag the reader back to the bottom"
        );
    }

    #[test]
    fn home_returns_to_the_live_edge() {
        let mut app = App::new();
        for i in 0..10 {
            app.push_system(&format!("line {i}"));
        }
        app.on_key(Event::Key(key(KeyCode::PageUp)));
        app.on_key(Event::Key(key(KeyCode::Home)));
        assert_eq!(app.scroll(), 0);
    }

    #[test]
    fn scroll_cannot_go_negative_or_past_the_start() {
        let mut app = App::new();
        for i in 0..3 {
            app.push_system(&format!("line {i}"));
        }
        for _ in 0..20 {
            app.on_key(Event::Key(key(KeyCode::PageDown)));
        }
        assert_eq!(app.scroll(), 0, "scrolled past the newest content");
    }

    #[test]
    fn submitting_after_scrolling_returns_to_the_bottom() {
        let mut app = App::new();
        for i in 0..10 {
            app.push_system(&format!("line {i}"));
        }
        app.on_key(Event::Key(key(KeyCode::PageUp)));
        type_text(&mut app, "next");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        assert_eq!(app.scroll(), 0);
    }

    #[test]
    fn clear_empties_the_transcript() {
        let mut app = App::new();
        app.push_system("something");
        type_text(&mut app, "/clear");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        assert_eq!(app.transcript_len(), 0);
    }

    #[test]
    fn busy_state_blocks_a_second_submission() {
        let mut app = App::new();
        type_text(&mut app, "one");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        app.take_submitted();
        type_text(&mut app, "two");
        app.on_key(Event::Key(key(KeyCode::Enter)));
        assert!(
            app.take_submitted().is_none(),
            "second prompt should be dropped"
        );
    }

    #[test]
    fn frame_signature_changes_only_on_visible_change() {
        let mut app = App::new();
        let a = app.frame_signature();
        assert_eq!(a, app.frame_signature(), "stable when idle");
        app.apply(AppEvent::Text("hi".into()));
        assert_ne!(a, app.frame_signature(), "streaming must dirty the frame");
    }

    #[test]
    fn short_id_is_multibyte_safe() {
        assert_eq!(short_id("0123456789abcdef"), "01234567");
        assert_eq!(short_id("short"), "short");
        // A non-ASCII id must not panic on a byte boundary.
        let _ = short_id("日本語日本語日本語");
        assert_eq!(short_id("日本語日本語日本語").chars().count(), 8);
    }

    #[test]
    fn completions_offer_models_characters_and_threads() {
        let mut app = App::new();
        app.set_models(vec![("fast".into(), "m".into(), "p".into())]);
        app.set_characters(vec!["nova".into(), "atlas".into()]);
        app.set_sessions(vec![(
            "abcdef1234".into(),
            "fix the bug".into(),
            "10-04".into(),
            4,
        )]);
        type_text(&mut app, "/model ");
        assert_eq!(app.completions().len(), 1);
        type_text(&mut app, "");
        let _ = app.completions();
    }

    #[test]
    fn replay_transcript_rebuilds_blocks_from_messages() {
        let mut app = App::new();
        let msgs = vec![
            Message::text(Role::User, "do it"),
            Message::text(Role::Assistant, "done"),
            Message::text(Role::Tool, "tool noise"),
        ];
        app.replay_transcript(&msgs);
        assert_eq!(app.transcript_len(), 3);
        assert!(matches!(app.blocks()[0], Block::Prompt { .. }));
        assert!(matches!(app.blocks()[1], Block::Assistant { .. }));
        assert!(matches!(app.blocks()[2], Block::System { .. }));
    }
}

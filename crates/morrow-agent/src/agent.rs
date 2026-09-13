use std::path::{Path, PathBuf};
use std::sync::Arc;

use morrow_core::config::{Config, ModelConfig, ProviderConfig};
use morrow_core::message::{Content, Message, Role, ToolCall, Usage};
use morrow_core::{
    Character, CharacterStore, Error, Mode, PermissionMode, ProjectSessions, Result, Session,
};
use morrow_provider::{CompletionRequest, LlmProvider, StreamEvent};
use morrow_tools::{Registry, ToolContext};

use crate::approval::ApprovalHandler;
use crate::prompt::base_system_prompt;
use crate::subagent;

/// Events streamed out of an agent turn for UIs to render.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        ok: bool,
        result: String,
    },
    /// Cumulative usage after each completion.
    Usage(Usage),
    /// Informational notices (compaction, denials, retries).
    Notice(String),
    SubagentStart {
        name: String,
    },
    SubagentEnd {
        name: String,
    },
    /// The active character changed (name).
    CharacterSwitched(String),
    /// Operating mode changed; bool = autonomous.
    ModeChanged(bool),
}

pub struct Runner {
    pub config: Arc<Config>,
    pub registry: Registry,
    ctx: ToolContext,
    cwd: PathBuf,
    pub character: Character,
    store: CharacterStore,
    approval: Arc<dyn ApprovalHandler>,
    pub total_usage: Usage,
    /// Subagents: restricted tool names (None = all).
    allowlist: Option<Vec<String>>,
    /// Subagents: replaces the character persona.
    persona_prompt: Option<String>,
    /// Subagents: explicit temperature.
    temperature_override: Option<f64>,
    /// Subagents cannot spawn further tasks.
    is_subagent: bool,
    /// Test seam: bypass config-based provider construction.
    provider_override: Option<Arc<dyn LlmProvider>>,
    /// Project-local threads (.morrow/sessions); None = global character chat only.
    sessions: Option<ProjectSessions>,
    /// Active thread id within `sessions` for the current character.
    session_id: Option<String>,
    /// Connected MCP servers (spawned once per Runner, on first turn).
    mcp_clients: Vec<morrow_tools::McpClient>,
    mcp_tool_counts: Vec<(String, usize)>,
    mcp_ready: bool,
}

impl Runner {
    /// Primary constructor: attach (or create) a persistent character
    /// living in the default store (~/.local/share/morrow/agents/<name>.json).
    pub fn new(
        config: Arc<Config>,
        cwd: PathBuf,
        model_key: Option<String>,
        approval: Arc<dyn ApprovalHandler>,
        character: Character,
    ) -> Self {
        Self::with_store(
            CharacterStore::new(),
            config,
            cwd,
            model_key,
            approval,
            character,
        )
    }

    /// Same, but backed by an explicit store (custom directory / tests).
    pub fn with_store(
        store: CharacterStore,
        config: Arc<Config>,
        cwd: PathBuf,
        model_key: Option<String>,
        approval: Arc<dyn ApprovalHandler>,
        mut character: Character,
    ) -> Self {
        let ctx = ctx_for(&character, &store, &config, cwd.clone());
        let model_key = model_key.or_else(|| character.model.clone());
        if let Some(k) = &model_key {
            character.model = Some(k.clone());
        }
        Self {
            config,
            registry: morrow_tools::builtin_registry(),
            ctx,
            cwd,
            character,
            store,
            approval,
            total_usage: Usage::default(),
            allowlist: None,
            persona_prompt: None,
            temperature_override: None,
            is_subagent: false,
            provider_override: None,
            sessions: None,
            session_id: None,
            mcp_clients: Vec::new(),
            mcp_tool_counts: Vec::new(),
            mcp_ready: false,
        }
    }

    /// Test-only: force a specific provider implementation.
    pub fn set_provider(&mut self, p: Arc<dyn LlmProvider>) {
        self.provider_override = Some(p);
    }

    /// Persist the character file (chat history + memories + integrations).
    /// Also snapshots the project thread when sessions are attached (best effort).
    pub fn save_character(&mut self) -> Result<()> {
        if !self.is_subagent {
            self.store.save(&mut self.character)?;
            let _ = self.snapshot_session();
        }
        Ok(())
    }

    /// Attach project-local threads (.morrow/sessions under `project_root`).
    /// Adopts the active pointer when its file still exists.
    pub fn attach_project_sessions(&mut self, project_root: PathBuf) {
        if self.is_subagent {
            return;
        }
        let ps = ProjectSessions::new(&project_root);
        let active = ps.active_id(&self.character.name).filter(|id| {
            ps.load_exact(&self.character.name, id)
                .ok()
                .flatten()
                .is_some()
        });
        self.session_id = active;
        self.sessions = Some(ps);
    }

    fn sessions(&self) -> Option<&ProjectSessions> {
        self.sessions.as_ref().filter(|_| !self.is_subagent)
    }

    /// Write the current conversation into the active thread file (creating one
    /// on demand). Returns the session id, or None when sessions are detached.
    pub fn snapshot_session(&mut self) -> Result<Option<String>> {
        let Some(ps) = self.sessions() else {
            return Ok(None);
        };
        let name = self.character.name.clone();
        let id = match &self.session_id {
            Some(id) => id.clone(),
            None => uuid::Uuid::new_v4().simple().to_string(),
        };
        let mut s = match ps.load_exact(&name, &id)? {
            Some(mut existing) => {
                existing.messages = self.character.messages.clone();
                existing.model = self.character.model.clone();
                existing.updated_at = chrono::Utc::now();
                if !existing.title_manual && existing.title == "new session" {
                    existing.title = derive_title(&existing.messages);
                }
                existing
            }
            None => {
                let mut fresh = Session::new(self.character.model.clone());
                fresh.id = id.clone();
                fresh.character = name.clone();
                for m in &self.character.messages {
                    fresh.push(m.clone());
                }
                fresh
            }
        };
        // Keep the stored character tag honest even for legacy files.
        s.character = name.clone();
        ps.save(&s)?;
        ps.set_active_id(&name, &id)?;
        self.session_id = Some(id.clone());
        Ok(Some(id))
    }

    /// `/new`: preserve the current thread, then start blank.
    pub fn start_new_session(&mut self, title: Option<String>) -> Result<Session> {
        let Some(ps) = self.sessions.clone().filter(|_| !self.is_subagent) else {
            return Err(Error::Other("project sessions are not attached".into()));
        };
        if !self.character.messages.is_empty() {
            self.snapshot_session()?;
        }
        let mut s = Session::new(self.character.model.clone());
        s.character = self.character.name.clone();
        if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
            s.title = t.trim().chars().take(60).collect();
        }
        self.character.messages.clear();
        // Point the autosnapshot at the new file before saving.
        self.session_id = Some(s.id.clone());
        self.save_character()?;
        ps.save(&s)?;
        ps.set_active_id(&s.character, &s.id)?;
        Ok(s)
    }

    /// Headless `--session-id`: start a brand-new thread under a client id.
    /// Errors when the id already has a thread (anti-overwrite: use --resume).
    pub fn begin_session_with_id(&mut self, id: &str) -> Result<Session> {
        let Some(ps) = self.sessions.clone().filter(|_| !self.is_subagent) else {
            return Err(Error::Other("project sessions are not attached".into()));
        };
        let id = id.trim();
        if id.len() < 8 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(Error::Other(
                "session id must be 8+ hex/- characters".into(),
            ));
        }
        let name = self.character.name.clone();
        if ps.load_exact(&name, id)?.is_some() {
            return Err(Error::Other(format!(
                "thread {id} already exists — use --resume to continue it"
            )));
        }
        if !self.character.messages.is_empty() {
            self.snapshot_session()?;
        }
        let mut s = Session::new(self.character.model.clone());
        s.id = id.to_string();
        s.character = name.clone();
        self.character.messages.clear();
        self.session_id = Some(s.id.clone());
        self.save_character()?;
        ps.save(&s)?;
        ps.set_active_id(&name, &s.id)?;
        Ok(s)
    }

    /// `/resume <id-prefix>`: preserve the current thread, load the chosen one.
    pub fn resume_session(&mut self, query: &str) -> Result<Session> {
        let Some(ps) = self.sessions.clone().filter(|_| !self.is_subagent) else {
            return Err(Error::Other("project sessions are not attached".into()));
        };
        let name = self.character.name.clone();
        let hits = ps.resolve(&name, query.trim())?;
        let s = match hits.len() {
            0 => {
                return Err(Error::Other(format!(
                    "no session '{query}' for {name} — see /sessions"
                )))
            }
            1 => hits.into_iter().next().unwrap(),
            _ => {
                let opts: Vec<String> = hits
                    .iter()
                    .map(|h| format!("{} ({})", &h.id[..8.min(h.id.len())], h.title))
                    .collect();
                return Err(Error::Other(format!(
                    "ambiguous '{}': {}",
                    query,
                    opts.join(", ")
                )));
            }
        };
        if !self.character.messages.is_empty() {
            self.snapshot_session()?;
        }
        self.character.messages = s.messages.clone();
        // Point the autosnapshot at the resumed file before saving.
        self.session_id = Some(s.id.clone());
        self.save_character()?;
        ps.set_active_id(&name, &s.id)?;
        Ok(s)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let Some(ps) = self.sessions() else {
            return Ok(Vec::new());
        };
        ps.list(&self.character.name)
    }

    /// Project rule files loaded for the current directory (for `/rules`).
    pub fn project_rules(&self) -> morrow_core::ProjectRules {
        morrow_core::ProjectRules::load(&self.cwd)
    }

    /// Spawn configured MCP servers once per Runner; tools join the registry.
    /// Failures are notices, never fatal — the turn continues without them.
    async fn ensure_mcp(&mut self, on_event: &mut (dyn FnMut(AgentEvent) + Send)) {
        if self.mcp_ready || self.is_subagent {
            return;
        }
        self.mcp_ready = true;
        if self.config.mcp_servers.is_empty() {
            return;
        }
        for (name, cfg) in &self.config.mcp_servers.clone() {
            if !cfg.enabled {
                continue;
            }
            match morrow_tools::McpClient::spawn(name, cfg, &self.cwd).await {
                Ok((client, tools)) => {
                    let n = tools.len();
                    for def in tools {
                        self.registry
                            .add(morrow_tools::McpTool::new(client.clone(), def));
                    }
                    // Subagent allowlists are name-based and unaffected.
                    self.mcp_tool_counts.push((name.clone(), n));
                    self.mcp_clients.push(client);
                    on_event(AgentEvent::Notice(format!("mcp: {n} tools from '{name}'")));
                }
                Err(e) => on_event(AgentEvent::Notice(format!("mcp '{name}' unavailable: {e}"))),
            }
        }
    }

    /// (server, tool-count) for connected MCP servers — for `/mcp`.
    pub fn mcp_status(&self) -> Vec<(String, usize)> {
        self.mcp_tool_counts.clone()
    }

    pub fn active_session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// `/switch <name>`: persist current character, load/create another.
    /// Each character keeps its own memories, integrations and chat history.
    pub fn switch_character(&mut self, name: &str) -> Result<()> {
        if self.is_subagent || name.eq_ignore_ascii_case(self.character.name.as_str()) {
            return Ok(());
        }
        self.save_character()?;
        self.character = self.store.load_or_create(name)?;
        self.ctx = ctx_for(&self.character, &self.store, &self.config, self.cwd.clone());
        // Threads are per character: adopt the new character's active pointer.
        if let Some(ps) = self.sessions() {
            let next = ps.active_id(&self.character.name).filter(|id| {
                ps.load_exact(&self.character.name, id)
                    .ok()
                    .flatten()
                    .is_some()
            });
            self.session_id = next;
        }
        Ok(())
    }

    /// `/mode`: toggle Approval <-> Autonomous (persisted per character).
    pub fn toggle_mode(&mut self) -> Mode {
        self.character.mode = match self.character.mode {
            Mode::Approval => Mode::Autonomous,
            Mode::Autonomous => Mode::Approval,
        };
        let _ = self.save_character();
        self.character.mode
    }

    pub fn mode(&self) -> Mode {
        self.character.mode
    }

    pub fn set_model(&mut self, key: impl Into<String>) {
        let key = key.into();
        self.character.model = Some(key.clone());
        let _ = self.save_character();
    }

    pub fn model_key(&self) -> Option<&str> {
        self.character
            .model
            .as_deref()
            .or(self.config.default_model.as_deref())
    }

    pub fn todos(&self) -> Vec<morrow_tools::TodoItem> {
        self.ctx.todos.lock().unwrap().clone()
    }

    fn resolve_model(&self) -> Result<(ModelConfig, ProviderConfig)> {
        let key = self.model_key().ok_or_else(|| {
            Error::Config(
                "no model selected. Add a [providers.*] and [models.*] block to \
                 ~/.config/morrow/config.toml or pass --model"
                    .into(),
            )
        })?;
        let (_, model) = self.config.find_model(key).ok_or_else(|| {
            Error::Config(format!(
                "unknown model '{key}'. Known models: {}",
                self.config
                    .models
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let provider_cfg = self.config.providers.get(&model.provider).ok_or_else(|| {
            Error::Config(format!(
                "model '{}' references unknown provider '{}'",
                model.id, model.provider
            ))
        })?;
        Ok((model, provider_cfg.clone()))
    }

    /// Run one user turn end-to-end. History lives in the character's own file.
    pub async fn chat(
        &mut self,
        text: &str,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<String> {
        self.ensure_mcp(on_event).await;
        self.character
            .messages
            .push(Message::text(Role::User, text));
        const MAX_ITERATIONS: usize = 64;
        let mut last_text = String::new();

        for _ in 0..MAX_ITERATIONS {
            let (assistant_msg, usage) = self.complete_once(on_event).await?;
            self.total_usage.input_tokens += usage.input_tokens;
            self.total_usage.output_tokens += usage.output_tokens;
            on_event(AgentEvent::Usage(self.total_usage.clone()));

            let calls: Vec<ToolCall> = assistant_msg
                .parts
                .iter()
                .filter_map(|p| match p {
                    Content::ToolUse(c) => Some(c.clone()),
                    _ => None,
                })
                .flatten()
                .collect();
            last_text = assistant_msg.plain_text();

            if calls.is_empty() {
                self.character.messages.push(assistant_msg);
                self.save_character().ok();
                self.maybe_compact(on_event).await?;
                return Ok(last_text);
            }

            self.character.messages.push(assistant_msg);
            for call in calls {
                let result = self.execute_call(&call, on_event).await;
                let (content, is_error) = match result {
                    Ok(s) => (s, false),
                    Err(e) => (format!("error: {e}"), true),
                };
                on_event(AgentEvent::ToolResult {
                    name: call.name.clone(),
                    ok: !is_error,
                    result: content.clone(),
                });
                self.character
                    .messages
                    .push(Message::tool_result(&call.id, content, is_error));
            }
            // checkpoint the character after every tool round (crash-safe)
            self.save_character().ok();
            self.maybe_compact(on_event).await?;
        }
        self.save_character().ok();
        Ok(last_text)
    }

    /// One streaming completion + tool-call accumulation, with retry/backoff.
    async fn complete_once(
        &mut self,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<(Message, Usage)> {
        let (model, provider_cfg) = self.resolve_model()?;
        let api_key = self.config.resolve_api_key(&provider_cfg);
        let provider: Arc<dyn LlmProvider> = match &self.provider_override {
            Some(p) => p.clone(),
            None => Arc::from(morrow_provider::create(&provider_cfg, api_key)?),
        };

        let system = match &self.persona_prompt {
            Some(persona) => base_system_prompt(&self.cwd, Some(persona)),
            None => self.character.system_prompt(&self.cwd),
        };

        let mut tools = self.registry.specs(self.allowlist.as_deref());
        if !self.is_subagent {
            tools.push(subagent::task_spec());
        }

        let req = CompletionRequest {
            model: model.id.clone(),
            system,
            messages: self.character.messages.clone(),
            tools,
            max_tokens: model.max_tokens,
            temperature: self.temperature_override.or(model.temperature),
        };

        let mut text_acc = String::new();
        let mut pending_calls: Vec<ToolCall> = Vec::new();
        let mut usage = Usage::default();
        let mut emitted = false;

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let mut ev_err: Option<Error> = None;
            let res = provider
                .stream(req.clone(), &mut |ev| match ev {
                    StreamEvent::TextDelta(d) => {
                        emitted = true;
                        text_acc.push_str(&d);
                        on_event(AgentEvent::TextDelta(d));
                    }
                    StreamEvent::ToolCallBegin { index, id, name } => {
                        while pending_calls.len() <= index {
                            pending_calls.push(ToolCall {
                                id: String::new(),
                                name: String::new(),
                                arguments: String::new(),
                            });
                        }
                        pending_calls[index] = ToolCall {
                            id,
                            name,
                            arguments: String::new(),
                        };
                    }
                    StreamEvent::ToolCallDelta { index, args } => {
                        if let Some(c) = pending_calls.get_mut(index) {
                            c.arguments.push_str(&args);
                        }
                    }
                    StreamEvent::Finish { usage: u } => usage = u,
                })
                .await;

            if let Err(e) = res {
                let retryable = matches!(&e, Error::Api { status, .. } if *status == 429 || *status >= 500)
                    || matches!(e, Error::Provider(_));
                if retryable && !emitted && attempts < 4 {
                    let wait = std::time::Duration::from_millis(1000 * 2u64.pow(attempts - 1));
                    on_event(AgentEvent::Notice(format!(
                        "{e}; retrying in {:?} (attempt {attempts}/3)",
                        wait
                    )));
                    tokio::time::sleep(wait).await;
                    continue;
                }
                ev_err = Some(e);
            }

            if let Some(e) = ev_err {
                return Err(e);
            }

            let mut parts = vec![];
            if !text_acc.is_empty() {
                parts.push(Content::Text(std::mem::take(&mut text_acc)));
            }
            if !pending_calls.is_empty() {
                parts.push(Content::ToolUse(std::mem::take(&mut pending_calls)));
            }
            return Ok((
                Message {
                    role: Role::Assistant,
                    parts,
                },
                usage,
            ));
        }
    }

    /// Execute a tool call, handling modes/permissions and `task` dispatch.
    async fn execute_call(
        &mut self,
        call: &ToolCall,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<String> {
        on_event(AgentEvent::ToolCall {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });

        if call.name == "task" {
            if self.is_subagent {
                return Ok("subagents cannot delegate further tasks".into());
            }
            // Boxed to break the async-recursion cycle (task -> chat -> task).
            return Box::pin(subagent::run_subagent(
                self,
                call.arguments.as_str(),
                on_event,
            ))
            .await;
        }

        let Some(tool) = self.registry.get(&call.name) else {
            return Err(Error::Other(format!("unknown tool '{}'", call.name)));
        };

        let mutating = tool.kind() == morrow_tools::ToolKind::Mutating;
        let args: serde_json::Value =
            serde_json::from_str(call.arguments.as_str()).unwrap_or_else(|_| serde_json::json!({}));
        match self.decide(&call.name, &args, mutating) {
            Decision::Deny(reason) => {
                on_event(AgentEvent::Notice(format!("blocked: {reason}")));
                return Ok(format!("denied: {reason}"));
            }
            Decision::Ask => {
                let ok = self.approval.approve(&call.name, &call.arguments).await;
                if !ok {
                    return Ok("denied by user".into());
                }
            }
            Decision::Allow => {}
        }

        // Share the live character with tools (remember/recall/integration mutate it),
        // then take the mutated state back so save_character persists everything.
        let shared = Arc::new(std::sync::Mutex::new(self.character.clone()));
        let mut ctx = self.ctx.clone();
        ctx.character = Some(shared.clone());

        let result = tool.run(&args, &ctx).await;

        self.character = shared.lock().unwrap().clone();
        result
    }

    /// Permission gate: config beats character mode, `deny` always wins.
    fn decide(&self, tool: &str, args: &serde_json::Value, mutating: bool) -> Decision {
        // Bash guardrails first — a denied pattern blocks in every mode.
        if tool == "bash" {
            if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                for pat in &self.config.bash_deny {
                    if glob_match(pat, cmd.trim()) {
                        return Decision::Deny(format!(
                            "bash command matches denied pattern '{pat}'"
                        ));
                    }
                }
            }
        }
        if let Some(p) = self.config.permissions.get(tool) {
            return match p {
                PermissionMode::Allow => Decision::Allow,
                PermissionMode::Ask => Decision::Ask,
                PermissionMode::Deny => {
                    Decision::Deny(format!("tool '{tool}' is denied by permissions config"))
                }
            };
        }
        if self.character.mode == Mode::Autonomous || !mutating {
            Decision::Allow
        } else {
            Decision::Ask
        }
    }

    /// Compact history when it grows past the configured fraction of the window.
    async fn maybe_compact(&mut self, on_event: &mut (dyn FnMut(AgentEvent) + Send)) -> Result<()> {
        self.compact_with_note(None, false, on_event).await?;
        Ok(())
    }

    /// `/compact [note]`: compress history now. Returns reclaimed tokens (est).
    pub async fn compact_now(
        &mut self,
        note: Option<String>,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<usize> {
        match self
            .compact_with_note(note.as_deref(), true, on_event)
            .await?
        {
            Some(reclaimed) => Ok(reclaimed),
            None => Err(Error::Other(
                "nothing worth compacting yet (need 5+ messages)".into(),
            )),
        }
    }

    async fn compact_with_note(
        &mut self,
        note: Option<&str>,
        force: bool,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<Option<usize>> {
        let (model, provider_cfg) = self.resolve_model()?;
        let window = model.context_window.unwrap_or(128_000);
        let budget = (window as f64 * self.config.compact_threshold) as usize;
        let est_tokens: usize = self
            .character
            .messages
            .iter()
            .map(|m| m.plain_text().len() / 4 + 32)
            .sum();
        if self.character.messages.len() < 5 {
            return Ok(None);
        }
        if !force && est_tokens < budget {
            return Ok(None);
        }

        on_event(AgentEvent::Notice(format!(
            "context ~{est_tokens} tokens > {budget}; compacting..."
        )));

        let keep = 4.min(self.character.messages.len());
        let split_at = self.character.messages.len() - keep;
        let older: Vec<Message> = self.character.messages.drain(..split_at).collect();

        let mut transcript = String::new();
        for m in &older {
            transcript.push_str(&format!("[{:?}] {}\n", m.role, m.plain_text()));
        }

        let mut system = "Summarize the conversation so far in under 400 words, preserving: the \
                     character's goal, decisions made, files modified, commands run, facts \
                     learned about the user, and current state."
            .to_string();
        if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
            system.push_str(&format!(" Pay special attention to: {n}"));
        }
        let api_key = self.config.resolve_api_key(&provider_cfg);
        let provider = morrow_provider::create(&provider_cfg, api_key)?;
        let req = CompletionRequest {
            model: model.id.clone(),
            system,
            messages: vec![Message::text(Role::User, transcript)],
            tools: vec![],
            max_tokens: None,
            temperature: None,
        };

        let mut summary = String::new();
        provider
            .stream(req, &mut |ev| {
                if let StreamEvent::TextDelta(d) = ev {
                    summary.push_str(&d);
                }
            })
            .await?;

        let summary_msg = Message::text(
            Role::User,
            format!("[earlier conversation summarized]\n{summary}"),
        );
        self.character.messages.insert(0, summary_msg);
        let kept: usize = self
            .character
            .messages
            .iter()
            .map(|m| m.plain_text().len() / 4 + 32)
            .sum();
        on_event(AgentEvent::Notice("compaction complete".into()));
        Ok(Some(est_tokens.saturating_sub(kept)))
    }

    /// `/flush [note]`: distill this thread into `.morrow/MEMORY.md`.
    /// Returns summary chars written. Great before `/compact` or `/new`.
    pub async fn flush_memory(
        &mut self,
        note: Option<String>,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<usize> {
        if self.character.messages.is_empty() {
            return Err(Error::Other("nothing to flush — thread is empty".into()));
        }
        let (model, provider_cfg) = self.resolve_model()?;
        // Tail-heavy transcript, capped so the flush turn stays cheap.
        let msgs = &self.character.messages;
        let start = msgs.len().saturating_sub(60);
        let mut transcript = String::new();
        for m in &msgs[start..] {
            let t = m.plain_text();
            if t.len() + transcript.len() > 12_000 {
                break;
            }
            transcript.push_str(&format!("[{:?}] {}\n", m.role, t));
        }
        let mut system = "Distill durable project learnings from this conversation: decisions, \
            conventions, gotchas, facts about the codebase. Skip chit-chat and transient \
            debugging. Output tight markdown bullets, under 1200 characters."
            .to_string();
        if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
            system.push_str(&format!(" Focus on: {n}"));
        }
        let api_key = self.config.resolve_api_key(&provider_cfg);
        let provider = morrow_provider::create(&provider_cfg, api_key)?;
        let req = CompletionRequest {
            model: model.id.clone(),
            system,
            messages: vec![Message::text(Role::User, transcript)],
            tools: vec![],
            max_tokens: None,
            temperature: None,
        };
        let mut summary = String::new();
        provider
            .stream(req, &mut |ev| {
                if let StreamEvent::TextDelta(d) = ev {
                    summary.push_str(&d);
                }
            })
            .await?;
        let view = self.current_session_view();
        let text = format!(
            "(thread {} · {})\n{}",
            &view.id[..8.min(view.id.len())],
            view.title,
            summary.trim()
        );
        let path = morrow_core::append_project_memory(&self.cwd, &text)?;
        on_event(AgentEvent::Notice(format!("flushed to {path}")));
        Ok(summary.len())
    }

    /// `/rename [title|--auto]`: manual titles freeze automatic titling.
    pub fn rename_session(&mut self, arg: Option<String>) -> Result<Session> {
        let Some(ps) = self.sessions.clone().filter(|_| !self.is_subagent) else {
            return Err(Error::Other("project sessions are not attached".into()));
        };
        let name = self.character.name.clone();
        let id = match &self.session_id {
            Some(id) => id.clone(),
            None => {
                self.snapshot_session()?;
                self.session_id
                    .clone()
                    .ok_or_else(|| Error::Other("no active session".into()))?
            }
        };
        let mut s = ps
            .load_exact(&name, &id)?
            .ok_or_else(|| Error::Other("active session file missing".into()))?;
        match arg.as_deref().map(str::trim) {
            None | Some("") | Some("--auto") => {
                s.title_manual = false;
                s.title = derive_title(&self.character.messages);
            }
            Some(t) => {
                s.title_manual = true;
                s.title = t.chars().take(60).collect();
            }
        }
        s.updated_at = chrono::Utc::now();
        ps.save(&s)?;
        Ok(s)
    }

    /// Current thread for `/info`: file version when present, else live view.
    pub fn current_session_view(&self) -> Session {
        if let Some(ps) = self.sessions() {
            if let Some(id) = &self.session_id {
                if let Ok(Some(s)) = ps.load_exact(&self.character.name, id) {
                    return s;
                }
            }
        }
        let mut s = Session::new(self.character.model.clone());
        s.character = self.character.name.clone();
        for m in &self.character.messages {
            s.push(m.clone());
        }
        s
    }

    /// Token/window breakdown for `/context`.
    pub fn context_breakdown(&self) -> Vec<String> {
        let (model_id, window) = match self.resolve_model() {
            Ok((m, _)) => (m.id.clone(), m.context_window.unwrap_or(128_000)),
            Err(_) => ("unknown".into(), 128_000),
        };
        let sys = self.character.system_prompt(&self.cwd);
        let sys_est = sys.len() / 4;
        let msg_est: usize = self
            .character
            .messages
            .iter()
            .map(|m| m.plain_text().len() / 4 + 32)
            .sum();
        let tools = self.registry.specs(self.allowlist.as_deref());
        let tools_est = tools.len() * 256;
        let total = sys_est + msg_est + tools_est;
        let pct = total as f64 / window as f64 * 100.0;
        vec![
            format!("model: {model_id} (window {window})"),
            format!("system prompt: ~{sys_est} tokens"),
            format!(
                "messages: ~{msg_est} tokens ({} msgs)",
                self.character.messages.len()
            ),
            format!("tool defs: ~{tools_est} tokens ({} tools)", tools.len()),
            format!("total: ~{total} / {window} ({pct:.1}%)"),
        ]
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn provider_override(&self) -> Option<Arc<dyn LlmProvider>> {
        self.provider_override.clone()
    }
}

impl Runner {
    /// Isolated runner for a subagent: own ephemeral state, restricted tools, custom persona.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_subagent(
        config: Arc<Config>,
        cwd: PathBuf,
        _model_key: Option<String>,
        allowlist: Option<Vec<String>>,
        persona: String,
        temperature: Option<f64>,
        provider: Option<Arc<dyn LlmProvider>>,
    ) -> Self {
        let mut ctx = ToolContext::new(cwd.clone());
        ctx.timeout_secs = config.bash_timeout_secs;
        Self {
            config,
            registry: morrow_tools::builtin_registry(),
            ctx,
            cwd,
            character: Character::new("subagent"),
            store: CharacterStore::new(),
            approval: Arc::new(crate::approval::AutoApprove),
            total_usage: Usage::default(),
            allowlist,
            persona_prompt: Some(persona),
            temperature_override: temperature,
            is_subagent: true,
            provider_override: provider,
            sessions: None,
            session_id: None,
            mcp_clients: Vec::new(),
            mcp_tool_counts: Vec::new(),
            mcp_ready: true, // subagents never spawn MCP servers
        }
    }
}

impl Runner {
    pub fn character_name(&self) -> &str {
        &self.character.name
    }

    /// One-line status for UIs: "Sam · grok-4 · autonomous".
    pub fn status_line(&self) -> String {
        format!(
            "{} · {} · {}",
            self.character.name,
            self.character.model.as_deref().unwrap_or("no model"),
            match self.character.mode {
                Mode::Autonomous => "autonomous",
                Mode::Approval => "approval",
            }
        )
    }
}

/// Outcome of the permission gate.
enum Decision {
    Allow,
    Ask,
    Deny(String),
}

/// Glob match where `*` spans any sequence (including empty); all else literal.
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Title for a snapshot whose stored title is still the default.
fn derive_title(messages: &[Message]) -> String {
    for m in messages {
        if m.role == Role::User {
            let t = m.plain_text();
            if !t.trim().is_empty() {
                return t.chars().take(60).collect();
            }
        }
    }
    "new session".into()
}

/// Build a tool context bound to a character's shared state + settings.
fn ctx_for(
    character: &Character,
    store: &CharacterStore,
    config: &Config,
    cwd: PathBuf,
) -> ToolContext {
    let _ = store;
    let mut ctx =
        ToolContext::new(cwd).with_character(Arc::new(std::sync::Mutex::new(character.clone())));
    ctx.timeout_secs = config.bash_timeout_secs;
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_matches() {
        assert!(glob_match("rm -rf /*", "rm -rf /tmp/x"));
        assert!(!glob_match("rm -rf /*", "rm -rf"));
        assert!(glob_match(
            "git push --force*",
            "git push --force origin main"
        ));
        assert!(glob_match("*", "anything at all"));
        assert!(glob_match("shutdown *", "shutdown now"));
        assert!(!glob_match("shutdown *", "shutdown"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exact!"));
    }
}

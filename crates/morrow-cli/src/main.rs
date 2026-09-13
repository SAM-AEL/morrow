use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};
use morrow_agent::{AgentEvent, AutoApprove, Runner};

#[derive(Parser)]
#[command(
    name = "morrow",
    version,
    about = "morrow — character-based agents that learn and grow with every interaction",
    arg_required_else_help = false
)]
struct Cli {
    /// Character to embody (created on first use; each has own memory/chat)
    #[arg(short, long)]
    agent: Option<String>,
    /// Model key: alias, id or provider/id (overrides the character's model)
    #[arg(short, long)]
    model: Option<String>,
    /// Config file path (default: global + project merged)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Working directory for the agent
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Start in fully autonomous mode
    #[arg(long)]
    auto: bool,
    /// Resume a past thread by id prefix (headless: continue it, then prompt)
    #[arg(long)]
    resume: Option<String>,
    /// Continue the active thread for this project (headless)
    #[arg(long = "continue")]
    cont: bool,
    /// Start headless work in a brand-new thread with this id (must be unique)
    #[arg(long = "session-id")]
    session_id: Option<String>,
    /// Non-interactive prompt (headless mode)
    prompt: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Launch the interactive TUI (default when no prompt given)
    Ui,
    /// Run a single headless prompt with the character
    Run {
        prompt: String,
        /// Print only the final assistant message
        #[arg(long)]
        quiet: bool,
    },
    /// Manage characters (persistent personas with private memory)
    #[command(subcommand)]
    Characters(CharactersCmd),
    /// Manage saved models (the "engines" powering characters)
    #[command(subcommand)]
    Models(ModelsCmd),
    /// List subagents available for task delegation
    Subagents,
    /// Execute any scheduled tasks that are due, then exit
    Tick,
    /// Resident scheduler: keep ticking every 30 seconds until killed
    Daemon {
        /// Seconds between ticks
        #[arg(long, default_value = "30")]
        interval: u64,
    },
    /// List scheduled tasks across all characters
    Schedules,
    /// List past session threads for a character in this project (.morrow/)
    Sessions {
        /// Character whose threads to list (default: nova)
        #[arg(long)]
        character: Option<String>,
    },
    /// Start a fresh session thread (current thread is kept)
    New {
        /// Optional title for the new thread
        #[arg(long)]
        title: Option<String>,
    },
    /// Resume a past session thread by id prefix
    Resume {
        /// Session id (or unique prefix, see `sessions`)
        id: String,
    },
    /// Show project rule files loaded for this directory
    Rules,
    /// List user-defined slash commands (.morrow/commands/*.md)
    Commands,
    /// Show configured external tool (MCP) servers
    Mcp,
    /// Rename the active thread (or --auto to restore auto-titles)
    Rename {
        /// New title (omit with --auto)
        title: Option<String>,
        /// Clear the manual title and resume automatic titling
        #[arg(long)]
        auto: bool,
    },
    /// Compress the active thread's history now
    Compact {
        /// Note on what to preserve
        #[arg(long)]
        note: Option<String>,
    },
    /// Start the HTTP API server (chat, memory, schedules, keys over REST)
    Serve {
        /// Interface to bind
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on
        #[arg(long, default_value = "8471")]
        port: u16,
    },
    /// Initialize sample configuration
    InitConfig {
        /// Write into the project (.morrow/config.toml) instead of the global path
        #[arg(long)]
        project: bool,
    },
}

#[derive(Subcommand)]
enum CharactersCmd {
    /// List all characters
    List,
    /// Create a new character with a custom persona
    New {
        name: String,
        /// Personality / role definition (quoted text)
        #[arg(long)]
        persona: Option<String>,
        /// Model key powering this character
        #[arg(long)]
        model: Option<String>,
        /// Start in autonomous mode
        #[arg(long)]
        autonomous: bool,
    },
    /// Show one character's memory, integrations and stats
    Show { name: String },
    /// Delete a character and everything it learned
    Delete { name: String },
}

#[derive(Subcommand)]
enum ModelsCmd {
    List,
    Add {
        /// Key under [models.*] — also usable as alias on the CLI
        name: String,
        /// Raw provider model id
        #[arg(long)]
        id: String,
        /// Provider name from [providers.*]
        #[arg(long)]
        provider: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        context_window: Option<u64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> Result<()> {
    let cwd = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let mut config = match &cli.config {
        Some(p) => morrow_core::Config::load_from(p)?,
        None => morrow_core::Config::load()?,
    };
    config.permission = morrow_core::PermissionMode::Allow; // gating is per-character now
    let config = Arc::new(config);

    match cli.command {
        Some(Command::Tick) => return tick_cmd(config, cwd).await,
        Some(Command::Daemon { interval }) => return daemon_cmd(config, cwd, interval).await,
        Some(Command::Schedules) => return schedules_cmd(),
        Some(Command::Sessions { character }) => {
            let who = character
                .as_deref()
                .or(cli.agent.as_deref())
                .unwrap_or("nova")
                .to_string();
            return sessions_cmd(cwd, &who);
        }
        Some(Command::New { title }) => {
            return new_cmd(
                config,
                cwd,
                cli.agent.clone().unwrap_or_else(|| "nova".to_string()),
                title,
            )
        }
        Some(Command::Resume { id }) => {
            return resume_cmd(
                config,
                cwd,
                cli.agent.clone().unwrap_or_else(|| "nova".to_string()),
                &id,
            )
        }
        Some(Command::Rules) => return rules_cmd(cwd),
        Some(Command::Commands) => return commands_cmd(cwd),
        Some(Command::Mcp) => return mcp_cmd(config),
        Some(Command::Rename { title, auto }) => {
            let arg = if auto {
                Some("--auto".to_string())
            } else {
                title
            };
            return rename_cmd(
                config,
                cwd,
                cli.agent.clone().unwrap_or_else(|| "nova".to_string()),
                arg,
            );
        }
        Some(Command::Compact { note }) => {
            return compact_cmd(
                config,
                cwd,
                cli.agent.clone().unwrap_or_else(|| "nova".to_string()),
                note,
            )
            .await;
        }
        Some(Command::Serve { host, port }) => return morrow_server::serve(&host, port).await,
        Some(Command::Models(cmd)) => return models_cmd(config, cmd),
        Some(Command::Characters(cmd)) => return characters_cmd(cmd),
        Some(Command::InitConfig { project }) => {
            let path = if project {
                cwd.join(morrow_core::Config::project_path())
            } else {
                morrow_core::Config::global_path()
            };
            if path.exists() {
                bail!("{} already exists", path.display());
            }
            morrow_core::Config::write_sample(&path)?;
            println!("wrote {}", path.display());
            return Ok(());
        }
        Some(Command::Subagents) => {
            println!("available subagents:");
            for (name, a) in &config.agents {
                if a.hidden {
                    continue;
                }
                println!(
                    "  {:<12} {}{}",
                    name,
                    a.description.clone().unwrap_or_default(),
                    a.model
                        .as_ref()
                        .map(|m| format!("  [model: {m}]"))
                        .unwrap_or_default()
                );
            }
            return Ok(());
        }
        // `ui` falls through to the interactive TUI path below.
        Some(Command::Ui) => {}
        _ => {}
    }

    // ---- run anything that came due while we were away ---------------------
    let store0 = morrow_core::CharacterStore::new();
    let reports = morrow_agent::scheduler::tick(&store0, &config, cwd.clone()).await?;
    for r in &reports {
        eprintln!("[schedule] {} ✓ {}", r.character, r.task);
        if !quiet_mode(cli.command.as_ref()) && !r.result.is_empty() {
            eprintln!("[schedule] → {}", first_line(&r.result));
        }
    }

    // ---- character setup ---------------------------------------------------
    let agent_name = cli.agent.clone().unwrap_or_else(|| "nova".to_string());
    let store = morrow_core::CharacterStore::new();
    let mut character = store.load_or_create(&agent_name)?;
    if cli.auto {
        character.mode = morrow_core::Mode::Autonomous;
    }

    let approval: Arc<dyn morrow_agent::ApprovalHandler> = Arc::new(AutoApprove);
    let runner = Runner::with_store(
        store.clone(),
        config.clone(),
        cwd.clone(),
        cli.model.clone(),
        approval,
        character,
    );

    let interactive = cli.prompt.is_none() && matches!(cli.command, None | Some(Command::Ui));
    if interactive {
        let tui_approval: Arc<dyn morrow_agent::ApprovalHandler> =
            Arc::new(morrow_tui::TuiApproval::new());
        let tui_runner = Runner::with_store(
            store.clone(),
            config,
            cwd,
            cli.model,
            tui_approval,
            runner.character.clone(),
        );
        return morrow_tui::run(tui_runner).await;
    }

    // ---- headless ----------------------------------------------------------
    let prompt = match (&cli.command, &cli.prompt) {
        (_, Some(p)) => p.clone(),
        (Some(Command::Run { prompt, .. }), None) => prompt.clone(),
        _ => bail!("no prompt"),
    };
    let quiet = matches!(cli.command, Some(Command::Run { quiet: true, .. }));

    let mut runner = runner;
    // Headless turns join the active project thread (autosnapshotted on save).
    runner.attach_project_sessions(cwd.clone());
    if let Some(q) = cli.resume.as_deref() {
        let s = runner.resume_session(q)?;
        eprintln!(
            "[morrow] resumed thread {} ({})",
            &s.id[..8.min(s.id.len())],
            s.title
        );
    } else if cli.cont {
        let active = runner.active_session_id().map(|s| s.to_string());
        match active {
            Some(id) => {
                let s = runner.resume_session(&id)?;
                eprintln!(
                    "[morrow] continued thread {} ({})",
                    &s.id[..8.min(s.id.len())],
                    s.title
                );
            }
            None => eprintln!("[morrow] no active thread here — starting fresh"),
        }
    }
    if let Some(sid) = cli.session_id.as_deref() {
        let s = runner.begin_session_with_id(sid)?;
        eprintln!(
            "[morrow] new thread {} ({})",
            &s.id[..8.min(s.id.len())],
            s.title
        );
    }
    eprintln!(
        "[morrow] character '{}' · mode {:?}{}",
        runner.character_name(),
        runner.mode(),
        runner
            .model_key()
            .map(|m| format!(" · model {m}"))
            .unwrap_or_default()
    );
    let mut on_event = |ev: AgentEvent| match ev {
        AgentEvent::TextDelta(t) => {
            if !quiet {
                print!("{t}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
        AgentEvent::ToolCall { name, .. } => eprintln!("[tool] {name}"),
        AgentEvent::Notice(n) => eprintln!("[morrow] {n}"),
        _ => {}
    };

    let answer = runner.chat(&prompt, &mut on_event).await?;
    runner.save_character()?;
    if quiet {
        println!("{answer}");
    } else {
        println!();
    }
    eprintln!(
        "[usage] {} in / {} out tokens",
        runner.total_usage.input_tokens, runner.total_usage.output_tokens
    );
    Ok(())
}

fn quiet_mode(cmd: Option<&Command>) -> bool {
    matches!(cmd, Some(Command::Run { quiet: true, .. }))
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

async fn tick_cmd(config: Arc<morrow_core::Config>, cwd: PathBuf) -> Result<()> {
    let store = morrow_core::CharacterStore::new();
    let reports = morrow_agent::scheduler::tick(&store, &config, cwd).await?;
    if reports.is_empty() {
        println!("no scheduled tasks due");
        return Ok(());
    }
    for r in reports {
        println!("{} ✓ {}\n  {}", r.character, r.task, first_line(&r.result));
    }
    Ok(())
}

async fn daemon_cmd(config: Arc<morrow_core::Config>, cwd: PathBuf, interval: u64) -> Result<()> {
    let store = morrow_core::CharacterStore::new();
    println!(
        "morrow daemon watching schedules every {interval}s (ctrl-c stops; \
         characters will act autonomously when tasks come due)"
    );
    loop {
        let reports = morrow_agent::scheduler::tick(&store, &config, cwd.clone()).await?;
        for r in reports {
            println!(
                "[{}] {} ✓ {} → {}",
                chrono::Local::now().format("%H:%M:%S"),
                r.character,
                r.task,
                first_line(&r.result)
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval.max(5))).await;
    }
}

fn sessions_cmd(cwd: PathBuf, character: &str) -> Result<()> {
    let ps = morrow_core::ProjectSessions::new(&cwd);
    let list = ps.list(character)?;
    if list.is_empty() {
        println!("no past sessions for {character} in {}", cwd.display());
        return Ok(());
    }
    for s in list {
        let active = ps.active_id(character).as_deref() == Some(s.id.as_str());
        println!(
            "{}{}  {}  · {} · {} msgs",
            &s.id[..8.min(s.id.len())],
            if active { " *" } else { "  " },
            s.title,
            s.updated_at.format("%m-%d %H:%M"),
            s.messages.len()
        );
    }
    Ok(())
}

fn build_session_runner(
    config: Arc<morrow_core::Config>,
    cwd: PathBuf,
    character_name: String,
) -> Result<Runner> {
    let store = morrow_core::CharacterStore::new();
    let character = store.load_or_create(&character_name)?;
    let mut runner = Runner::with_store(
        store,
        config,
        cwd.clone(),
        None,
        Arc::new(AutoApprove),
        character,
    );
    runner.attach_project_sessions(cwd);
    Ok(runner)
}

fn new_cmd(
    config: Arc<morrow_core::Config>,
    cwd: PathBuf,
    character_name: String,
    title: Option<String>,
) -> Result<()> {
    let mut runner = build_session_runner(config, cwd, character_name)?;
    let s = runner.start_new_session(title)?;
    println!("new session {} ({})", &s.id[..8.min(s.id.len())], s.title);
    Ok(())
}

fn resume_cmd(
    config: Arc<morrow_core::Config>,
    cwd: PathBuf,
    character_name: String,
    id: &str,
) -> Result<()> {
    let mut runner = build_session_runner(config, cwd, character_name)?;
    let s = runner.resume_session(id)?;
    println!(
        "resumed {} ({}) — {} messages",
        &s.id[..8.min(s.id.len())],
        s.title,
        s.messages.len()
    );
    Ok(())
}

fn rename_cmd(
    config: Arc<morrow_core::Config>,
    cwd: PathBuf,
    character_name: String,
    arg: Option<String>,
) -> Result<()> {
    let mut runner = build_session_runner(config, cwd, character_name)?;
    let s = runner.rename_session(arg)?;
    println!("thread is now \"{}\"", s.title);
    Ok(())
}

async fn compact_cmd(
    config: Arc<morrow_core::Config>,
    cwd: PathBuf,
    character_name: String,
    note: Option<String>,
) -> Result<()> {
    let mut runner = build_session_runner(config, cwd, character_name)?;
    let mut noop = |_: AgentEvent| {};
    match runner.compact_now(note, &mut noop).await {
        Ok(reclaimed) => println!("compacted — reclaimed ~{reclaimed} tokens"),
        Err(e) => println!("compact skipped: {e}"),
    }
    runner.save_character()?;
    Ok(())
}

fn mcp_cmd(config: Arc<morrow_core::Config>) -> Result<()> {
    if config.mcp_servers.is_empty() {
        println!("no MCP servers configured — see [mcp_servers.*] in config");
        return Ok(());
    }
    println!("MCP servers (tools appear as mcp_<server>_<tool>):");
    for (name, cfg) in &config.mcp_servers {
        println!(
            "  {:<14} {} {}  [{}]",
            name,
            cfg.command,
            cfg.args.join(" "),
            if cfg.enabled { "enabled" } else { "disabled" }
        );
    }
    Ok(())
}

fn commands_cmd(cwd: PathBuf) -> Result<()> {
    let cmds = morrow_core::load_custom_commands(&cwd);
    if cmds.is_empty() {
        println!("no custom commands — add .morrow/commands/<name>.md");
        println!("frontmatter `description:` + `$ARGUMENTS` placeholder supported");
        return Ok(());
    }
    println!("custom slash commands:");
    for c in cmds {
        println!("  /{:<14} {}  [{}]", c.name, c.description, c.scope);
    }
    Ok(())
}

fn rules_cmd(cwd: PathBuf) -> Result<()> {
    let rules = morrow_core::ProjectRules::load(&cwd);
    if rules.is_empty() {
        println!("no project rules loaded for {}", cwd.display());
        println!("add MORROW.md or .morrow/rules/*.md to teach this project");
        return Ok(());
    }
    println!(
        "project rules in effect ({} files, {} chars):",
        rules.len(),
        rules.total_chars()
    );
    for (path, content) in &rules.files {
        println!("  {path}  ({} chars)", content.len());
    }
    Ok(())
}

fn schedules_cmd() -> Result<()> {
    let store = morrow_core::CharacterStore::new();
    let mut any = false;
    for c in store.list()? {
        for sched in &c.schedules {
            any = true;
            println!("{:<12} {}", c.name, sched.describe());
            println!("             task: {}", first_line(&sched.task));
        }
    }
    if !any {
        println!(
            "nothing scheduled. Ask a character to schedule something, e.g. \"run this at 12am\""
        );
    }
    Ok(())
}

fn characters_cmd(cmd: CharactersCmd) -> Result<()> {
    let store = morrow_core::CharacterStore::new();
    match cmd {
        CharactersCmd::List => {
            let chars = store.list()?;
            if chars.is_empty() {
                println!("no characters yet — `morrow -a Sam` creates one on first use");
                return Ok(());
            }
            println!(
                "{:<14} {:<12} {:<18} {:>3} {:>3} UPDATED",
                "NAME", "MODE", "MODEL", "MEM", "MSG"
            );
            for c in chars {
                println!(
                    "{:<14} {:<12} {:<18} {:>3} {:>3} {}",
                    c.name,
                    format!("{:?}", c.mode).to_lowercase(),
                    c.model.as_deref().unwrap_or("-"),
                    c.memories.len(),
                    c.messages.len(),
                    c.updated_at.format("%Y-%m-%d %H:%M")
                );
            }
        }
        CharactersCmd::New {
            name,
            persona,
            model,
            autonomous,
        } => {
            if store.exists(&name) {
                bail!("character '{name}' already exists");
            }
            let mut c = store.load_or_create(&name)?;
            if let Some(p) = persona {
                c.persona = p;
            }
            if let Some(m) = model {
                c.model = Some(m);
            }
            if autonomous {
                c.mode = morrow_core::Mode::Autonomous;
            }
            store.save(&mut c)?;
            println!("character '{name}' created");
        }
        CharactersCmd::Show { name } => {
            let c = store
                .load(&name)?
                .with_context(|| format!("no character '{name}'"))?;
            println!("name:     {}", c.name);
            println!("mode:     {:?}", c.mode);
            println!(
                "model:    {}",
                c.model.as_deref().unwrap_or("(inherit default)")
            );
            println!("persona:  {}", c.persona);
            println!("memories: {}", c.memories.len());
            println!("chats:    {} messages", c.messages.len());
            if !c.integrations.is_empty() {
                println!("integrations:");
                for (n, i) in &c.integrations {
                    println!(
                        "  {n}: {} @ {} (key: {})",
                        i.description,
                        i.base_url,
                        i.masked_key()
                    );
                }
            }
            if !c.memories.is_empty() {
                println!("recent memories:");
                for m in c.memories.iter().rev().take(10) {
                    println!("  [{}] {}", m.at.format("%Y-%m-%d"), m.text);
                }
            }
        }
        CharactersCmd::Delete { name } => {
            if store.delete(&name)? {
                println!("character '{name}' deleted");
            } else {
                bail!("no character '{name}'");
            }
        }
    }
    Ok(())
}

fn models_cmd(config: Arc<morrow_core::Config>, cmd: ModelsCmd) -> Result<()> {
    match cmd {
        ModelsCmd::List => {
            println!("{:<16} {:<28} PROVIDER", "NAME", "ID");
            for (key, m) in &config.models {
                println!("{:<16} {:<28} {}", key, m.id, m.provider);
            }
            if config.models.is_empty() {
                eprintln!("no models configured. See `morrow init-config` / config.toml.");
            }
        }
        ModelsCmd::Add {
            name,
            id,
            provider,
            display_name,
            context_window,
        } => {
            if !config.providers.contains_key(&provider) {
                bail!(
                    "unknown provider '{provider}'. Add it first under [providers.{provider}] in {}",
                    morrow_core::Config::global_path().display()
                );
            }
            let path = morrow_core::Config::global_path();
            use std::io::Write;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)?;
            let mut entry =
                format!("\n[models.{name}]\nid = \"{id}\"\nprovider = \"{provider}\"\n");
            if let Some(dn) = display_name {
                entry.push_str(&format!("display_name = \"{dn}\"\n"));
            }
            if let Some(cw) = context_window {
                entry.push_str(&format!("context_window = {cw}\n"));
            }
            f.write_all(entry.as_bytes())?;
            println!("saved model '{name}' → {}", path.display());
        }
    }
    Ok(())
}

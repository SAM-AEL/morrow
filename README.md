<div align="center">

# morrow

**Character-based AI agents that learn, grow, and get work done — from your terminal.**

morrow is a fast, hackable, open-source **terminal coding agent** built in Rust. Instead
of a stateless chatbot, you talk to *characters* — persistent personas with their own
memory, personality, tool access, scheduled tasks, and third-party API integrations.

Characters keep private conversations, remember durable facts across sessions and projects,
delegate work to isolated subagents, and can operate fully autonomously (or ask you first).

<br/>

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Language](https://img.shields.io/badge/built%20with-Rust-orange.svg)
![Status](https://img.shields.io/badge/status-experimental-yellow)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)

<br/>

**[Quick Start](#quick-start)** · **[Configuration](#configuration)** · **[Usage](#usage)** ·
**[HTTP API](#http-api-server)** · **[Security](#security)** · **[Architecture](#architecture)** ·
**[Development](#development)**

</div>

---

## Table of Contents

- [What is morrow?](#what-is-morrow)
- [Features](#features)
- [Quick Start](#quick-start)
  - [Requirements](#requirements)
  - [Install](#install)
  - [First run](#first-run)
  - [Headless mode](#headless-mode)
- [Configuration](#configuration)
  - [Where config lives](#where-config-lives)
  - [Providers](#providers)
  - [Models](#models)
  - [Permissions & safety](#permissions--safety)
  - [MCP servers](#mcp-servers)
  - [Subagents](#subagents)
- [Usage](#usage)
  - [Interactive TUI](#interactive-tui)
  - [Slash commands](#slash-commands)
  - [CLI commands](#cli-commands)
  - [Sessions & threads](#sessions--threads)
  - [Scheduling](#scheduling)
  - [Project rules & memory](#project-rules--memory)
  - [Custom slash commands](#custom-slash-commands)
- [Built-in tools](#built-in-tools)
- [HTTP API server](#http-api-server)
- [Security](#security)
- [Architecture](#architecture)
- [Repository layout](#repository-layout)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

---

## What is morrow?

Morrow is a *character*-centered agent runtime. The difference from a plain LLM CLI:

| Plain CLI assistant | morrow character |
|---------------------|----------------------|
| Stateless per invocation | Persistent identity, memory & chat across runs |
| One shared context | Each character has **private** memory and messages |
| Fixed model | Per-character model, switchable at runtime |
| No concept of time | Natural-language scheduling ("run this every monday at 9am") |
| Whatever tools the host grants | Characters manage their own API integrations themselves |

Characters are stored as single JSON files (`~/.local/share/morrow/agents/<name>.json`)
— trivial to back up, move between machines, or version control.

---

## Features

- **🪄 Persistent characters** — each has a persona, model, independent memory, and a
  private conversation that survives restarts.
- **🔎 Full coding-agent toolset** — `bash`, `read`, `write`, `edit`, `grep`, `glob`,
  `ls`, `webfetch`, `todo`, and more (see [Built-in tools](#built-in-tools)).
- **🛡️ Approval or autonomy** — default `Approval` mode asks before every mutating tool
  call; flip to `Autonomous` (per-character) with `/mode` or `--auto`.
- **🧠 Self-managed memory** — characters *remember* durable facts
  (scope: `character` or `project`), surfaced automatically in future prompts.
- **👥 Subagents** — delegate research and parallel work to isolated agents
  (built-in `general`, `explore`, `plan`, plus custom ones) that return a single answer.
- **⏰ Scheduling** — tell a character *"run this every day at 6pm"* in plain English;
  `morrow daemon` or the API server executes due tasks autonomously.
- **🔌 MCP support** — attach any [Model Context Protocol](https://modelcontextprotocol.io)
  stdio server; its tools appear as `mcp_<server>_<tool>` behind the normal permission gate.
- **🌐 BYO model** — one config surface for OpenAI, OpenAI-compatible (xAI, Groq,
  DeepSeek, OpenRouter, Ollama, vLLM, LM Studio, ...), Codex/Responses, and Anthropic.
- **💬 Interactive TUI** — ratatui-powered terminal UI with streaming output, live
  completion for `/` commands, threads, splash loader, and inline approval prompts.
- **🔌 Headless & API** — `morrow run "..."`, batch CLI commands, and a REST API
  with agent-scoped bearer keys (SHA-256 hashed, never stored in plaintext).
- **📁 Project-aware** — honors `MORROW.md` / `AGENTS.md` / `CLAUDE.md`, project rules in
  `.morrow/rules/`, commit-worthy project memory in `.morrow/MEMORY.md`, and per-project
  session threads in `.morrow/sessions/`.

---

## Quick Start

### Requirements

- **Rust** toolchain (edition 2021), i.e. `curl https://sh.rustup.rs | sh`
- Platform: **macOS** or **Linux** (a POSIX shell is assumed by the `bash` tool)
- An LLM provider — or Ollama / a local endpoint for a fully local setup

### Install

Build from source (the workspace compiles into a single `morrow` binary):

```bash
cargo install --path crates/morrow-cli
morrow --help
```

or run without installing:

```bash
cargo run -p morrow-cli -- ui
```

or use the packaged `package.json` scripts (`yarn build` → `cargo build -p morrow-cli`).

### First run

```bash
# 1. Generate a commented, documented config (global, or --project for .morrow/)
morrow init-config

# 2. Add your provider + model to ~/.config/morrow/config.toml
#    (or copy the committed template: cp .morrow/config.example.toml .morrow/config.toml)

# 3. Launch the TUI (character "nova" is created on first use)
morrow -a nova
```

> **API keys are read from environment variables** — e.g. `api_key = "$OPENAI_API_KEY"`.
> Set it with `export OPENAI_API_KEY=sk-...`. See [Security](#security).

### Headless mode

```bash
# One-shot prompt, print the final answer
morrow -a nova -p "what does this repo's workspace consist of?"

# Same as a subcommand, quiet (final message only)
morrow run "summarize Cargo.toml" --quiet

# Start a brand-new, idempotent thread in one shot (for scripts)
morrow --session-id 8f3a91b2 -p "bootstrap the project"

# Fully autonomous agentic run
morrow -a nova --auto -p "find the bug in crates/morrow-agent and fix it"
```

---

## Configuration

### Where config lives

Config files are **TOML**, loaded in order (later files win per-key):

| Path | Scope | Notes |
|------|-------|-------|
| `~/.config/morrow/config.toml` | global | `morrow init-config` writes here by default |
| `.morrow/config.toml` | project | **gitignored** — may contain real keys |
| `.morrow/config.toml` | legacy | still read for older setups; loses to `.morrow/` |

`morrow init-config --project` writes the sample into `.morrow/config.toml`.
A fully documented template is committed as [`.morrow/config.example.toml`](.morrow/config.example.toml).

### Providers

A provider is an endpoint + wire protocol. Three protocols are supported:

- `openai-chat` — `POST {base_url}/chat/completions` (OpenAI, xAI, Groq, DeepSeek,
  OpenRouter, Ollama, vLLM, LM Studio, ...)
- `openai-responses` — `POST {base_url}/responses` (OpenAI Codex / Responses API)
- `anthropic` — `POST {base_url}/v1/messages` (Claude)

```toml
[providers.grok]
base_url = "https://api.x.ai/v1"
api_key  = "$XAI_API_KEY"     # "$ENV_VAR" recommended; literal keys also work
protocol = "openai-chat"

[providers.openrouter.headers]   # optional extra headers
HTTP-Referer = "https://morrow.local"
X-Title      = "morrow"

[providers.my-provider.query]    # optional extra query params
version = "2024-01-01"
```

Omitting `api_key` is fine for keyless local endpoints (Ollama, LM Studio).

### Models

Models are referenced by an **alias key** (`[models.<key>]`). The CLI accepts the alias,
the raw model id, or `provider/id`:

```toml
[models.grok-fast]
id              = "grok-4-1-fast-non-reasoning"
provider        = "grok"
display_name    = "Grok Fast"
context_window  = 200000     # used by automatic compaction
max_tokens      = 8192
temperature     = 0.7
```

`morrow -m grok-fast`, `/model grok-fast`, or per-character `model = "grok-fast"`.

### Permissions & safety

Tunable at three levels — the most restrictive wins:

1. **Character mode** — `mode = "approval"` (default) or `"autonomous"` per character.
2. **Config default** — `permission = "ask" | "allow" | "deny"` (top-level config).
3. **Per-tool overrides** — `[permissions]`, e.g.

```toml
[permissions]
edit = "ask"     # ask | allow | deny — deny always wins, even in autonomous mode
bash = "ask"
read = "allow"

# Bash guardrails: glob patterns always blocked, matched against the command
bash_deny = ["rm -rf /*", "git push --force*", "shutdown *"]
```

### MCP servers

Attach external tool servers over stdio. Their tools surface as `mcp_<server>_<tool>`
and pass through the same permission gate:

```toml
[mcp_servers.files]
command    = "npx"
args       = ["-y", "@modelcontextprotocol/server-filesystem", "/home/me/projects"]
enabled    = true
timeout_secs = 60
```

### Subagents

The primary model can delegate to subagents via the `task` tool. Built-ins ship out of
the box (`general`, `explore` — read-only, `plan` — read-only); extend in config:

```toml
[agents.reviewer]
prompt       = "You are a strict code reviewer. Report risks with file:line evidence."
model        = "fast"                    # optional: different model than the parent
tools        = ["read", "grep", "glob"]  # optional: restrict the toolset
temperature  = 0.2
description  = "Code reviewer subagent"  # shown when the parent picks an agent
hidden       = false
```

Subagents run in an isolated session, cannot spawn further subagents, and return only
their final answer — ideal for codebase research and parallel investigation.
---

## Usage

### Interactive TUI

```bash
morrow                # default character
morrow -a nova        # named character
morrow -m fast        # override model
morrow --auto         # start autonomous
```

| Key | Action |
|-----|--------|
| `Enter` | Submit prompt / complete `/` dropdown |
| `Tab` | Complete slash command or value (models, characters, threads) |
| `↑` / `↓` | Navigate completions, or scroll transcript |
| `PgUp` / `PgDn` | Scroll transcript by pages |
| `Esc` | Dismiss completions |
| `Ctrl+C` | Cancel current input (idle: quit) |
| `Ctrl+Q` | Quit |
| `y` / `n` | Approve / deny a pending tool request (approval modal) |

Streaming text, tool calls, tool results, usage, and subagent activity are all rendered
live; a rainbow spinner covers the terminal while the model is busy.

### Slash commands

| Command | Description |
|---------|-------------|
| `/help` | show all commands |
| `/new [title]` | start a fresh session (current thread is preserved) |
| `/resume <id-prefix>` | continue a past thread — press `Tab` to list threads |
| `/sessions` | list past sessions for this character |
| `/rename [title]` · `/rename --auto` | manual title / restore auto-titling |
| `/compact [note]` | compress history now (what to preserve) |
| `/flush [note]` | save this thread's learnings to `.morrow/MEMORY.md` |
| `/info` | thread details: model, turns, usage |
| `/context` | context-window breakdown |
| `/memory` | project memory stats (`.morrow/MEMORY.md`) |
| `/model <key>` | switch model — `Tab` completes |
| `/models` | list saved models |
| `/switch <name>` | switch character — `Tab` completes |
| `/mode` | toggle approval ↔ autonomous |
| `/agents` | list subagents |
| `/mcp` | show connected external tool servers |
| `/rules` | show loaded rule files |
| `/usage` | token totals |
| `/clear` | clear the transcript |
| `/quit` | exit |

### CLI commands

```
morrow ui                 launch the interactive TUI
morrow run <prompt>       one-shot headless run
morrow characters         list / new / show / delete characters
morrow models             list / add saved models
morrow subagents          list available subagents
morrow sessions [--character X]
morrow new [--title T]    start a fresh thread
morrow resume <id>        continue a past thread
morrow rename <title|--auto>
morrow compact [--note N] compress the active thread's history
morrow rules              show project rule files in effect
morrow commands           list user-defined slash commands
morrow mcp                show configured MCP servers
morrow schedules          list scheduled tasks across characters
morrow tick               run due scheduled tasks once, then exit
morrow daemon --interval 30   resident scheduler loop
morrow serve [--host 127.0.0.1 --port 8471]   HTTP API
morrow init-config [--project]
```

### Sessions & threads

- Every project keeps its threads under `.morrow/sessions/<character>/<id>.json`,
  scoped by **character + project** (nothing leaks across characters).
- On launch inside a project, any live conversation is snapshotted and a fresh thread
  begins; `/sessions`, `/resume`, and `/new` manage them.
- Global character chats (outside a project) live in the character's own JSON file.
- Automatic compaction runs at 85% of the model's `context_window`
  (`compact_threshold` in config; `/compact` forces it).

### Scheduling

Characters manage their own schedules via natural language — directly in chat or through
the `schedule` tool. Examples that parse:

```
at 12am                       once, next occurrence
tomorrow at 9:30am            once
in 30 minutes / in 2 hours    once, relative
every day at 6pm              daily (device-local time)
every weekday at 8am          Mon–Fri
every monday at 9am           weekly
```

Due tasks run **autonomously** and their results are appended to the character's memory.
Drive them with `morrow tick` (one pass), `morrow daemon` (loop), or the
built-in 30s scheduler inside `morrow serve`.

### Project rules & memory

The system prompt is assembled from, in precedence order:

1. Global rules — `~/.config/morrow/rules/*.md`
2. Repo-root → cwd walk — `MORROW.md`, `AGENTS.md`, `CLAUDE.md` per directory
   (interop with other agent conventions)
3. Project rules — `<root>/.morrow/rules/*.md`

Plus **project memory** (`.morrow/MEMORY.md`) — durable learnings the character writes
itself (`/flush`, `remember scope="project"`). It's explicitly *safe to commit* so the
whole team's agents benefit. Rules are capped before injection; `/rules` shows what's live.

### Custom slash commands

Drop a Markdown file into `.morrow/commands/<name>.md` (project) or
`~/.config/morrow/commands/<name>.md` (global). Optional frontmatter sets the
autocomplete description; `$ARGUMENTS` injects the trailing text:

```markdown
---
description: Review a diff for risks
---
Review $ARGUMENTS for security risks and suggest concrete fixes with file:line evidence.
```
---

## Built-in tools

| Tool | Kind | Purpose |
|------|------|---------|
| `bash` | mutating | run a shell command (stdout+stderr, configurable timeout) |
| `read` | read-only | read a file with line numbers (`offset`/`limit`) |
| `write` | mutating | create/overwrite a file (parent dirs auto-created) |
| `edit` | mutating | exact-match text replacement (fails on ambiguity) |
| `glob` | read-only | find files by glob pattern, newest first |
| `grep` | read-only | regex search over files (respects `.gitignore`) |
| `ls` | read-only | list a directory with entry types |
| `webfetch` | read-only | fetch a URL and return its body as text |
| `todo` | mutating | maintain the session's task list |
| `remember` / `recall` | mutating | durable memory, `character` or `project` scope |
| `integration` | mutating | manage the character's own third-party API integrations (keys resolved from `$ENV_VAR`, masked everywhere) |
| `schedule` | mutating | schedule tasks in natural language |
| `task` | mutating | delegate to a subagent |
| `mcp_*` | varies | tools exposed by configured MCP servers |

Tool specs are declared as JSON schema and exposed to the model over the configured
provider protocol.

---

## HTTP API server

```bash
morrow serve --host 127.0.0.1 --port 8471
```

A per-character REST API with **agent-scoped bearer keys**. Bootstrap rule: while a
character has no keys it is open (so you can issue the first key); afterwards every
request must present a key owned by the `{agent}` in the path. Keys are generated as
`dvk_...`, stored only as **SHA-256 hashes**, and shown in plaintext **once**.

| Method & Path | Description |
|---------------|-------------|
| `GET /health` | liveness |
| `POST /v1/{agent}/chat` | send a message (`message`/`prompt`/`text`), returns reply + usage |
| `GET /v1/{agent}/messages` | thread history |
| `DELETE /v1/{agent}/messages` | clear history |
| `GET /v1/{agent}` | character info |
| `POST /v1/{agent}/settings` | update settings |
| `GET /v1/{agent}/memory` | list memories |
| `POST /v1/{agent}/memory` | add a memory |
| `DELETE /v1/{agent}/memory` | delete a memory |
| `GET /v1/{agent}/schedules` | list schedules |
| `POST /v1/{agent}/schedules` | add a schedule |
| `DELETE /v1/{agent}/schedules/{id}` | remove a schedule |
| `GET /v1/{agent}/keys` | list keys |
| `POST /v1/{agent}/keys` | issue a key |
| `DELETE /v1/{agent}/keys/{id}` | revoke a key |

The server also runs a built-in scheduler (30s tick) while serving.

---

## Security

morrow was designed around a few hard rules:

- **Config keys are env-var references.** `api_key = "$OPENAI_API_KEY"` never lands in a
  repo or a JSON state file; literal keys in `.morrow/config.toml` are the user's choice
  and that file is gitignored.
- **Keys are masked.** Literal integration keys are rendered to the model/UI as
  `sk…******` (`Integration::masked_key()`); prompt assembly uses masked values only.
- **API keys are hashed.** The server issues `dvk_*` keys but stores only SHA-256
  digests; plaintext is shown once at issue time.
- **Agent-scoped auth.** A token only ever grants access to the character that owns it;
  mismatches are rejected with a 403.
- **Permission gate.** All mutating tools go through `ApprovalHandler`; `deny` entries
  in `[permissions]` and `bash_deny` shorthands always win, even in autonomous mode.
- **Nothing sensitive is committed.** See [`.gitignore`](.gitignore): local config,
  sessions, and state are excluded; the documented template
  (`.morrow/config.example.toml`) is what you share.

> ⚠️ This is **experimental** software. Granting an agent shell access is inherently
> risky — audit your `[permissions]` and `bash_deny` before running with `--auto`.
---

## Architecture

```
                      ┌───────────────────────────┐
                      │        terminal           │
                      │  (TUI / CLI / REST client)│
                      └─────────────┬─────────────┘
                                    │
              ┌─────────────────────▼─────────────────────┐
              │              morrow-cli               │
              │   clap CLI, subcommands, session wiring   │
              └─────────────────────┬─────────────────────┘
          ┌─────────────────────────┼─────────────────────────┐
          │                                                 │
┌─────────▼──────────┐                           ┌──────────▼─────────┐
│  morrow-tui    │                           │ morrow-server  │
│  ratatui interface │                           │ axum REST API      │
└─────────┬──────────┘                           │ bearer-key auth    │
          │                                      └──────────┬─────────┘
          │                                                 │
          └────────────────────────┬────────────────────────┘
                                   ▼
                    ┌─────────────────────────────┐
                    │       morrow-agent      │
                    │  Runner loop, tool dispatch,│
                    │  approvals, subagents, sched│
                    └───────┬─────────────┬───────┘
                            │             │
              ┌─────────────▼───┐   ┌─────▼──────────────┐
              │ morrow-tools│   │ morrow-provider│
              │  built-ins + MCP│   │ OpenAI / Responses │
              │  registry       │   │ / Anthropic, SSE   │
              └─────────────┬───┘   └─────┬──────────────┘
                            │             │
                            ▼             ▼
                    ┌─────────────────────────────┐
                    │      morrow-core        │
                    │ characters, config, memory, │
                    │ sessions, rules, schedules, │
                    │ messages                    │
                    └─────────────────────────────┘
```

## Repository layout

```
morrow/
├── Cargo.toml                    # workspace manifest (MIT, shared deps)
├── package.json                  # convenience scripts (yarn build/dev/ui)
├── .cargo/config.toml            # cargo aliases (dev/serve/ui)
├── .morrow/
│   ├── config.example.toml       # committed, documented config template
│   └── .gitignore                # local config/sessions/state never committed
└── crates/
    ├── morrow-core/          # characters, config, message types, memory,
    │                             # sessions, rules, schedules, API-key store
    ├── morrow-provider/      # LlmProvider trait + OpenAI/Chat/Responses,
    │                             # Anthropic wire protocols, SSE parser
    ├── morrow-tools/         # built-in tools + MCP client + registry
    ├── morrow-agent/         # Runner loop, approvals, prompt assembly,
    │                             # subagents, scheduled-task execution
    ├── morrow-tui/           # ratatui terminal UI
    ├── morrow-server/        # axum REST API with bearer-key auth
    └── morrow-cli/           # `morrow` binary (clap)
```

---

## Development

```bash
# Build everything
cargo build --workspace

# Run the full test suite (unit + integration tests in every crate)
cargo test --workspace

# Run just the CLI's tests
cargo test -p morrow-cli

# Format & lint
cargo fmt --all
cargo clippy --workspace --all-targets

# Fast release build
cargo build --release -p morrow-cli
```

The release profile is already tuned (`lto = true`, `codegen-units = 1`, `strip = true`).

### Adding a tool

1. Implement `morrow_tools::Tool` (name, description, JSON-schema `parameters`,
   `kind`, and `async fn run`).
2. Register it in `crates/morrow-tools/src/lib.rs::builtin_registry`.
3. Add an integration test in `crates/morrow-tools/tests/tools.rs`.

### Adding a provider protocol

1. Implement `morrow_provider::LlmProvider` (one streaming `stream` method).
2. Add a `Protocol` variant in `crates/morrow-core/src/config.rs`.
3. Wire the constructor in `morrow_provider::create`.

---

## Contributing

Contributions are welcome! Open an issue first for anything non-trivial, then open a PR.

- Keep Rust code idiomatic: `cargo fmt`, `cargo clippy`, no warnings.
- Cover behavior with tests — the workspace has unit tests in every crate and an
  integration test suite in `morrow-tools`.
- Never commit API keys. Follow the env-var convention (`api_key = "$NAME"`) and make
  sure local state stays out of your diff (see [Security](#security)).
- Update `.morrow/config.example.toml` and this README when you change user-facing config.

## License

[MIT](LICENSE) © morrow contributors
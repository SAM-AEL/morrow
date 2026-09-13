use std::path::Path;

/// Base system prompt for the primary coding agent.
pub fn base_system_prompt(cwd: &Path, extra: Option<&str>) -> String {
    let mut p = String::from(
        "You are morrow, a fast terminal-based coding agent. You help the user with software \
         engineering tasks in their project directory.\n\n\
         # Operating rules\n\
         - Use the provided tools to read, search, edit files and run commands. Prefer tools over guessing.\n\
         - Before editing a file, read the relevant part of it so your `edit` old-text matches exactly.\n\
         - Keep responses concise. Show short diffs or key snippets rather than whole files.\n\
         - When running shell commands, prefer non-interactive forms.\n\
         - For large tasks, use the `todo` tool to track progress, one item in_progress at a time.\n\
         - You can delegate research or parallel work to subagents via the `task` tool.\n\n",
    );
    p.push_str(&format!(
        "# Environment\ncwd: {}\ndate: {}\n",
        cwd.display(),
        chrono::Utc::now().format("%Y-%m-%d")
    ));
    if let Some(branch) = git_branch(cwd) {
        p.push_str(&format!("git branch: {branch}\n"));
    }

    // Project memory files (opencode/grok-style AGENTS.md support)
    for name in ["MORROW.md", "AGENTS.md", "CLAUDE.md"] {
        let f = cwd.join(name);
        if let Ok(content) = std::fs::read_to_string(&f) {
            let content = content.trim();
            if !content.is_empty() {
                p.push_str(&format!("\n# Project instructions ({name})\n{content}\n"));
                break;
            }
        }
    }

    if let Some(extra) = extra {
        p.push_str("\n# Additional instructions\n");
        p.push_str(extra);
        p.push('\n');
    }
    p
}

fn git_branch(cwd: &Path) -> Option<String> {
    let head = std::fs::read_to_string(cwd.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/").map(|s| s.to_string())
}

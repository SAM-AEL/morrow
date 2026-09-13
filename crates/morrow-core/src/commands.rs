use std::fs;
use std::path::{Path, PathBuf};

/// A user-defined slash command: `.morrow/commands/<name>.md` (project) or
/// `~/.config/morrow/commands/<name>.md` (global).
///
/// Optional frontmatter sets metadata:
///
/// ```md
/// ---
/// description: Review this diff for risks
/// ---
/// Review $ARGUMENTS for security risks and suggest fixes.
/// ```
///
/// `$ARGUMENTS` is replaced with the text after the command name; when the
/// template has no placeholder, the arguments are appended.
#[derive(Debug, Clone)]
pub struct CustomCommand {
    pub name: String,
    pub description: String,
    pub scope: &'static str,
    pub template: String,
}

impl CustomCommand {
    pub fn render(&self, args: &str) -> String {
        let args = args.trim();
        if self.template.contains("$ARGUMENTS") {
            self.template.replace("$ARGUMENTS", args)
        } else if args.is_empty() {
            self.template.clone()
        } else {
            format!("{}\n\n{args}", self.template)
        }
    }
}

/// Load project commands first, then global ones (project wins on collision).
pub fn load_custom_commands(project_root: &Path) -> Vec<CustomCommand> {
    let mut out: Vec<CustomCommand> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let global = dirs::config_dir()
        .map(|d| d.join("morrow").join("commands"))
        .unwrap_or_else(|| PathBuf::from("."));
    for (dir, scope) in [
        (project_root.join(".morrow").join("commands"), "project"),
        (global, "global"),
    ] {
        for cmd in read_dir_commands(&dir, scope) {
            if seen.insert(cmd.name.clone()) {
                out.push(cmd);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_dir_commands(dir: &Path, scope: &'static str) -> Vec<CustomCommand> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        if name.is_empty() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let (description, template) = split_frontmatter(&raw);
        let template = template.trim().to_string();
        if template.is_empty() {
            continue;
        }
        let description = if description.is_empty() {
            format!("custom command /{name}")
        } else {
            description
        };
        out.push(CustomCommand {
            name,
            description,
            scope,
            template,
        });
    }
    out
}

fn split_frontmatter(raw: &str) -> (String, String) {
    let mut lines = raw.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (String::new(), raw.to_string());
    }
    let mut desc = String::new();
    let mut body_start = 0;
    for (i, line) in raw.lines().enumerate().skip(1) {
        if line.trim() == "---" {
            body_start = i + 1;
            break;
        }
        if let Some(v) = line.strip_prefix("description:").map(str::trim) {
            desc = v.trim_matches('"').trim_matches('\'').to_string();
        }
    }
    if body_start == 0 {
        return (String::new(), raw.to_string());
    }
    let body: String = raw.lines().skip(body_start).collect::<Vec<_>>().join("\n");
    (desc, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_renders_args() {
        let (d, t) =
            split_frontmatter("---\ndescription: Review stuff\n---\nReview $ARGUMENTS now");
        assert_eq!(d, "Review stuff");
        let cmd = CustomCommand {
            name: "review".into(),
            description: d,
            scope: "project",
            template: t,
        };
        assert_eq!(cmd.render("main.rs"), "Review main.rs now");
        let plain = CustomCommand {
            name: "x".into(),
            description: String::new(),
            scope: "project",
            template: "Do it".into(),
        };
        assert_eq!(plain.render("fast"), "Do it\n\nfast");
        assert_eq!(plain.render(""), "Do it");
    }

    #[test]
    fn loads_project_over_global() {
        let root = std::env::temp_dir().join(format!("morrow-cmds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join(".morrow").join("commands");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("review.md"),
            "---\ndescription: R\n---\nReview $ARGUMENTS",
        )
        .unwrap();
        let cmds = load_custom_commands(&root);
        assert!(cmds
            .iter()
            .any(|c| c.name == "review" && c.scope == "project"));
        let _ = fs::remove_dir_all(&root);
    }
}

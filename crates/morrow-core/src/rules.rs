use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Project rules: markdown instruction files the agent loads into its system
/// prompt. morrow's own name is `MORROW.md`; `AGENTS.md` and `CLAUDE.md`
/// are honored for interop with other tools' conventions.
///
/// Discovery order (later = higher precedence in the prompt):
/// 1. global `~/.config/morrow/rules/*.md` (alphabetical)
/// 2. repo-root → cwd walk: `MORROW.md`, `AGENTS.md`, `CLAUDE.md` per directory
/// 3. `<root>/.morrow/rules/*.md` (alphabetical)
#[derive(Debug, Clone, Default)]
pub struct ProjectRules {
    /// (display path, content) in precedence order.
    pub files: Vec<(String, String)>,
}

impl ProjectRules {
    pub fn load(cwd: &Path) -> Self {
        let mut files: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();

        let push_file =
            |path: PathBuf, files: &mut Vec<(String, String)>, seen: &mut HashSet<PathBuf>| {
                if !path.is_file() {
                    return;
                }
                // Dedupe case-insensitive filesystem collisions (Agents.md == AGENTS.md).
                let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
                if !seen.insert(canon) {
                    return;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    let content = content.trim().to_string();
                    if !content.is_empty() {
                        files.push((path.display().to_string(), content));
                    }
                }
            };

        // 1. global rules
        if let Some(mut dir) = dirs::config_dir() {
            dir.push("morrow");
            dir.push("rules");
            if dir.is_dir() {
                for p in sorted_md(&dir) {
                    push_file(p, &mut files, &mut seen);
                }
            }
        }

        // 2 + 3. repo-root → cwd walk (plain cwd when outside a repo)
        let root = git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut dir = cwd.to_path_buf();
        loop {
            chain.push(dir.clone());
            if dir == root || !dir.starts_with(&root) {
                break;
            }
            match dir.parent() {
                Some(p) => dir = p.to_path_buf(),
                None => break,
            }
        }
        chain.reverse();
        const NAMES: &[&str] = &["MORROW.md", "AGENTS.md", "CLAUDE.md"];
        for d in &chain {
            for name in NAMES {
                push_file(d.join(name), &mut files, &mut seen);
            }
        }
        // 3. project rules dir at the root
        for p in sorted_md(&root.join(".morrow").join("rules")) {
            push_file(p, &mut files, &mut seen);
        }

        Self { files }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Total characters across all rule files.
    pub fn total_chars(&self) -> usize {
        self.files.iter().map(|(_, c)| c.len()).sum()
    }

    /// Prompt section, capped at `cap` chars (whole files until the cap).
    pub fn prompt_section(&self, cap: usize) -> String {
        if self.files.is_empty() {
            return String::new();
        }
        let mut out = String::from("\n# Project rules (follow for every interaction)\n");
        let mut used = 0;
        for (path, content) in &self.files {
            if used >= cap {
                out.push_str(&format!(
                    "\n- ({path} and others omitted: rule budget exceeded)\n"
                ));
                break;
            }
            let room = cap - used;
            if content.len() <= room {
                out.push_str(&format!("\n## {path}\n{content}\n"));
                used += content.len();
            } else {
                out.push_str(&format!(
                    "\n## {path}\n{}\n…(truncated)\n",
                    &content[..room]
                ));
                used = cap;
            }
        }
        out
    }
}

fn sorted_md(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Nearest ancestor (or self) containing `.git`, else None.
fn git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("morrow-rules-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&r);
        r
    }

    #[test]
    fn discovers_root_to_cwd_with_precedence() {
        let root = tmp_root("chain");
        let sub = root.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("MORROW.md"), "root rules").unwrap();
        fs::write(sub.join("AGENTS.md"), "sub rules").unwrap();
        fs::create_dir_all(root.join(".morrow").join("rules")).unwrap();
        fs::write(root.join(".morrow").join("rules").join("02-b.md"), "b").unwrap();
        fs::write(root.join(".morrow").join("rules").join("01-a.md"), "a").unwrap();

        let rules = ProjectRules::load(&sub);
        let paths: Vec<&str> = rules.files.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with("MORROW.md")), "{paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("AGENTS.md")), "{paths:?}");
        // root file precedes deeper file; rules dir last + alphabetical
        let morrow = paths.iter().position(|p| p.ends_with("MORROW.md")).unwrap();
        let agents = paths.iter().position(|p| p.ends_with("AGENTS.md")).unwrap();
        let a01 = paths.iter().position(|p| p.ends_with("01-a.md")).unwrap();
        let b02 = paths.iter().position(|p| p.ends_with("02-b.md")).unwrap();
        assert!(morrow < agents && agents < a01 && a01 < b02);

        let section = rules.prompt_section(1_000_000);
        assert!(section.contains("root rules") && section.contains("sub rules"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prompt_section_caps_content() {
        let root = tmp_root("cap");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("MORROW.md"), "x".repeat(500)).unwrap();
        let rules = ProjectRules::load(&root);
        assert!(rules.prompt_section(100).contains("truncated"));
        let _ = fs::remove_dir_all(&root);
    }
}

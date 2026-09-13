use std::fs;
use std::path::{Path, PathBuf};

/// Project memory: `.morrow/MEMORY.md` — curated learnings the agent writes
/// and reads back on every turn. Character memory (global JSON) stays personal;
/// project memory stays with the repo and is safe to commit.
pub fn memory_path(project_root: &Path) -> PathBuf {
    project_root.join(".morrow").join("MEMORY.md")
}

const HEADER: &str = "# Project memory\n\nDurable learnings about this project, written by morrow.\nEntries are newest-last. Safe to commit.\n";

/// Read project memory, capped at `cap` chars (tail kept: newest wins).
pub fn read_project_memory(project_root: &Path, cap: usize) -> String {
    let path = memory_path(project_root);
    let Ok(raw) = fs::read_to_string(&path) else {
        return String::new();
    };
    let raw = raw.trim().to_string();
    if raw.len() <= cap {
        return raw;
    }
    // Keep the header line + the tail.
    let mut lines: Vec<&str> = raw.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut len = 0;
    for l in lines.drain(..).rev() {
        if len + l.len() + 1 > cap {
            break;
        }
        len += l.len() + 1;
        kept.push(l);
    }
    kept.reverse();
    format!(
        "{}\n…(older entries trimmed)…\n{}",
        HEADER.trim(),
        kept.join("\n")
    )
}

/// Append one entry under today's heading, creating the file when missing.
/// Returns the file path for confirmations.
pub fn append_project_memory(project_root: &Path, text: &str) -> crate::error::Result<String> {
    let path = memory_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        fs::write(&path, HEADER)?;
    }
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let raw = fs::read_to_string(&path).unwrap_or_default();
    let heading = format!("## {today}");
    // Single facts become `- ` bullets; multi-line summaries keep their shape.
    let entry = if text.trim().contains('\n') {
        text.trim().to_string()
    } else {
        format!("- {}", text.trim().replace('\n', " / "))
    };
    let next = if raw.contains(&heading) {
        format!("{raw}\n{entry}\n")
    } else {
        format!("{raw}\n{heading}\n{entry}\n")
    };
    fs::write(&path, next)?;
    Ok(path.display().to_string())
}

/// Quick stats for `/memory`: (path, entry count, size).
pub fn memory_stats(project_root: &Path) -> Option<(String, usize, u64)> {
    let path = memory_path(project_root);
    let meta = fs::metadata(&path).ok()?;
    let raw = fs::read_to_string(&path).unwrap_or_default();
    let entries = raw.lines().filter(|l| l.starts_with("- ")).count();
    Some((path.display().to_string(), entries, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_groups_by_day_and_reads_back() {
        let root = std::env::temp_dir().join(format!("morrow-mem-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        append_project_memory(&root, "prefers tabs").unwrap();
        append_project_memory(&root, "deploys fridays").unwrap();
        let back = read_project_memory(&root, 10_000);
        assert!(back.contains("prefers tabs") && back.contains("deploys fridays"));
        let (path, entries, _) = memory_stats(&root).unwrap();
        assert_eq!(entries, 2);
        assert!(path.ends_with("MEMORY.md"));
        let capped = read_project_memory(&root, 20);
        assert!(capped.contains("trimmed"));
        let _ = fs::remove_dir_all(&root);
    }
}

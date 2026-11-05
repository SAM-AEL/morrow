mod bash;
mod edit;
mod glob_tool;
mod grep;
mod integration;
mod list;
pub mod mcp;
pub mod memory;
mod read;
pub mod registry;
pub mod schedule_tool;
mod todo;
pub use todo::TodoItem;
mod webfetch;
mod write;

pub use mcp::{McpClient, McpTool};
pub use registry::{Registry, Tool, ToolContext, ToolKind};

/// Truncate `s` to at most `max_bytes`, cutting on a character boundary.
///
/// A plain `&s[..max]` panics when the cut lands inside a multi-byte character,
/// which any real API response or web page can trigger. When the input is longer
/// than the budget the result is marked so the reader knows it was cut.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &s[..end])
}

/// Build a registry preloaded with every built-in tool.
pub fn builtin_registry() -> Registry {
    let mut r = Registry::new();
    r.add(bash::Bash);
    r.add(read::Read);
    r.add(write::Write);
    r.add(edit::Edit);
    r.add(glob_tool::GlobTool);
    r.add(grep::Grep);
    r.add(list::List);
    r.add(webfetch::WebFetch::new());
    r.add(todo::Todo);
    r.add(memory::Remember);
    r.add(memory::Recall);
    r.add(integration::IntegrationTool::new());
    r.add(schedule_tool::ScheduleTool);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_leaves_short_input_alone() {
        assert_eq!(truncate_utf8("hello", 100), "hello");
        assert_eq!(truncate_utf8("", 10), "");
    }

    #[test]
    fn truncate_marks_long_input() {
        let out = truncate_utf8(&"a".repeat(100), 10);
        assert!(out.starts_with(&"a".repeat(10)));
        assert!(out.ends_with("[truncated]"));
    }

    #[test]
    fn truncate_never_splits_a_character() {
        // The whole point: every cut point must land on a char boundary.
        for max in 0..40usize {
            let s = "héllo wörld 日本語 🎉 ok";
            let out = truncate_utf8(s, max);
            assert!(out.len() <= max + 20, "max={max} overflowed: {out:?}");
            // Must still be valid UTF-8 and preserve a prefix of the input.
            let prefix: String = out.chars().take_while(|c| *c != '…').collect();
            assert!(
                s.starts_with(&prefix),
                "max={max}: {prefix:?} is not a prefix of {s:?}"
            );
        }
    }

    #[test]
    fn truncate_handles_a_cut_landing_exactly_on_a_boundary() {
        // "é" is two bytes; a budget of 2 must keep it whole.
        let out = truncate_utf8("ééé", 2);
        assert!(out.starts_with('é'), "got {out:?}");
    }
}

use morrow_tools::ToolContext;
use serde_json::json;

fn ctx(dir: &std::path::Path) -> ToolContext {
    let mut c = ToolContext::new(dir);
    c.timeout_secs = 30;
    c
}

#[tokio::test]
async fn write_read_edit_roundtrip() {
    let dir = tempdir();
    let registry = morrow_tools::builtin_registry();
    let c = ctx(&dir);

    let out = registry
        .get("write")
        .unwrap()
        .run(
            &json!({
                "path": "a.txt",
                "content": "hello\nworld\n"
            }),
            &c,
        )
        .await
        .unwrap();
    assert!(out.contains("wrote"));

    let out = registry
        .get("read")
        .unwrap()
        .run(&json!({"path": "a.txt"}), &c)
        .await
        .unwrap();
    assert!(out.contains("1: hello"));
    assert!(out.contains("2: world"));

    let out = registry
        .get("edit")
        .unwrap()
        .run(
            &json!({
                "path": "a.txt", "old": "world", "new": "morrow"
            }),
            &c,
        )
        .await
        .unwrap();
    assert!(out.contains("edited"));

    let err = registry
        .get("edit")
        .unwrap()
        .run(
            &json!({
                "path": "a.txt", "old": "nope-not-here", "new": "x"
            }),
            &c,
        )
        .await;
    assert!(err.is_err());
}

#[tokio::test]
async fn edit_requires_unique_match() {
    let dir = tempdir();
    let registry = morrow_tools::builtin_registry();
    let c = ctx(&dir);
    registry
        .get("write")
        .unwrap()
        .run(
            &json!({
                "path": "b.txt", "content": "x\nx\n"
            }),
            &c,
        )
        .await
        .unwrap();
    let err = registry
        .get("edit")
        .unwrap()
        .run(
            &json!({
                "path": "b.txt", "old": "x", "new": "y"
            }),
            &c,
        )
        .await;
    assert!(err.is_err()); // ambiguous
    let ok = registry
        .get("edit")
        .unwrap()
        .run(
            &json!({
                "path": "b.txt", "old": "x", "new": "y", "replace_all": true
            }),
            &c,
        )
        .await
        .unwrap();
    assert!(ok.contains("2 replacement"));
}

#[tokio::test]
async fn grep_finds_matches() {
    let dir = tempdir();
    std::fs::write(dir.join("code.rs"), "fn main() {}\nlet answer = 42;\n").unwrap();
    let registry = morrow_tools::builtin_registry();
    let c = ctx(&dir);
    let out = registry
        .get("grep")
        .unwrap()
        .run(&json!({"pattern": "answer"}), &c)
        .await
        .unwrap();
    assert!(out.contains("code.rs:2"));
}

#[tokio::test]
async fn bash_runs_and_reports_exit() {
    let dir = tempdir();
    let registry = morrow_tools::builtin_registry();
    let c = ctx(&dir);
    let out = registry
        .get("bash")
        .unwrap()
        .run(&json!({"command": "echo hi"}), &c)
        .await
        .unwrap();
    assert!(out.contains("hi") && out.contains("(exit code: 0)"));
}

#[tokio::test]
async fn glob_lists_files() {
    let dir = tempdir();
    std::fs::write(dir.join("x.rs"), "").unwrap();
    std::fs::write(dir.join("y.rs"), "").unwrap();
    let registry = morrow_tools::builtin_registry();
    let c = ctx(&dir);
    let out = registry
        .get("glob")
        .unwrap()
        .run(&json!({"pattern": "*.rs"}), &c)
        .await
        .unwrap();
    assert!(out.contains("x.rs") && out.contains("y.rs"));
}

#[tokio::test]
async fn todo_updates_shared_state() {
    let dir = tempdir();
    let registry = morrow_tools::builtin_registry();
    let c = ctx(&dir);
    registry.get("todo").unwrap().run(&json!({
        "todos": [{"content":"task 1","status":"completed"},{"content":"task 2","status":"pending"}]
    }), &c).await.unwrap();
    let todos = c.todos.lock().unwrap().clone();
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].status, "completed");
}

fn tempdir() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = std::env::temp_dir().join(format!(
        "morrow-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

use morrow_core::message::{Message, Role};
use morrow_core::{ProjectSessions, Session, SessionStore};

#[test]
fn save_load_list_roundtrip() {
    let dir = std::env::temp_dir().join(format!("morrow-sessions-{}", std::process::id()));
    let store = SessionStore::with_dir(dir.clone());

    let mut s = Session::new(Some("fast".into()));
    s.push(Message::text(Role::User, "fix the bug in main.rs please"));
    s.push(Message::text(Role::Assistant, "on it"));
    store.save(&s).unwrap();

    let loaded = store.load(&s.id).unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.title, "fix the bug in main.rs please");

    let listed = store.list().unwrap();
    assert!(listed.iter().any(|x| x.id == s.id));
    let latest = store.latest().unwrap().unwrap();
    assert_eq!(latest.id, s.id);
}

#[test]
fn project_sessions_scope_by_character_and_prefix() {
    let root = std::env::temp_dir().join(format!("morrow-sessions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ps = ProjectSessions::new(&root);
    // .morrow footprint: sessions dir + gitignore for local-only threads
    assert!(root.join(".morrow").join(".gitignore").exists());

    let mut a = Session::new(None);
    a.character = "nova".into();
    a.push(Message::text(Role::User, "thread A"));
    ps.save(&a).unwrap();
    let mut b = Session::new(None);
    b.character = "nova".into();
    b.push(Message::text(Role::User, "thread B"));
    ps.save(&b).unwrap();
    let mut other = Session::new(None);
    other.character = "sam".into();
    other.push(Message::text(Role::User, "other character"));
    ps.save(&other).unwrap();

    // scoped listing
    let nova = ps.list("nova").unwrap();
    assert_eq!(nova.len(), 2);
    assert_eq!(ps.list("sam").unwrap().len(), 1);
    assert!(ps.list("nobody").unwrap().is_empty());

    // prefix resolution
    let hits = ps.resolve("nova", &a.id[..8]).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "thread A");

    // active pointer roundtrip (+ stale pointer ignored)
    assert!(ps.active_id("nova").is_none());
    ps.set_active_id("nova", &b.id).unwrap();
    assert_eq!(ps.active_id("nova").as_deref(), Some(b.id.as_str()));
    ps.set_active_id("nova", "deadbeef").unwrap();
    assert!(ps.active_id("nova").is_none());

    let _ = std::fs::remove_dir_all(&root);
}

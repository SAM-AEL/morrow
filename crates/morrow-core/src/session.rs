use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::message::Message;

/// A conversation session, persisted as JSON (messages + header).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// True once the user renames by hand — automatic titling never overrides.
    #[serde(default)]
    pub title_manual: bool,
    /// Owning character name (character + project scopes a thread).
    #[serde(default)]
    pub character: String,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(model: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            title: "new session".into(),
            title_manual: false,
            character: String::new(),
            model,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, msg: Message) {
        if self.title == "new session" && msg.role == crate::message::Role::User {
            let t = msg.plain_text();
            if !t.is_empty() {
                self.title = t.chars().take(60).collect();
            }
        }
        self.messages.push(msg);
        self.updated_at = Utc::now();
    }
}

/// Disk-backed store: ~/.local/share/morrow/sessions/<id>.json
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("morrow")
            .join("sessions");
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }

    pub fn save(&self, s: &Session) -> Result<()> {
        let path = self.dir.join(format!("{}.json", s.id));
        fs::write(path, serde_json::to_vec_pretty(s)?)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Option<Session>> {
        let path = if id.ends_with(".json") {
            PathBuf::from(id)
        } else {
            self.dir.join(format!("{id}.json"))
        };
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&raw)?))
    }

    pub fn list(&self) -> Result<Vec<Session>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(s) = serde_json::from_str::<Session>(&raw) {
                    out.push(s);
                }
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(out)
    }

    pub fn latest(&self) -> Result<Option<Session>> {
        Ok(self.list()?.into_iter().next())
    }
}

/// Project-local threads: `<project>/.morrow/sessions/<character>/<id>.json`
/// plus `<project>/.morrow/state.json` holding the active session per character.
///
/// A thread is scoped to (character, project): `nova` in `~/proj` has its own
/// threads, separate from `nova` anywhere else.
#[derive(Clone)]
pub struct ProjectSessions {
    dir: PathBuf,
    state_file: PathBuf,
}

impl ProjectSessions {
    pub fn new(project_root: &Path) -> Self {
        let base = project_root.join(".morrow");
        let dir = base.join("sessions");
        let _ = fs::create_dir_all(&dir);
        // Keep threads + pointers local; MEMORY.md stays committable.
        let gi = base.join(".gitignore");
        if !gi.exists() {
            let _ = fs::write(&gi, "sessions/\nstate.json\n");
        }
        Self {
            dir,
            state_file: base.join("state.json"),
        }
    }

    /// Filesystem-safe segment (mirrors CharacterStore sanitizing).
    fn safe(name: &str) -> String {
        name.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn char_dir(&self, character: &str) -> PathBuf {
        self.dir.join(Self::safe(character))
    }

    fn path_for(&self, character: &str, id: &str) -> PathBuf {
        self.char_dir(character)
            .join(format!("{}.json", Self::safe(id)))
    }

    pub fn save(&self, s: &Session) -> Result<()> {
        let dir = self.char_dir(&s.character);
        fs::create_dir_all(&dir)?;
        fs::write(
            self.path_for(&s.character, &s.id),
            serde_json::to_vec_pretty(s)?,
        )?;
        Ok(())
    }

    pub fn load_exact(&self, character: &str, id: &str) -> Result<Option<Session>> {
        let path = self.path_for(character, id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&fs::read_to_string(path)?)?))
    }

    /// All sessions whose id starts with `query` (exact first).
    pub fn resolve(&self, character: &str, query: &str) -> Result<Vec<Session>> {
        let all = self.list(character)?;
        let mut hits: Vec<Session> = all
            .into_iter()
            .filter(|s| s.id.starts_with(query))
            .collect();
        hits.sort_by(|a, b| {
            let a_exact = a.id == query;
            let b_exact = b.id == query;
            b_exact.cmp(&a_exact).then(b.updated_at.cmp(&a.updated_at))
        });
        Ok(hits)
    }

    pub fn list(&self, character: &str) -> Result<Vec<Session>> {
        let dir = self.char_dir(character);
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(s) = serde_json::from_str::<Session>(&raw) {
                    out.push(s);
                }
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(out)
    }

    pub fn delete(&self, character: &str, id: &str) -> Result<bool> {
        let p = self.path_for(character, id);
        if p.exists() {
            fs::remove_file(p)?;
            if self.active_id(character).as_deref() == Some(id) {
                let _ = self.set_active_id(character, "");
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn read_state(&self) -> serde_json::Value {
        if !self.state_file.exists() {
            return serde_json::json!({});
        }
        fs::read_to_string(&self.state_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    }

    /// Active (most recently written) session id for a character, if its file exists.
    pub fn active_id(&self, character: &str) -> Option<String> {
        let id = self.read_state()["active"][character].as_str()?.to_string();
        if id.is_empty() || !self.path_for(character, &id).exists() {
            return None;
        }
        Some(id)
    }

    pub fn set_active_id(&self, character: &str, id: &str) -> Result<()> {
        let mut v = self.read_state();
        if !v.is_object() {
            v = serde_json::json!({});
        }
        v["active"][character] = serde_json::Value::String(id.to_string());
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.state_file, serde_json::to_vec_pretty(&v)?)?;
        Ok(())
    }
}

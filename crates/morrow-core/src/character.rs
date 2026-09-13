use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::message::Message;

/// Operating mode of a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Mutating tools need explicit user approval.
    #[default]
    Approval,
    /// Fully autonomous: acts without asking.
    Autonomous,
}

/// An API key granting access to exactly one character.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub label: String,
    /// SHA-256 hex of the key; plaintext never stored.
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A learned fact the character chose to remember.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Millis-since-epoch id (stable across reloads).
    pub id: u64,
    pub at: DateTime<Utc>,
    pub text: String,
}

/// Third-party API the character manages itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Integration {
    pub description: String,
    pub base_url: String,
    /// Literal or "$ENV_VAR".
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl Integration {
    /// Key safe to show the model / UI (env refs stay, literals masked).
    pub fn masked_key(&self) -> String {
        match &self.api_key {
            None => "(none)".into(),
            Some(k) if k.starts_with('$') => k.clone(),
            Some(k) => format!("{}…{}", &k[..2.min(k.len())], "*".repeat(6)),
        }
    }
}

/// A persistent character: identity, memory, integrations, mode and its OWN chat.
/// Everything lives in one JSON file — fast to load, trivially portable/backupable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    #[serde(default = "default_version")]
    pub version: u8,
    pub name: String,
    /// Core personality / role definition.
    pub persona: String,
    /// Which saved model currently powers this character (key into config.models).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub memories: Vec<MemoryEntry>,
    #[serde(default)]
    pub integrations: BTreeMap<String, Integration>,
    /// This character's private conversation (never shared across characters).
    #[serde(default)]
    pub messages: Vec<Message>,
    /// API keys scoped to THIS character only.
    #[serde(default)]
    pub api_keys: Vec<ApiKey>,
    /// Scheduled tasks owned by this character.
    #[serde(default)]
    pub schedules: Vec<crate::schedule::Schedule>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_version() -> u8 {
    1
}

fn default_true() -> bool {
    true
}

impl Character {
    pub fn new(name: &str) -> Self {
        let now = Utc::now();
        Self {
            version: 1,
            name: name.to_string(),
            persona: default_persona(name),
            model: None,
            mode: Mode::Approval,
            memories: Vec::new(),
            integrations: BTreeMap::new(),
            messages: Vec::new(),
            api_keys: Vec::new(),
            schedules: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn remember(&mut self, text: impl Into<String>) {
        self.memories.push(MemoryEntry {
            id: Utc::now().timestamp_millis() as u64,
            at: Utc::now(),
            text: text.into(),
        });
        // keep bounded for speed
        const MAX_MEMORIES: usize = 500;
        if self.memories.len() > MAX_MEMORIES {
            let excess = self.memories.len() - MAX_MEMORIES;
            self.memories.drain(..excess);
        }
        self.touch();
    }

    pub fn recall(&self, query: &str) -> Vec<&MemoryEntry> {
        let q = query.to_lowercase();
        let q = q.trim();
        if q.is_empty() {
            return vec![];
        }
        self.memories
            .iter()
            .filter(|m| m.text.to_lowercase().contains(q))
            .collect()
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Build the character's system prompt: persona + world + memory + integrations.
    pub fn system_prompt(&self, cwd: &Path) -> String {
        let mut p = format!(
            "{}\n\n# Environment\ncwd: {}\ndate: {}\n",
            self.persona.trim(),
            cwd.display(),
            Utc::now().format("%Y-%m-%d")
        );
        if let Ok(head) = fs::read_to_string(cwd.join(".git").join("HEAD")) {
            if let Some(branch) = head.trim().strip_prefix("ref: refs/heads/") {
                p.push_str(&format!("git branch: {branch}\n"));
            }
        }

        // Project rules (MORROW.md / AGENTS.md / .morrow/rules/): always loaded,
        // capped so a rules-heavy repo can't eat the whole window.
        let rules = crate::rules::ProjectRules::load(cwd);
        if !rules.is_empty() {
            p.push_str(&rules.prompt_section(12_000));
        }

        p.push_str(
            "\n# Abilities\n\
             You run real tools: shell commands, file editing, web fetches. Use them rather than guessing.\n\
             You manage your own third-party API integrations via the `integration` tool\n\
             (add/list/remove/call). You decide what to remember about the user and your work via\n\
             the `remember` tool; everything you save lives in YOUR single memory file and survives\n\
             forever. Stay in character at all times.\n",
        );

        match self.mode {
            Mode::Autonomous => p.push_str(
                "\n# Mode: AUTONOMOUS\nYou act without asking for permission. Execute plans end-to-end.\n",
            ),
            Mode::Approval => p.push_str(
                "\n# Mode: APPROVAL\nDestructive/mutating actions will be confirmed with the user first;\nplan accordingly and batch proposals.\n",
            ),
        }

        if !self.memories.is_empty() {
            p.push_str("\n# Your memory (most recent learnings)\n");
            for m in self
                .memories
                .iter()
                .rev()
                .take(30)
                .collect::<Vec<_>>()
                .iter()
                .rev()
            {
                p.push_str(&format!("- {}\n", m.text));
            }
        }

        // Project memory (.morrow/MEMORY.md): shared with the repo, newest kept.
        let pmem = crate::pmemory::read_project_memory(cwd, 2_000);
        if !pmem.trim().is_empty() {
            p.push_str("\n# Project memory (.morrow/MEMORY.md — commit-worthy learnings)\n");
            p.push_str(&pmem);
            p.push('\n');
        }

        if !self.integrations.is_empty() {
            p.push_str("\n# Your API integrations\n");
            for (name, i) in &self.integrations {
                p.push_str(&format!(
                    "- {name}: {} @ {} (key: {})\n",
                    i.description,
                    i.base_url,
                    i.masked_key()
                ));
            }
        }
        p
    }
}

fn default_persona(name: &str) -> String {
    format!(
        "You are {name}, an autonomous AI character with your own persistent memory, personality \
         and private chat history. You are not a generic assistant: you are {name}. You learn from \
         every interaction by saving durable facts about the user, projects and yourself. You can \
         write and run code autonomously and manage third-party APIs on your own."
    )
}

/// Disk store: one JSON file per character under `<dir>/<name>.json`.
#[derive(Clone)]
pub struct CharacterStore {
    dir: PathBuf,
}

impl Character {
    // ---- api keys ----------------------------------------------------------

    /// Issue a new key. Returns `(entry, plaintext)`; plaintext is shown ONCE.
    pub fn issue_key(&mut self, label: &str) -> Result<(ApiKey, String)> {
        use sha2::{Digest, Sha256};
        let raw: [u8; 24] = rand::random();
        let plaintext = format!(
            "dvk_{}",
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let entry = ApiKey {
            id: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            label: label.to_string(),
            key_hash: format!("{:x}", Sha256::digest(plaintext.as_bytes())),
            created_at: Utc::now(),
            last_used: None,
            enabled: true,
        };
        if self.api_keys.len() >= 50 {
            self.api_keys.remove(0);
        }
        self.api_keys.push(entry.clone());
        self.touch();
        Ok((entry, plaintext))
    }

    /// Verify a presented key against this character's hashes.
    pub fn verify_key(&self, presented: &str) -> Option<String> {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(presented.as_bytes()));
        self.api_keys
            .iter()
            .find(|k| k.enabled && k.key_hash == hash)
            .map(|k| k.id.clone())
    }

    pub fn revoke_key(&mut self, id: &str) -> bool {
        let before = self.api_keys.len();
        self.api_keys.retain(|k| k.id != id);
        let removed = self.api_keys.len() != before;
        if removed {
            self.touch();
        }
        removed
    }
}

impl CharacterStore {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("morrow")
            .join("agents");
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }

    fn path_for(&self, name: &str) -> PathBuf {
        // sanitize: keep filesystem-safe
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path_for(name).exists()
    }

    pub fn save(&self, c: &mut Character) -> Result<()> {
        c.touch();
        fs::write(self.path_for(&c.name), serde_json::to_vec_pretty(c)?)?;
        Ok(())
    }

    /// Load or create-on-first-use.
    pub fn load_or_create(&self, name: &str) -> Result<Character> {
        let path = self.path_for(name);
        if path.exists() {
            let raw = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&raw)?)
        } else {
            let mut c = Character::new(name);
            self.save(&mut c)?;
            Ok(c)
        }
    }

    pub fn load(&self, name: &str) -> Result<Option<Character>> {
        let path = self.path_for(name);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&fs::read_to_string(path)?)?))
    }

    pub fn list(&self) -> Result<Vec<Character>> {
        let mut out = Vec::new();
        for e in fs::read_dir(&self.dir)? {
            let p = e?.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(c) = serde_json::from_str::<Character>(&fs::read_to_string(p)?) {
                out.push(c);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn delete(&self, name: &str) -> Result<bool> {
        let p = self.path_for(name);
        if p.exists() {
            fs::remove_file(p)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_roundtrip_isolated_per_name() {
        let dir = std::env::temp_dir().join(format!("morrow-chars-{}", std::process::id()));
        let store = CharacterStore::with_dir(dir);

        let mut sam = store.load_or_create("Sam").unwrap();
        sam.persona = "I am Sam.".into();
        sam.remember("user prefers rust");
        sam.integrations.insert(
            "github".into(),
            Integration {
                description: "GitHub API".into(),
                base_url: "https://api.github.com".into(),
                api_key: Some("$GITHUB_TOKEN".into()),
                headers: Default::default(),
            },
        );
        store.save(&mut sam).unwrap();

        let mut nova = store.load_or_create("Nova").unwrap();
        assert!(nova.memories.is_empty()); // never mixed

        let reloaded = store.load("Sam").unwrap().unwrap();
        assert_eq!(reloaded.persona, "I am Sam.");
        assert_eq!(reloaded.memories[0].text, "user prefers rust");
        assert_eq!(
            reloaded.integrations["github"].masked_key(),
            "$GITHUB_TOKEN"
        );

        let names: Vec<_> = store.list().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["Nova", "Sam"]);
    }

    #[test]
    fn recall_matches_case_insensitive() {
        let mut c = Character::new("t");
        c.remember("Loves Rust and coffee");
        assert_eq!(c.recall("rust").len(), 1);
        assert_eq!(c.recall("python").len(), 0);
    }

    #[test]
    fn system_prompt_contains_sections() {
        let mut c = Character::new("Sam");
        c.remember("knows the deploy script");
        c.integrations.insert(
            "linear".into(),
            Integration {
                description: "issue tracker".into(),
                base_url: "https://api.linear.app".into(),
                api_key: Some("lin_api_abcdef123".into()),
                headers: Default::default(),
            },
        );
        let p = c.system_prompt(Path::new("/tmp"));
        assert!(p.contains("You are Sam"));
        assert!(p.contains("knows the deploy script"));
        assert!(p.contains("linear"));
        // literal key must be masked in prompts
        assert!(!p.contains("lin_api_abcdef123"));
    }
}

impl CharacterStore {
    /// Canonical single-file location for a character.
    pub fn file_for(&self, name: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }
}

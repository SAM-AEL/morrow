use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use morrow_agent::Runner;
use morrow_core::{CharacterStore, Config};

/// Shared server state.
pub struct ServerState {
    pub config: Arc<Config>,
    pub store: CharacterStore,
    /// One runner per active character; chats serialize per character,
    /// run in parallel across characters.
    pub runners: Mutex<HashMap<String, Arc<Mutex<Runner>>>>,
    /// sha256(key) -> agent name. Rescanned on miss so keys issued via
    /// CLI/TUI while the server runs are picked up automatically.
    pub key_index: RwLock<HashMap<String, String>>,
}

impl ServerState {
    pub fn new(config: Arc<Config>, store: CharacterStore) -> Arc<Self> {
        Arc::new(Self {
            config,
            store,
            runners: Mutex::new(HashMap::new()),
            key_index: RwLock::new(HashMap::new()),
        })
    }

    /// (Re)build the hash->agent index from every character file.
    pub async fn rebuild_key_index(&self) -> usize {
        let mut idx = HashMap::new();
        if let Ok(chars) = self.store.list() {
            for c in chars {
                for k in &c.api_keys {
                    if k.enabled {
                        idx.insert(k.key_hash.clone(), c.name.clone());
                    }
                }
            }
        }
        let n = idx.len();
        *self.key_index.write().await = idx;
        n
    }

    /// Resolve an agent name from a presented bearer token.
    /// Returns None if unknown; rescans once before giving up.
    pub async fn resolve_key(&self, presented: &str) -> Option<String> {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(presented.as_bytes()));

        {
            let idx = self.key_index.read().await;
            if let Some(agent) = idx.get(&hash) {
                return Some(agent.clone());
            }
        }
        // miss: rescan (picks up externally-issued keys), then retry
        self.rebuild_key_index().await;
        let idx = self.key_index.read().await;
        idx.get(&hash).cloned()
    }

    /// Get-or-create the live runner for a character.
    pub async fn runner_for(self: &Arc<Self>, agent: &str) -> anyhow::Result<Arc<Mutex<Runner>>> {
        let mut map = self.runners.lock().await;
        if let Some(r) = map.get(agent) {
            return Ok(r.clone());
        }
        let mut character = self.store.load_or_create(agent)?;
        if character.model.is_none() {
            character.model = self.config.default_model.clone();
        }
        let approval = Arc::new(morrow_agent::AutoApprove);
        let runner = Runner::with_store(
            self.store.clone(),
            self.config.clone(),
            std::env::current_dir().unwrap_or_default(),
            None,
            approval,
            character,
        );
        let arc = Arc::new(Mutex::new(runner));
        map.insert(agent.to_string(), arc.clone());
        Ok(arc)
    }
}

/// Execute due scheduled tasks using live runners where available
/// (avoids clobbering in-memory state with a stale disk copy).
pub async fn tick_active(state: &Arc<ServerState>) -> anyhow::Result<usize> {
    use chrono::Utc;

    let mut ran = 0usize;
    // 1) live runners
    let live: Vec<(String, Arc<Mutex<Runner>>)> = {
        let map = state.runners.lock().await;
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    for (_, runner_mutex) in live {
        let mut runner = runner_mutex.lock().await;
        let due: Vec<String> = runner
            .character
            .schedules
            .iter()
            .filter(|s| s.enabled && s.next_run <= Utc::now())
            .map(|s| s.id.clone())
            .collect();
        for id in due {
            let Some(sched) = runner.character.schedules.iter().find(|s| s.id == id) else {
                continue;
            };
            let task = sched.task.clone();
            eprintln!(
                "[schedule] {} running '{}'",
                runner.character_name(),
                sched.when_text
            );
            let mut noop = |_: morrow_agent::AgentEvent| {};
            if let Err(e) = runner.chat(&task, &mut noop).await {
                eprintln!("[schedule] task failed: {e}");
            } else {
                ran += 1;
            }
            if let Some(s) = runner.character.schedules.iter_mut().find(|s| s.id == id) {
                s.completed();
            }
        }
        if !runner.character.schedules.is_empty() {
            runner.save_character()?;
        }
    }

    // 2) characters not currently loaded
    let loaded: Vec<String> = state.runners.lock().await.keys().cloned().collect();
    for summary in state.store.list()? {
        if loaded.contains(&summary.name) {
            continue;
        }
        if !summary
            .schedules
            .iter()
            .any(|s| s.enabled && s.next_run <= Utc::now())
        {
            continue;
        }
        let runner = state.runner_for(&summary.name).await?;
        let mut r = runner.lock().await;
        let due: Vec<String> = r
            .character
            .schedules
            .iter()
            .filter(|s| s.enabled && s.next_run <= Utc::now())
            .map(|s| s.id.clone())
            .collect();
        for id in due {
            let Some(sched) = r.character.schedules.iter().find(|s| s.id == id) else {
                continue;
            };
            let task = sched.task.clone();
            let mut noop = |_: morrow_agent::AgentEvent| {};
            if r.chat(&task, &mut noop).await.is_ok() {
                ran += 1;
            }
            if let Some(s) = r.character.schedules.iter_mut().find(|s| s.id == id) {
                s.completed();
            }
        }
        r.save_character()?;
    }
    Ok(ran)
}

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use morrow_core::{CharacterStore, Config};

use crate::agent::Runner;
use crate::approval::AutoApprove;

/// Outcome of one executed scheduled task.
pub struct TickReport {
    pub character: String,
    pub task: String,
    pub result: String,
}

/// Scan every character's schedules and execute the ones that are due.
/// Scheduled runs always act autonomously (there is nobody to approve at 12am);
/// results are appended to the character's own memory/chat file.
pub async fn tick(
    store: &CharacterStore,
    config: &Arc<Config>,
    cwd: PathBuf,
) -> anyhow::Result<Vec<TickReport>> {
    let mut reports = Vec::new();

    for summary in store.list()? {
        let due_ids: Vec<String> = summary
            .schedules
            .iter()
            .filter(|s| s.enabled && s.next_run <= Utc::now())
            .map(|s| s.id.clone())
            .collect();
        if due_ids.is_empty() {
            continue;
        }

        let character = store.load(&summary.name)?.expect("listed but missing");
        let mut runner = Runner::with_store(
            store.clone(),
            config.clone(),
            cwd.clone(),
            None,
            Arc::new(AutoApprove),
            character,
        );
        runner.attach_project_sessions(cwd.clone());

        for id in due_ids {
            let Some(sched) = runner.character.schedules.iter().find(|s| s.id == id) else {
                continue;
            };
            let task = sched.task.clone();
            let when_text = sched.when_text.clone();
            eprintln!(
                "[schedule] {} running '{}' (due {})",
                runner.character_name(),
                when_text,
                sched.next_run.format("%Y-%m-%d %H:%M %Z")
            );
            let mut noop = |_: crate::AgentEvent| {};
            match runner.chat(&task, &mut noop).await {
                Ok(output) => reports.push(TickReport {
                    character: runner.character_name().to_string(),
                    task,
                    result: output,
                }),
                Err(e) => reports.push(TickReport {
                    character: runner.character_name().to_string(),
                    task,
                    result: format!("error: {e}"),
                }),
            }
            // advance recurrence / disable one-shots
            if let Some(s) = runner.character.schedules.iter_mut().find(|s| s.id == id) {
                s.completed();
            }
        }

        // remember that scheduled work happened
        let n = reports.len();
        if n > 0 {
            runner.character.remember(format!(
                "completed {n} scheduled task(s) around {}",
                Utc::now().format("%Y-%m-%d %H:%M UTC")
            ));
        }
        runner.save_character()?;
    }

    Ok(reports)
}

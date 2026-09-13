use morrow_provider::ToolSpec;
use serde_json::{json, Value};

use crate::agent::{AgentEvent, Runner};

/// Spec for the `task` tool exposed to the primary model.
pub fn task_spec() -> ToolSpec {
    ToolSpec {
        name: "task".to_string(),
        description: "Delegate a self-contained job to a subagent. Subagents run with their own \
                      context, prompt and toolset, and return only their final answer — ideal for \
                      codebase research and parallel investigation. Available agents are listed in \
                      the `agent` enum; omit it for the general-purpose agent."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "agent": {"type": "string", "description": "Subagent name (see available agents)"},
                "description": {"type": "string", "description": "One-line summary of the job"},
                "prompt": {"type": "string", "description": "Full self-contained instructions"}
            },
            "required": ["description", "prompt"]
        }),
    }
}

fn agent_list_markdown(cfg: &morrow_core::Config) -> String {
    let mut out = String::from("Available subagents:\n");
    for (name, a) in &cfg.agents {
        if a.hidden {
            continue;
        }
        let desc = a.description.clone().unwrap_or_default();
        out.push_str(&format!("- {name}: {desc}\n"));
    }
    out
}

/// Execute the `task` tool: spawn an isolated sub-runner and drive its loop.
pub async fn run_subagent(
    parent: &mut Runner,
    arguments: &str,
    on_event: &mut (dyn FnMut(AgentEvent) + Send),
) -> morrow_core::Result<String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let agent_name = args["agent"].as_str().unwrap_or("general").to_string();
    let prompt = args["prompt"].as_str().unwrap_or_default().to_string();
    let description = args["description"].as_str().unwrap_or("").to_string();

    let Some(agent_cfg) = parent.config.agents.get(&agent_name).cloned() else {
        return Ok(format!(
            "error: unknown agent '{agent_name}'. {}\n{}",
            agent_list_markdown(&parent.config),
            "(pass \"agent\": \"general\" to use the default)"
        ));
    };

    on_event(AgentEvent::SubagentStart {
        name: format!("{agent_name} ({description})"),
    });

    // Resolve the subagent's model (fall back to parent model / default).
    let model_key = agent_cfg
        .model
        .clone()
        .or_else(|| parent.model_key().map(String::from))
        .filter(|k| parent.config.find_model(k).is_some());

    // Fresh isolated session + restricted registry view.
    let mut sub = Runner::new_subagent(
        parent.config.clone(),
        parent.cwd().to_path_buf(),
        model_key,
        agent_cfg.tools.clone(),
        agent_cfg.prompt.clone(),
        agent_cfg.temperature,
        parent.provider_override(),
    );

    let mut events = |ev: AgentEvent| match ev {
        AgentEvent::TextDelta(d) => on_event(AgentEvent::TextDelta(d)),
        other => on_event(other),
    };

    let result = sub.chat(&prompt, &mut events).await;
    on_event(AgentEvent::SubagentEnd { name: agent_name });
    result
}

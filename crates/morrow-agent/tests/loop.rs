use std::sync::Arc;

use async_trait::async_trait;
use morrow_agent::{AgentEvent, Runner};
use morrow_core::message::Usage;
use morrow_core::Config;
use morrow_provider::{CompletionRequest, LlmProvider, StreamEvent};

/// Scripted provider: replays canned responses in order, records requests.
use std::sync::Mutex;

struct MockProvider {
    steps: std::sync::Mutex<Vec<Vec<StreamEvent>>>,
    seen_models: std::sync::Mutex<Vec<String>>,
}

impl MockProvider {
    fn new(steps: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            steps: Mutex::new(steps),
            seen_models: Default::default(),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn stream(
        &self,
        req: CompletionRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> morrow_core::Result<()> {
        self.seen_models.lock().unwrap().push(req.model.clone());
        let step = self.steps.lock().unwrap().remove(0);
        for ev in step {
            on_event(ev);
        }
        Ok(())
    }
}

fn test_config() -> Arc<Config> {
    let mut cfg = Config::builtin();
    cfg.providers.insert(
        "mock".into(),
        morrow_core::ProviderConfig {
            base_url: "http://localhost:0".into(),
            ..Default::default()
        },
    );
    cfg.models.insert(
        "test-model".into(),
        morrow_core::ModelConfig {
            id: "mock-1".into(),
            provider: "mock".into(),
            display_name: None,
            alias: None,
            context_window: Some(128_000),
            max_tokens: None,
            temperature: None,
        },
    );
    Arc::new(cfg)
}

fn tool_call_step() -> Vec<StreamEvent> {
    vec![
        StreamEvent::TextDelta("let me check.".into()),
        StreamEvent::ToolCallBegin {
            index: 0,
            id: "call1".into(),
            name: "bash".into(),
        },
        StreamEvent::ToolCallDelta {
            index: 0,
            args: "{\"command\":".into(),
        },
        StreamEvent::ToolCallDelta {
            index: 0,
            args: " \"echo agent-loop-ok\"}".into(),
        },
        StreamEvent::Finish {
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
            },
        },
    ]
}

#[tokio::test]
async fn agent_loop_executes_tools_and_finishes() {
    let mock = Arc::new(MockProvider::new(vec![
        tool_call_step(),
        vec![
            StreamEvent::TextDelta("done: ".into()),
            StreamEvent::TextDelta("all good".into()),
            StreamEvent::Finish {
                usage: Usage {
                    input_tokens: 20,
                    output_tokens: 3,
                },
            },
        ],
    ]));

    let config = test_config();
    let approval = Arc::new(morrow_agent::AutoApprove);
    let mut runner = Runner::new(
        config,
        std::env::temp_dir(),
        Some("test-model".into()),
        approval,
        morrow_core::Character::new("TestChar"),
    );
    runner.set_provider(mock.clone());

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let events2 = events.clone();
    let mut on_event = move |ev: AgentEvent| events2.lock().unwrap().push(ev);

    let answer = runner.chat("run a command", &mut on_event).await.unwrap();
    assert_eq!(answer, "done: all good");

    let evs = events.lock().unwrap();
    assert!(evs
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolCall { name, .. } if name == "bash")));
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::ToolResult { name, ok: true, result } if name == "bash" && result.contains("agent-loop-ok")
    )));
    // usage accumulated across both completions
    assert_eq!(runner.total_usage.total(), 38);
}

#[tokio::test]
async fn unknown_tool_returns_error_result_not_panic() {
    let mock = Arc::new(MockProvider::new(vec![
        vec![
            StreamEvent::TextDelta("hmm".into()),
            StreamEvent::ToolCallBegin {
                index: 0,
                id: "c".into(),
                name: "nonexistent".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                args: "{}".into(),
            },
            StreamEvent::Finish {
                usage: Usage::default(),
            },
        ],
        vec![StreamEvent::TextDelta("recovered".into())],
    ]));

    let mut runner = Runner::new(
        test_config(),
        std::env::temp_dir(),
        Some("test-model".into()),
        Arc::new(morrow_agent::AutoApprove),
        morrow_core::Character::new("TestChar"),
    );
    runner.set_provider(mock.clone());

    let mut noop = |_| {};
    let answer = runner.chat("go", &mut noop).await.unwrap();
    assert_eq!(answer, "recovered");
}

#[tokio::test]
async fn character_learns_via_remember_tool() {
    // model asks to remember, then finishes; memory must land in the character file
    let dir = std::env::temp_dir().join(format!(
        "morrow-char-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = morrow_core::CharacterStore::with_dir(dir.clone());
    let mut character = store.load_or_create("Sam").unwrap();

    let mock = Arc::new(MockProvider::new(vec![
        vec![
            StreamEvent::ToolCallBegin {
                index: 0,
                id: "m1".into(),
                name: "remember".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                args: "{\"text\":\"user hates tabs\"}".into(),
            },
            StreamEvent::Finish {
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 5,
                },
            },
        ],
        vec![
            StreamEvent::TextDelta("noted!".into()),
            StreamEvent::Finish {
                usage: Usage::default(),
            },
        ],
    ]));

    let config = test_config();
    let approval = Arc::new(morrow_agent::AutoApprove);
    character.mode = morrow_core::Mode::Autonomous;
    let mut runner = Runner::with_store(
        store.clone(),
        config,
        std::env::temp_dir(),
        Some("test-model".into()),
        approval,
        character,
    );
    runner.set_provider(mock);

    let evs: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let evs2 = evs.clone();
    let mut log = move |e: AgentEvent| match e {
        AgentEvent::ToolResult { name, ok, result } => evs2
            .lock()
            .unwrap()
            .push(format!("{name} ok={ok} {result}")),
        _ => {}
    };
    let answer = runner
        .chat("remember my preference", &mut log)
        .await
        .unwrap();
    assert_eq!(answer, "noted!");
    for l in evs.lock().unwrap().iter() {
        eprintln!("EVT {l}");
    }

    // persisted to the single-file store, isolated from other characters
    let reloaded = store.load("Sam").unwrap().unwrap();
    assert!(reloaded
        .memories
        .iter()
        .any(|m| m.text == "user hates tabs"));
    assert!(store.load("Nova").unwrap().is_none());
    assert!(reloaded.messages.len() >= 4); // private chat history kept in-file
}

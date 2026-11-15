//! End-to-end tests of the agent loop against a scripted fake Anthropic server.
//! They check the harness enforces the rules, whatever the model asks for.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cradle_backends::Backend;
use cradle_config::SourceKind;
use cradle_config::soul::{Audience, Soul};
use cradle_core::{
    AgentCommand, AgentEvent, AgentHandle, AgentSetup, Answer, Approval, ClashChoice, ModeSet,
    ReviewPolicy,
};
use cradle_provider_spec::{
    Auth, Backend as SpecBackend, Capabilities, HttpApi, HttpProvider, ProviderSpec,
};
use cradle_tools::PathGuard;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one scripted SSE response per request and records request bodies.
async fn fake_server(responses: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    tokio::spawn(async move {
        for body in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            seen.lock().unwrap().push(request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
        }
    });
    (url, requests)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse().unwrap())
                .unwrap_or(0);
            while buf.len() < end + 4 + length {
                let n = socket.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            return serde_json::from_slice(&buf[end + 4..end + 4 + length]).unwrap();
        }
    }
}

/// An SSE response containing one tool call.
fn tool_call(id: &str, name: &str, input: Value) -> String {
    sse(&[
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":input.to_string()}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ])
}

/// An SSE response containing only text.
fn text(text: &str) -> String {
    sse(&[
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ])
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn project(root: &Path) {
    project_with(root, "[commands]\nmark = \"touch ran.txt\"\n");
}

fn project_with(root: &Path, modes_toml: &str) {
    fs::create_dir_all(root.join(".agents")).unwrap();
    fs::write(root.join(".agents/modes.toml"), modes_toml).unwrap();
}

/// Run one turn, answering every approval with `approve`. Returns each approval's title
/// and body.
async fn turn_approvals(
    handle: &mut AgentHandle,
    prompt: &str,
    approve: Approval,
) -> Vec<(String, String)> {
    handle
        .commands
        .send(AgentCommand::Send(prompt.into()))
        .unwrap();
    let mut asked = Vec::new();
    while let Some(event) = handle.events.recv().await {
        match event {
            AgentEvent::Approval(req) => {
                asked.push((req.title, req.body));
                req.reply.send(approve.clone()).unwrap();
            }
            AgentEvent::TurnFinished => return asked,
            _ => {}
        }
    }
    panic!("agent stopped");
}

fn mode_review(verdicts: Value) -> String {
    text(&json!({ "commands": verdicts }).to_string())
}

fn start(url: &str, root: &Path, mode: &str, review: ReviewPolicy) -> AgentHandle {
    start_with(url, root, mode, review, Soul::default(), None)
}

fn start_with(
    url: &str,
    root: &Path,
    mode: &str,
    review: ReviewPolicy,
    soul: Soul,
    trust_store: Option<PathBuf>,
) -> AgentHandle {
    let spec = ProviderSpec {
        id: "fake".into(),
        backend: SpecBackend::Http(HttpProvider {
            api: HttpApi::Anthropic,
            base_url: url.into(),
            auth: Auth::None,
            models: vec!["test-model".into()],
            default_model: None,
            headers: vec![],
            capabilities: Capabilities {
                tools: true,
                streaming: true,
                prompt_caching: false,
                reasoning: false,
            },
        }),
    };
    let config = cradle_config::load(root, SourceKind::Directory).unwrap();
    let modes = ModeSet::resolve(Some(&config.modes), None, ClashChoice::Override).unwrap();
    cradle_core::spawn(AgentSetup {
        backend: Backend::new(&spec, None).unwrap(),
        guard: PathGuard::new(root).unwrap(),
        project: Some(config),
        modes,
        mode: mode.into(),
        review,
        history: vec![],
        session: None,
        data_dir: None,
        trust_store,
        global_modes: None,
        soul,
    })
}

/// Run one turn, answering approvals with `approve`. Returns whether an approval was asked.
async fn turn(handle: &mut AgentHandle, prompt: &str, approve: Approval) -> bool {
    handle
        .commands
        .send(AgentCommand::Send(prompt.into()))
        .unwrap();
    let mut asked = false;
    while let Some(event) = handle.events.recv().await {
        match event {
            AgentEvent::Approval(req) => {
                asked = true;
                req.reply.send(approve.clone()).unwrap();
            }
            AgentEvent::TurnFinished => return asked,
            _ => {}
        }
    }
    panic!("agent stopped");
}

/// The tool_result content the agent sent back in request `i`.
fn tool_result(requests: &Mutex<Vec<Value>>, i: usize) -> (String, bool) {
    let requests = requests.lock().unwrap();
    let messages = requests[i]["messages"].as_array().unwrap();
    let block = &messages.last().unwrap()["content"][0];
    assert_eq!(block["type"], "tool_result");
    (
        block["content"].as_str().unwrap().to_owned(),
        block["is_error"].as_bool().unwrap(),
    )
}

#[tokio::test]
async fn plan_mode_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "write_file",
            json!({"path": "evil.txt", "content": "x"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    let asked = turn(&mut agent, "write a file", Approval::Yes).await;

    assert!(!asked);
    assert!(!tmp.path().join("evil.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(is_error && content.contains("read-only"), "{content}");
    // The mode was announced to the model as a system message.
    let first = &requests.lock().unwrap()[0];
    assert_eq!(first["messages"][1]["role"], "system");
}

#[tokio::test]
async fn declined_command_does_not_run() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    let asked = turn(&mut agent, "run it", Approval::No).await;

    assert!(asked);
    assert!(!tmp.path().join("ran.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(is_error && content.contains("declined"), "{content}");
}

#[tokio::test]
async fn approved_command_runs() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    assert!(turn(&mut agent, "run it", Approval::Yes).await);
    assert!(tmp.path().join("ran.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(!is_error && content.starts_with("exit code 0"), "{content}");
}

#[tokio::test]
async fn reviewer_approves_safe_command_without_asking() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text(r#"{"verdict": "safe", "reason": "touches a file in the project"}"#),
        text("done"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    let asked = turn(&mut agent, "run it", Approval::No).await;

    assert!(!asked, "a safe verdict needs no user approval");
    assert!(tmp.path().join("ran.txt").exists());
    // The reviewer is a fresh call: no tools, no history, just the review request.
    let review = &requests.lock().unwrap()[1];
    assert!(review.get("tools").is_none());
    assert_eq!(review["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn reviewer_is_skipped_after_protected_file_changes() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "write_file",
            json!({"path": "build.rs", "content": "fn main() {}"}),
        ),
        tool_call(
            "t2",
            "run_command",
            json!({"name": "mark", "reason": "build"}),
        ),
        text("done"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    // Approves both the build.rs write and the command; no reviewer request is made.
    assert!(turn(&mut agent, "go", Approval::Yes).await);
    assert!(tmp.path().join("build.rs").exists());
    assert!(tmp.path().join("ran.txt").exists());
    assert_eq!(requests.lock().unwrap().len(), 3, "no reviewer call");
}

#[tokio::test]
async fn build_mode_asks_before_writing_and_always_allows_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, _requests) = fake_server(vec![
        tool_call("t1", "write_file", json!({"path": "a.txt", "content": "a"})),
        text("ok"),
        tool_call("t2", "write_file", json!({"path": "b.txt", "content": "b"})),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    assert!(turn(&mut agent, "write a", Approval::Always).await);
    assert!(tmp.path().join("a.txt").exists());
    assert!(
        !turn(&mut agent, "write b", Approval::No).await,
        "allowed for the session"
    );
    assert!(tmp.path().join("b.txt").exists());
}

#[tokio::test]
async fn read_files_returns_every_file_and_reports_failures() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    fs::write(tmp.path().join("a.txt"), "alpha\n").unwrap();
    fs::write(tmp.path().join("b.txt"), "beta\n").unwrap();
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "read_files",
            json!({"files": [{"path": "a.txt"}, {"path": "nope.txt"}, {"path": "b.txt"}]}),
        ),
        tool_call("t2", "read_files", json!({"files": [{"path": "nope.txt"}]})),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    agent
        .commands
        .send(AgentCommand::Send("read".into()))
        .unwrap();
    let mut finished = Vec::new();
    while let Some(event) = agent.events.recv().await {
        match event {
            AgentEvent::ToolFinished { ok, failed, .. } => finished.push((ok, failed)),
            AgentEvent::TurnFinished => break,
            _ => {}
        }
    }

    let (content, is_error) = tool_result(&requests, 1);
    assert!(!is_error, "{content}");
    assert!(
        content.contains("==> a.txt <==\n     1\talpha"),
        "{content}"
    );
    assert!(content.contains("==> b.txt <==\n     1\tbeta"), "{content}");
    assert!(content.contains("==> nope.txt <==\nerror: "), "{content}");
    let (content, is_error) = tool_result(&requests, 2);
    assert!(is_error, "every file failed: {content}");

    assert_eq!(finished.len(), 2);
    assert!(finished[0].0);
    assert_eq!(finished[0].1.len(), 1);
    assert!(finished[0].1[0].starts_with("nope.txt: "));
    assert!(!finished[1].0);
}

#[tokio::test]
async fn requested_changes_reach_the_model() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call("t1", "write_file", json!({"path": "a.txt", "content": "a"})),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    let feedback = Approval::Change("call it b.txt".into());
    assert!(turn(&mut agent, "write a", feedback).await);
    assert!(!tmp.path().join("a.txt").exists());
    let (content, is_error) = tool_result(&requests, 1);
    assert!(is_error && content.contains("call it b.txt"), "{content}");
}

#[tokio::test]
async fn reviewed_mode_command_runs_without_asking_and_is_reviewed_once() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[build_mode.commands]\nmark = \"touch ran.txt\"\n",
    );
    let run_mark = || {
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        )
    };
    let (url, requests) = fake_server(vec![
        mode_review(json!({"mark": {"verdict": "safe", "reason": "touches a file"}})),
        run_mark(),
        text("done"),
        run_mark(),
        text("done"),
        // After /clear: a new session, so a new review.
        mode_review(json!({"mark": {"verdict": "safe", "reason": "touches a file"}})),
        run_mark(),
        text("done"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    assert!(!turn(&mut agent, "run it", Approval::No).await);
    assert!(tmp.path().join("ran.txt").exists());
    // The mode review is a fresh call that lists the mode's commands.
    {
        let requests = requests.lock().unwrap();
        let review = &requests[0];
        assert!(review.get("tools").is_none());
        assert_eq!(review["messages"].as_array().unwrap().len(), 1);
        assert!(review.to_string().contains("touch ran.txt"));
    }

    fs::remove_file(tmp.path().join("ran.txt")).unwrap();
    assert!(!turn(&mut agent, "again", Approval::No).await);
    assert!(tmp.path().join("ran.txt").exists());
    assert_eq!(requests.lock().unwrap().len(), 5, "no second review");

    agent.commands.send(AgentCommand::Clear).unwrap();
    assert!(!turn(&mut agent, "after clear", Approval::No).await);
    assert_eq!(
        requests.lock().unwrap().len(),
        8,
        "reviewed again after /clear"
    );
}

#[tokio::test]
async fn rejected_mode_command_is_blocked_everywhere() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[commands]\nmark = \"touch ran.txt\"\n[build_mode.commands]\nmark = \"touch ran.txt\"\n",
    );
    let (url, requests) = fake_server(vec![
        mode_review(json!({"mark": {"verdict": "unsure", "reason": "cannot tell"}})),
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    let asked = turn(&mut agent, "run it", Approval::Yes).await;

    assert!(!asked, "blocked commands are refused, not asked about");
    assert!(!tmp.path().join("ran.txt").exists());
    let (content, is_error) = tool_result(&requests, 2);
    assert!(is_error && content.contains("blocked"), "{content}");
}

#[tokio::test]
async fn user_policy_asks_for_mode_commands_without_a_review() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[build_mode.commands]\nmark = \"touch ran.txt\"\n",
    );
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    assert!(turn(&mut agent, "run it", Approval::Yes).await);
    assert!(tmp.path().join("ran.txt").exists());
    assert_eq!(requests.lock().unwrap().len(), 2, "no reviewer call");
}

#[tokio::test]
async fn read_only_mode_runs_only_its_own_commands() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[commands]\nother = \"touch other.txt\"\n[plan_mode.commands]\nmark = \"touch ran.txt\"\n",
    );
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "check"}),
        ),
        tool_call(
            "t2",
            "run_command",
            json!({"name": "other", "reason": "check"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    turn(&mut agent, "go", Approval::Yes).await;

    assert!(tmp.path().join("ran.txt").exists());
    assert!(!tmp.path().join("other.txt").exists());
    let (content, is_error) = tool_result(&requests, 2);
    assert!(is_error && content.contains("read-only"), "{content}");
}

#[tokio::test]
async fn protected_change_suspends_mode_auto_approval() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[build_mode.commands]\nmark = \"touch ran.txt\"\n",
    );
    let (url, requests) = fake_server(vec![
        mode_review(json!({"mark": {"verdict": "safe", "reason": "touches a file"}})),
        tool_call(
            "t1",
            "write_file",
            json!({"path": "build.rs", "content": "fn main() {}"}),
        ),
        tool_call(
            "t2",
            "run_command",
            json!({"name": "mark", "reason": "build"}),
        ),
        text("done"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::Subagent);

    let asked = turn_approvals(&mut agent, "go", Approval::Yes).await;

    let titles: Vec<&str> = asked.iter().map(|(t, _)| t.as_str()).collect();
    assert!(titles.iter().any(|t| t.contains("build.rs")), "{titles:?}");
    assert!(titles.iter().any(|t| t.contains("mark")), "{titles:?}");
    assert!(tmp.path().join("ran.txt").exists());
    assert_eq!(requests.lock().unwrap().len(), 4, "no per-command review");
}

#[tokio::test]
async fn changed_config_asks_with_a_diff_and_decline_keeps_the_old_one() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);
    project_with(tmp.path(), "[commands]\nmark = \"touch evil.txt\"\n");

    let asked = turn_approvals(&mut agent, "run it", Approval::No).await;

    let (title, body) = &asked[0];
    assert!(title.contains("config changed"), "{title}");
    assert!(body.contains("-mark = touch ran.txt"), "{body}");
    assert!(body.contains("+mark = touch evil.txt"), "{body}");
    // Declined: the command still means the old line (and was declined too).
    assert!(asked[1].1.contains("$ touch ran.txt"), "{:?}", asked[1]);
    assert!(!tmp.path().join("evil.txt").exists());

    let asked = turn_approvals(&mut agent, "hello", Approval::No).await;
    assert!(
        asked.is_empty(),
        "a declined version is not asked about again"
    );
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn approved_config_change_applies_before_the_message() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "run_command",
            json!({"name": "mark", "reason": "test"}),
        ),
        text("ok"),
    ])
    .await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);
    project_with(tmp.path(), "[commands]\nmark = \"touch new.txt\"\n");

    let asked = turn_approvals(&mut agent, "run it", Approval::Yes).await;

    assert_eq!(asked.len(), 2, "the config, then the command");
    assert!(tmp.path().join("new.txt").exists());
    assert!(!tmp.path().join("ran.txt").exists());
    // The model was told about the new command.
    let first = &requests.lock().unwrap()[0];
    assert!(first.to_string().contains("touch new.txt"));
}

#[tokio::test]
async fn unchanged_config_is_not_asked_about() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    fs::write(tmp.path().join(".agents/product.md"), "v1").unwrap();
    let (url, requests) = fake_server(vec![text("ok"), text("ok")]).await;
    let mut agent = start(&url, tmp.path(), "build", ReviewPolicy::User);

    assert!(
        turn_approvals(&mut agent, "one", Approval::No)
            .await
            .is_empty()
    );
    // Docs are not trusted config: a change applies silently.
    fs::write(tmp.path().join(".agents/product.md"), "v2 product").unwrap();
    assert!(
        turn_approvals(&mut agent, "two", Approval::No)
            .await
            .is_empty()
    );
    let second = &requests.lock().unwrap()[1];
    assert!(second["system"].to_string().contains("v2 product"));
}

/// How many messages in request `i` contain `needle`.
fn count_messages(requests: &Mutex<Vec<Value>>, i: usize, needle: &str) -> usize {
    let requests = requests.lock().unwrap();
    requests[i]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m.to_string().contains(needle))
        .count()
}

#[tokio::test]
async fn preferences_are_announced_once_and_again_when_changed() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let (url, requests) = fake_server(vec![text("ok"), text("ok"), text("ok")]).await;
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    turn(&mut agent, "one", Approval::No).await;
    turn(&mut agent, "two", Approval::No).await;
    assert_eq!(count_messages(&requests, 0, "User preferences"), 1);
    assert_eq!(
        count_messages(&requests, 1, "User preferences"),
        1,
        "not repeated while unchanged"
    );
    assert_eq!(count_messages(&requests, 1, "Questions (5 of 10)"), 1);

    agent
        .commands
        .send(AgentCommand::SetSoul(Soul {
            questions: 9,
            audience: Audience::NonTechnical,
            ..Soul::default()
        }))
        .unwrap();
    turn(&mut agent, "three", Approval::No).await;
    assert_eq!(count_messages(&requests, 2, "Questions (9 of 10)"), 1);
    assert_eq!(count_messages(&requests, 2, "plain language"), 1);
}

#[tokio::test]
async fn project_preferences_apply_only_when_the_user_picks_them() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    fs::write(
        tmp.path().join(".agents/soul.toml"),
        "questions = 9\naudience = \"non-technical\"\n",
    )
    .unwrap();
    let store = tmp.path().join("cfg/trust.toml");
    let (url, requests) = fake_server(vec![text("ok"), text("ok")]).await;
    let mut agent = start_with(
        &url,
        tmp.path(),
        "plan",
        ReviewPolicy::User,
        Soul::default(),
        Some(store.clone()),
    );

    // Pick the project's questions value and keep our own audience.
    agent
        .commands
        .send(AgentCommand::Send("one".into()))
        .unwrap();
    let mut asked = Vec::new();
    while let Some(event) = agent.events.recv().await {
        match event {
            AgentEvent::Question(req) => {
                let answers = req
                    .questions
                    .iter()
                    .map(|q| {
                        asked.push(q.question.clone());
                        let pick = if q.question.contains("asks you questions") {
                            0
                        } else {
                            1
                        };
                        Answer::Choice(q.options[pick].clone())
                    })
                    .collect();
                req.reply.send(answers).unwrap();
            }
            AgentEvent::TurnFinished => break,
            _ => {}
        }
    }
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(count_messages(&requests, 0, "Questions (9 of 10)"), 1);
    assert_eq!(count_messages(&requests, 0, "precise technical terms"), 1);

    // Remembered: not asked again.
    let mut asked_again = false;
    agent
        .commands
        .send(AgentCommand::Send("two".into()))
        .unwrap();
    while let Some(event) = agent.events.recv().await {
        match event {
            AgentEvent::Question(_) => asked_again = true,
            AgentEvent::TurnFinished => break,
            _ => {}
        }
    }
    assert!(!asked_again);
    assert!(fs::read_to_string(&store).unwrap().contains("questions"));
}

#[tokio::test]
async fn auto_verify_reports_a_failed_build_to_the_model() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[commands]\nbuild = \"false\"\ntest = \"touch tested.txt\"\n",
    );
    let (url, requests) = fake_server(vec![
        tool_call("t1", "write_file", json!({"path": "a.txt", "content": "a"})),
        text("done"),
        text("fixed"),
    ])
    .await;
    let soul = Soul {
        auto_verify: true,
        ..Soul::default()
    };
    let mut agent = start_with(&url, tmp.path(), "build", ReviewPolicy::User, soul, None);

    let asked = turn_approvals(&mut agent, "write a", Approval::Yes).await;

    let titles: Vec<&str> = asked.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["Create a.txt?", "Run `build`?"]);
    assert!(
        !tmp.path().join("tested.txt").exists(),
        "test doesn't run after a failed build"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "one more model call to fix it");
    let last = requests[2]["messages"].as_array().unwrap().last().unwrap();
    assert!(last.to_string().contains("`build` failed"), "{last}");
}

#[tokio::test]
async fn auto_verify_runs_build_and_test_once_after_edits() {
    let tmp = tempfile::tempdir().unwrap();
    project_with(
        tmp.path(),
        "[commands]\nbuild = \"touch built.txt\"\ntest = \"touch tested.txt\"\n",
    );
    let (url, requests) = fake_server(vec![
        tool_call("t1", "write_file", json!({"path": "a.txt", "content": "a"})),
        text("done"),
        text("just chatting"),
    ])
    .await;
    let soul = Soul {
        auto_verify: true,
        ..Soul::default()
    };
    let mut agent = start_with(&url, tmp.path(), "build", ReviewPolicy::User, soul, None);

    let asked = turn_approvals(&mut agent, "write a", Approval::Yes).await;
    assert_eq!(asked.len(), 3, "the write, build and test");
    assert!(tmp.path().join("built.txt").exists());
    assert!(tmp.path().join("tested.txt").exists());
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "passing checks need no model call"
    );

    fs::remove_file(tmp.path().join("built.txt")).unwrap();
    let asked = turn_approvals(&mut agent, "hello", Approval::Yes).await;
    assert!(asked.is_empty(), "nothing changed: no checks");
    assert!(!tmp.path().join("built.txt").exists());
}

#[tokio::test]
async fn saved_results_are_summarised_and_filtered() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    fs::write(tmp.path().join("a.txt"), "fn alpha\n").unwrap();
    fs::write(tmp.path().join("b.txt"), "fn beta\nfn gamma\n").unwrap();
    let (url, requests) = fake_server(vec![
        tool_call(
            "t1",
            "search",
            json!({"pattern": "^fn ", "save_as": "hits"}),
        ),
        tool_call(
            "t2",
            "var_replace",
            json!({"var": "hits", "pattern": "^([^:]+):.*", "replacement": "$1", "save_as": "paths"}),
        ),
        tool_call(
            "t3",
            "var_lines",
            json!({"var": "$paths", "unique": true, "sort": true}),
        ),
        tool_call("t4", "var_show", json!({"var": "nope"})),
        text("ok"),
    ])
    .await;
    // Variable tools are read-only, so plan mode may use them.
    let mut agent = start(&url, tmp.path(), "plan", ReviewPolicy::User);

    agent
        .commands
        .send(AgentCommand::Send("find".into()))
        .unwrap();
    let mut started = Vec::new();
    while let Some(event) = agent.events.recv().await {
        match event {
            AgentEvent::ToolStarted { summary, .. } => started.push(summary),
            AgentEvent::TurnFinished => break,
            _ => {}
        }
    }

    let (content, is_error) = tool_result(&requests, 1);
    assert!(!is_error, "{content}");
    assert!(
        content.starts_with("saved $hits: 3 lines, "),
        "a summary, not the full result: {content}"
    );
    let (content, is_error) = tool_result(&requests, 2);
    assert!(
        !is_error && content.starts_with("saved $paths: 3 lines"),
        "{content}"
    );
    let (content, is_error) = tool_result(&requests, 3);
    assert!(!is_error, "{content}");
    assert_eq!(content, "a.txt\nb.txt");
    let (content, is_error) = tool_result(&requests, 4);
    assert!(
        is_error && content.contains("no variable `$nope`"),
        "{content}"
    );
    // The UI shows where a result was saved.
    assert_eq!(started[0], "search /^fn / in . → $hits");
}

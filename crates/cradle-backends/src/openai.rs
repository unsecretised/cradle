//! OpenAI-compatible Chat Completions (OpenAI, OpenRouter, Ollama, LM Studio, ...).
//!
//! Non-streaming for now: the full reply is emitted as one text event. Opaque blocks
//! (Anthropic thinking) are dropped when converting the conversation.

use serde_json::{Value, json};

use crate::BackendError;
use crate::http;
use crate::message::{Block, Message, ModelRequest, Role, StopReason, StreamEvent, Turn, Usage};
use cradle_provider_spec::Capabilities;

pub(crate) struct Client {
    http: reqwest::Client,
    url: String,
    key: Option<String>,
    model: String,
    headers: Vec<(String, String)>,
    caps: Capabilities,
}

impl Client {
    pub(crate) fn new(
        base_url: &str,
        key: Option<String>,
        model: String,
        headers: Vec<(String, String)>,
        caps: Capabilities,
    ) -> Result<Self, BackendError> {
        Ok(Client {
            http: http::client()?,
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            key,
            model,
            headers,
            caps,
        })
    }

    pub(crate) async fn complete(
        &self,
        req: &ModelRequest<'_>,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<Turn, BackendError> {
        let body = self.body(req);
        let resp = http::send(|| {
            let mut builder = self.http.post(&self.url).json(&body);
            if let Some(key) = &self.key {
                builder = builder.bearer_auth(key);
            }
            for (name, value) in &self.headers {
                builder = builder.header(name, value);
            }
            builder
        })
        .await?;
        let value: Value = resp.json().await?;
        parse_response(&value, on_event)
    }

    fn body(&self, req: &ModelRequest<'_>) -> Value {
        let mut messages = vec![json!({"role": "system", "content": req.system})];
        for message in req.messages {
            messages.extend(message_json(message));
        }
        let mut body = json!({"model": self.model, "messages": messages});
        if self.caps.tools && !req.tools.is_empty() {
            body["tools"] = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect();
        }
        body
    }
}

fn message_json(message: &Message) -> Vec<Value> {
    match message.role {
        Role::System => vec![json!({"role": "system", "content": message.text()})],
        Role::User => {
            // Tool results must directly follow the assistant message that called them.
            let mut out: Vec<Value> = message
                .content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => Some(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": if *is_error { format!("ERROR: {content}") } else { content.clone() },
                    })),
                    _ => None,
                })
                .collect();
            let text = message.text();
            if !text.is_empty() {
                out.push(json!({"role": "user", "content": text}));
            }
            out
        }
        Role::Assistant => {
            let calls: Vec<Value> = message
                .tool_uses()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": call.input.to_string()},
                    })
                })
                .collect();
            let text = message.text();
            if text.is_empty() && calls.is_empty() {
                return Vec::new();
            }
            let mut out = json!({
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { Value::String(text) },
            });
            if !calls.is_empty() {
                out["tool_calls"] = Value::Array(calls);
            }
            vec![out]
        }
    }
}

fn parse_response(
    value: &Value,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) -> Result<Turn, BackendError> {
    let choice = value
        .pointer("/choices/0")
        .ok_or_else(|| BackendError::Protocol("response has no choices".into()))?;
    let msg = choice
        .get("message")
        .ok_or_else(|| BackendError::Protocol("choice has no message".into()))?;

    let mut content = Vec::new();
    if let Some(text) = msg.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        on_event(StreamEvent::TextDelta(text.to_owned()));
        content.push(Block::Text { text: text.into() });
    }
    for call in msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let raw = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        let (input, invalid_json) = match serde_json::from_str::<Value>(raw) {
            Ok(v @ Value::Object(_)) => (v, None),
            _ => (json!({}), Some(raw.to_owned())),
        };
        on_event(StreamEvent::ToolUseStarted { name: name.into() });
        content.push(Block::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
            invalid_json,
        });
    }

    let has_calls = content.iter().any(|b| matches!(b, Block::ToolUse { .. }));
    let stop = match choice.get("finish_reason").and_then(Value::as_str) {
        _ if has_calls => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Refusal {
            category: Some("content_filter".into()),
            explanation: None,
        },
        Some("stop") | None => StopReason::EndTurn,
        Some(other) => StopReason::Other(other.into()),
    };
    let usage = value.get("usage").cloned().unwrap_or_default();
    Ok(Turn {
        message: Message {
            role: Role::Assistant,
            content,
        },
        stop,
        usage: Usage {
            input_tokens: usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            ..Usage::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_tool_round_trip() {
        let assistant = Message {
            role: Role::Assistant,
            content: vec![
                Block::Opaque {
                    value: json!({"type": "thinking"}),
                },
                Block::ToolUse {
                    id: "c1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "a"}),
                    invalid_json: None,
                },
            ],
        };
        let out = message_json(&assistant);
        assert_eq!(out[0]["content"], Value::Null);
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );

        let results = Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "c1".into(),
                content: "nope".into(),
                is_error: true,
            }],
        };
        let out = message_json(&results);
        assert_eq!(
            out,
            vec![json!({"role":"tool","tool_call_id":"c1","content":"ERROR: nope"})]
        );
    }

    #[test]
    fn parses_tool_calls_and_bad_arguments() {
        let mut sink = |_| {};
        let turn = parse_response(
            &json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[
                {"id":"c1","function":{"name":"search","arguments":"{\"pattern\":\"x\"}"}},
                {"id":"c2","function":{"name":"search","arguments":"{oops"}}
            ]}}],"usage":{"prompt_tokens":5,"completion_tokens":7}}),
            &mut sink,
        )
        .unwrap();
        assert_eq!(turn.stop, StopReason::ToolUse);
        let calls: Vec<_> = turn.message.tool_uses().collect();
        assert!(calls[0].invalid_json.is_none());
        assert_eq!(calls[1].invalid_json, Some("{oops"));
        assert_eq!(turn.usage.output_tokens, 7);
    }
}

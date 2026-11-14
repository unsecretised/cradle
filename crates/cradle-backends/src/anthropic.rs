//! Anthropic Messages API over raw HTTP (there is no official Rust SDK).
//!
//! - Streams over SSE and rebuilds content blocks from deltas.
//! - Thinking blocks (with signatures) and other unknown blocks are kept verbatim and sent
//!   back unchanged; history is append-only.
//! - Adaptive thinking with summarized display when the provider enables `reasoning`.
//! - Prompt caching: a breakpoint on the system prompt plus top-level automatic caching.
//! - Tools stream eagerly; streamed tool input is parsed strictly, and invalid JSON is
//!   flagged so the tool never runs on it.
//! - Server-side refusal fallbacks on models that support them.

use futures_util::StreamExt;
use serde_json::{Map, Value, json};

use crate::BackendError;
use crate::http;
use crate::message::{Block, Message, ModelRequest, Role, StopReason, StreamEvent, Turn, Usage};
use cradle_provider_spec::Capabilities;

pub(crate) const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Models that accept `fallbacks: "default"` on the Claude API.
const FALLBACK_MODELS: &[&str] = &[
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];
const EFFORT: &str = "high";

pub(crate) struct Client {
    http: reqwest::Client,
    url: String,
    key: Option<String>,
    model: String,
    caps: Capabilities,
    fallbacks: bool,
}

impl Client {
    pub(crate) fn new(
        base_url: &str,
        key: Option<String>,
        model: String,
        caps: Capabilities,
    ) -> Result<Self, BackendError> {
        let base = base_url.trim_end_matches('/');
        let first_party = base == "https://api.anthropic.com";
        Ok(Client {
            http: http::client()?,
            url: format!("{base}/v1/messages"),
            key,
            fallbacks: first_party && FALLBACK_MODELS.contains(&model.as_str()),
            model,
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
            let mut builder = self
                .http
                .post(&self.url)
                .header("anthropic-version", API_VERSION)
                .json(&body);
            if let Some(key) = &self.key {
                builder = builder.header("x-api-key", key);
            }
            if self.fallbacks {
                builder = builder.header("anthropic-beta", FALLBACK_BETA);
            }
            builder
        })
        .await?;

        if self.caps.streaming {
            read_stream(resp, on_event).await
        } else {
            let value: Value = resp.json().await?;
            parse_message(&value, on_event)
        }
    }

    fn body(&self, req: &ModelRequest<'_>) -> Value {
        let mut system = json!({"type": "text", "text": req.system});
        if self.caps.prompt_caching {
            system["cache_control"] = json!({"type": "ephemeral"});
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "system": [system],
            "messages": req.messages.iter().filter_map(message_json).collect::<Vec<_>>(),
        });
        let obj = body.as_object_mut().unwrap();

        if self.caps.streaming {
            obj.insert("stream".into(), json!(true));
        }
        if self.caps.prompt_caching {
            obj.insert("cache_control".into(), json!({"type": "ephemeral"}));
        }
        if self.caps.tools && !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    let mut tool = json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    });
                    if self.caps.streaming {
                        tool["eager_input_streaming"] = json!(true);
                    }
                    tool
                })
                .collect();
            obj.insert("tools".into(), Value::Array(tools));
        }
        if self.caps.reasoning {
            obj.insert(
                "thinking".into(),
                json!({"type": "adaptive", "display": "summarized"}),
            );
            obj.insert("output_config".into(), json!({"effort": EFFORT}));
        }
        if self.fallbacks {
            obj.insert("fallbacks".into(), json!("default"));
        }
        body
    }
}

fn message_json(message: &Message) -> Option<Value> {
    if message.role == Role::System {
        return Some(json!({"role": "system", "content": message.text()}));
    }
    let content: Vec<Value> = message
        .content
        .iter()
        .filter_map(|block| match block {
            // The API rejects empty text blocks.
            Block::Text { text } if text.is_empty() => None,
            Block::Text { text } => Some(json!({"type": "text", "text": text})),
            Block::ToolUse {
                id, name, input, ..
            } => Some(json!({"type": "tool_use", "id": id, "name": name, "input": input})),
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            })),
            Block::Opaque { value } => Some(value.clone()),
        })
        .collect();
    if content.is_empty() {
        return None;
    }
    let role = match message.role {
        Role::User => "user",
        _ => "assistant",
    };
    Some(json!({"role": role, "content": content}))
}

/// A content block being assembled from stream deltas.
struct Partial {
    block: Map<String, Value>,
    input_json: String,
}

impl Partial {
    fn append(&mut self, field: &str, text: &str) {
        match self.block.get_mut(field) {
            Some(Value::String(s)) => s.push_str(text),
            _ => {
                self.block.insert(field.into(), Value::String(text.into()));
            }
        }
    }

    fn finish(self) -> Block {
        let kind = self.block.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" if !self.block.contains_key("citations") => Block::Text {
                text: str_field(&self.block, "text"),
            },
            "tool_use" => {
                let id = str_field(&self.block, "id");
                let name = str_field(&self.block, "name");
                let raw = if self.input_json.trim().is_empty() {
                    "{}"
                } else {
                    &self.input_json
                };
                match serde_json::from_str::<Value>(raw) {
                    Ok(input @ Value::Object(_)) => Block::ToolUse {
                        id,
                        name,
                        input,
                        invalid_json: None,
                    },
                    _ => Block::ToolUse {
                        id,
                        name,
                        input: json!({}),
                        invalid_json: Some(self.input_json),
                    },
                }
            }
            _ => Block::Opaque {
                value: Value::Object(self.block),
            },
        }
    }
}

fn str_field(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Rebuild a turn from the SSE stream.
async fn read_stream(
    resp: reqwest::Response,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) -> Result<Turn, BackendError> {
    let mut parser = StreamParser::default();
    let mut bytes = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();

    while let Some(chunk) = bytes.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some((end, sep)) = find_event_end(&buf) {
            let event: Vec<u8> = buf.drain(..end + sep).collect();
            let text = String::from_utf8_lossy(&event[..end]);
            let data: String = text
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&data)
                .map_err(|e| BackendError::Protocol(format!("invalid event JSON: {e}")))?;
            if parser.handle(&value, on_event)? {
                return parser.finish();
            }
        }
    }
    Err(BackendError::Protocol(
        "stream ended before message_stop".into(),
    ))
}

fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

#[derive(Default)]
struct StreamParser {
    open: Vec<(u64, Partial)>,
    done: Vec<(u64, Block)>,
    stop: Option<StopReason>,
    usage: Usage,
}

impl StreamParser {
    /// Handle one event. Returns `true` at `message_stop`.
    fn handle(
        &mut self,
        event: &Value,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<bool, BackendError> {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.usage.input_tokens = u64_at(usage, "input_tokens");
                    self.usage.cache_read_tokens = u64_at(usage, "cache_read_input_tokens");
                    self.usage.cache_write_tokens = u64_at(usage, "cache_creation_input_tokens");
                }
            }
            "content_block_start" => {
                let block = event
                    .get("content_block")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    on_event(StreamEvent::ToolUseStarted {
                        name: str_field(&block, "name"),
                    });
                }
                // tool_use starts with `input: {}`; the real input arrives as deltas.
                let mut block = block;
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    block.remove("input");
                }
                self.open.push((
                    index,
                    Partial {
                        block,
                        input_json: String::new(),
                    },
                ));
            }
            "content_block_delta" => {
                let Some(partial) = self.open.iter_mut().find(|(i, _)| *i == index) else {
                    return Ok(false);
                };
                let partial = &mut partial.1;
                let delta = event.get("delta").cloned().unwrap_or_default();
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        partial.append("text", text);
                        on_event(StreamEvent::TextDelta(text.to_owned()));
                    }
                    "thinking_delta" => {
                        let text = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        partial.append("thinking", text);
                        on_event(StreamEvent::ThinkingDelta(text.to_owned()));
                    }
                    "signature_delta" => {
                        if let Some(sig) = delta.get("signature") {
                            partial.block.insert("signature".into(), sig.clone());
                        }
                    }
                    "input_json_delta" => {
                        let json = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        partial.input_json.push_str(json);
                    }
                    "citations_delta" => {
                        if let Some(citation) = delta.get("citation") {
                            let list = partial
                                .block
                                .entry("citations")
                                .or_insert_with(|| Value::Array(Vec::new()));
                            if let Value::Array(items) = list {
                                items.push(citation.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(pos) = self.open.iter().position(|(i, _)| *i == index) {
                    let (i, partial) = self.open.remove(pos);
                    self.done.push((i, partial.finish()));
                }
            }
            "message_delta" => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    let details = event
                        .pointer("/delta/stop_details")
                        .or_else(|| event.get("stop_details"));
                    self.stop = Some(StopReason::from_anthropic(reason, details));
                }
                if let Some(usage) = event.get("usage") {
                    self.usage.output_tokens = u64_at(usage, "output_tokens");
                }
            }
            "message_stop" => return Ok(true),
            "error" => {
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown stream error");
                return Err(BackendError::Protocol(message.to_owned()));
            }
            _ => {} // ping and future event types
        }
        Ok(false)
    }

    fn finish(mut self) -> Result<Turn, BackendError> {
        // Blocks that never got a stop event (cut off) are still kept.
        for (i, partial) in self.open.drain(..) {
            self.done.push((i, partial.finish()));
        }
        self.done.sort_by_key(|(i, _)| *i);
        Ok(Turn {
            message: Message {
                role: Role::Assistant,
                content: self.done.into_iter().map(|(_, b)| b).collect(),
            },
            stop: self
                .stop
                .ok_or_else(|| BackendError::Protocol("no stop_reason in stream".into()))?,
            usage: self.usage,
        })
    }
}

/// Parse a non-streamed response.
fn parse_message(
    value: &Value,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) -> Result<Turn, BackendError> {
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| BackendError::Protocol("response has no content".into()))?;
    let mut content = Vec::new();
    for block in blocks {
        let Some(map) = block.as_object() else {
            continue;
        };
        let block = match map.get("type").and_then(Value::as_str) {
            Some("text") if !map.contains_key("citations") => {
                let text = str_field(map, "text");
                on_event(StreamEvent::TextDelta(text.clone()));
                Block::Text { text }
            }
            Some("tool_use") => Block::ToolUse {
                id: str_field(map, "id"),
                name: str_field(map, "name"),
                input: map.get("input").cloned().unwrap_or_else(|| json!({})),
                invalid_json: None,
            },
            _ => Block::Opaque {
                value: block.clone(),
            },
        };
        content.push(block);
    }
    let reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let usage = value.get("usage").cloned().unwrap_or_default();
    Ok(Turn {
        message: Message {
            role: Role::Assistant,
            content,
        },
        stop: StopReason::from_anthropic(reason, value.get("stop_details")),
        usage: Usage {
            input_tokens: u64_at(&usage, "input_tokens"),
            output_tokens: u64_at(&usage, "output_tokens"),
            cache_read_tokens: u64_at(&usage, "cache_read_input_tokens"),
            cache_write_tokens: u64_at(&usage, "cache_creation_input_tokens"),
        },
    })
}

fn u64_at(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolSpec;

    fn caps() -> Capabilities {
        Capabilities {
            tools: true,
            streaming: true,
            prompt_caching: true,
            reasoning: true,
        }
    }

    #[test]
    fn rebuilds_blocks_from_stream_events() {
        let events = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":4}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me "}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"look."}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading."}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"a.rs\"}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":30}}),
            json!({"type":"message_stop"}),
        ];
        let mut parser = StreamParser::default();
        let mut seen = Vec::new();
        let mut sink = |e: StreamEvent| seen.push(e);
        for (i, e) in events.iter().enumerate() {
            let stop = parser.handle(e, &mut sink).unwrap();
            assert_eq!(stop, i == events.len() - 1);
        }
        let turn = parser.finish().unwrap();

        assert_eq!(turn.stop, StopReason::ToolUse);
        assert_eq!(turn.usage.input_tokens, 10);
        assert_eq!(turn.usage.output_tokens, 30);
        assert_eq!(
            turn.message.content[0],
            Block::Opaque {
                value: json!({"type":"thinking","thinking":"Let me look.","signature":"sig"})
            }
        );
        assert_eq!(turn.message.text(), "Reading.");
        let call = turn.message.tool_uses().next().unwrap();
        assert_eq!(call.input, &json!({"path": "a.rs"}));
        assert!(call.invalid_json.is_none());
        assert!(seen.contains(&StreamEvent::ThinkingDelta("look.".into())));
        assert!(seen.contains(&StreamEvent::ToolUseStarted {
            name: "read_file".into()
        }));
    }

    #[test]
    fn flags_invalid_tool_json() {
        let partial = Partial {
            block: json!({"type":"tool_use","id":"t","name":"write_file"})
                .as_object()
                .unwrap()
                .clone(),
            input_json: "{\"path\": \"a\", \"content\": \"unterminated".into(),
        };
        match partial.finish() {
            Block::ToolUse {
                input,
                invalid_json,
                ..
            } => {
                assert_eq!(input, json!({}));
                assert!(invalid_json.is_some());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn request_body_shape() {
        let client = Client::new(
            "https://api.anthropic.com/",
            Some("k".into()),
            "claude-opus-5-5".into(),
            caps(),
        )
        .unwrap();
        assert_eq!(client.url, "https://api.anthropic.com/v1/messages");
        assert!(client.fallbacks);

        let messages = [
            Message::user("hi"),
            Message::system("mode: plan"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::Opaque {
                        value: json!({"type":"thinking","thinking":"","signature":"s"}),
                    },
                    Block::Text {
                        text: String::new(),
                    },
                ],
            },
        ];
        let tools = [ToolSpec {
            name: "read_file".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        }];
        let body = client.body(&ModelRequest {
            system: "sys",
            messages: &messages,
            tools: &tools,
            max_tokens: 64000,
        });
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["cache_control"]["type"], "ephemeral");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["eager_input_streaming"], true);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["fallbacks"], "default");
        assert!(body.get("temperature").is_none());
        assert_eq!(
            body["messages"][1],
            json!({"role":"system","content":"mode: plan"})
        );
        // Thinking is passed back verbatim; the empty text block is dropped.
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn no_fallbacks_on_third_party_hosts() {
        let client = Client::new(
            "https://proxy.example",
            None,
            "claude-opus-5-5".into(),
            caps(),
        )
        .unwrap();
        assert!(!client.fallbacks);
    }
}

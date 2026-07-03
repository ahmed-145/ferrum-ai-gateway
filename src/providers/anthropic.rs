use crate::config::ProviderConfig;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

pub fn build_request_headers(
    _config: &ProviderConfig,
    client_auth_header: Option<&str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

    if let Some(auth) = client_auth_header {
        // Authorization: Bearer <key> -> x-api-key: <key>
        let key = auth.strip_prefix("Bearer ").unwrap_or(auth);
        if let Ok(val) = key.parse() {
            headers.insert("x-api-key", val);
        }
    }
    headers
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

/// Translates an OpenAI Chat Completions request to an Anthropic Messages request.
pub fn translate_request(openai_req: OpenAIRequest, target_model: String) -> AnthropicRequest {
    let mut system_content = Vec::new();
    let mut anthropic_messages = Vec::new();

    for msg in openai_req.messages {
        if msg.role == "system" {
            system_content.push(msg.content);
        } else {
            // Anthropic only allows "user" and "assistant" roles
            let role = match msg.role.as_str() {
                "user" => "user".to_string(),
                "assistant" => "assistant".to_string(),
                _ => "user".to_string(), // Default fallback
            };
            anthropic_messages.push(AnthropicMessage {
                role,
                content: msg.content,
            });
        }
    }

    let system = if system_content.is_empty() {
        None
    } else {
        Some(system_content.join("\n"))
    };

    // Anthropic requires max_tokens
    let max_tokens = openai_req
        .max_tokens
        .or(openai_req.max_completion_tokens)
        .unwrap_or(4096);

    AnthropicRequest {
        model: target_model,
        system,
        messages: anthropic_messages,
        max_tokens,
        temperature: openai_req.temperature,
        stream: openai_req.stream,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicResponseUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicResponseContent {
    pub r#type: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicResponse {
    pub id: String,
    pub role: String,
    pub content: Vec<AnthropicResponseContent>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub usage: Option<AnthropicResponseUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIResponseChoiceMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIResponseChoice {
    pub index: u32,
    pub message: OpenAIResponseChoiceMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIResponseUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIResponseChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAIResponseUsage>,
}

/// Translates an Anthropic Messages response back to an OpenAI Chat Completions response.
pub fn translate_response(anth_resp: AnthropicResponse) -> OpenAIResponse {
    let content = anth_resp
        .content
        .iter()
        .filter(|c| c.r#type == "text")
        .filter_map(|c| c.text.clone())
        .collect::<Vec<String>>()
        .join("");

    let finish_reason = anth_resp.stop_reason.map(|r| match r.as_str() {
        "end_turn" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        "stop_sequence" => "stop".to_string(),
        other => other.to_string(),
    });

    let choices = vec![OpenAIResponseChoice {
        index: 0,
        message: OpenAIResponseChoiceMessage {
            role: anth_resp.role,
            content,
        },
        finish_reason,
    }];

    let usage = anth_resp.usage.map(|u| OpenAIResponseUsage {
        prompt_tokens: u_u32_default(u.input_tokens),
        completion_tokens: u_u32_default(u.output_tokens),
        total_tokens: u_u32_default(u.input_tokens + u.output_tokens),
    });

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    OpenAIResponse {
        id: anth_resp.id,
        object: "chat.completion".to_string(),
        created,
        model: anth_resp.model,
        choices,
        usage,
    }
}

fn u_u32_default(val: u32) -> u32 {
    val
}

// Structs for Anthropic SSE streaming events
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageStart {
    pub message: AnthropicMessageStartMessage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageStartMessage {
    pub id: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicContentBlockDelta {
    pub delta: AnthropicContentBlockDeltaData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicContentBlockDeltaData {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDelta {
    pub delta: AnthropicMessageDeltaData,
    pub usage: Option<AnthropicMessageDeltaUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDeltaData {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDeltaUsage {
    pub output_tokens: u32,
}

/// A parser and translator that processes incoming Anthropic SSE chunks
/// and yields formatted OpenAI SSE chunks.
#[derive(Debug, Default)]
pub struct AnthropicStreamTranslator {
    buffer: String,
    current_event: Option<String>,
    message_id: String,
    model_name: String,
}

impl AnthropicStreamTranslator {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            current_event: None,
            message_id: "msg-unknown".to_string(),
            model_name: "claude-model".to_string(),
        }
    }

    /// Processes a chunk of raw response bytes, updating internal buffers
    /// and returning any complete OpenAI chunk lines generated.
    pub fn process_chunk(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut results = Vec::new();
        let chunk_str = match std::str::from_utf8(chunk) {
            Ok(s) => s,
            Err(_) => return results, // Ignore invalid UTF-8 chunks
        };

        self.buffer.push_str(chunk_str);

        while let Some(line_end_idx) = self.buffer.find('\n') {
            let line = self.buffer[..line_end_idx].trim().to_string();
            self.buffer.drain(..=line_end_idx);

            if line.is_empty() {
                continue;
            }

            if line.starts_with("event:") {
                let event_type = line.strip_prefix("event:").unwrap_or("").trim().to_string();
                self.current_event = Some(event_type);
            } else if line.starts_with("data:") {
                let data_str = line.strip_prefix("data:").unwrap_or("").trim();
                if let Some(event) = self.current_event.clone() {
                    if let Some(translated) = self.translate_event(&event, data_str) {
                        results.push(translated);
                    }
                }
            }
        }

        results
    }

    fn translate_event(&mut self, event: &str, data_json: &str) -> Option<String> {
        match event {
            "message_start" => {
                if let Ok(start) = serde_json::from_str::<AnthropicMessageStart>(data_json) {
                    self.message_id = start.message.id;
                    self.model_name = start.message.model;
                }
                None
            }
            "content_block_delta" => {
                if let Ok(delta) = serde_json::from_str::<AnthropicContentBlockDelta>(data_json) {
                    let openai_chunk = serde_json::json!({
                        "id": self.message_id,
                        "object": "chat.completion.chunk",
                        "created": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
                        "model": self.model_name,
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "content": delta.delta.text
                            },
                            "finish_reason": serde_json::Value::Null
                        }]
                    });
                    Some(format!("data: {}\n\n", openai_chunk))
                } else {
                    None
                }
            }
            "message_delta" => {
                if let Ok(delta) = serde_json::from_str::<AnthropicMessageDelta>(data_json) {
                    let finish_reason = delta.delta.stop_reason.map(|r| match r.as_str() {
                        "end_turn" => "stop".to_string(),
                        "max_tokens" => "length".to_string(),
                        "stop_sequence" => "stop".to_string(),
                        other => other.to_string(),
                    });
                    let openai_chunk = serde_json::json!({
                        "id": self.message_id,
                        "object": "chat.completion.chunk",
                        "created": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
                        "model": self.model_name,
                        "choices": [{
                            "index": 0,
                            "delta": {},
                            "finish_reason": finish_reason
                        }]
                    });
                    Some(format!("data: {}\n\n", openai_chunk))
                } else {
                    None
                }
            }
            "message_stop" => Some("data: [DONE]\n\n".to_string()),
            _ => None,
        }
    }
}

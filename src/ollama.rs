use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Model {
    pub name: String,
    pub size: Option<u64>,
    pub modified_at: Option<String>,
    pub details: Option<ModelDetails>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModelDetails {
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatDelta {
    Content(String),
    Thinking(String),
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    message: Option<ChunkMessage>,
    response: Option<String>,
    done: bool,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkMessage {
    content: Option<String>,
    thinking: Option<String>,
}

impl OllamaClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<Model>> {
        let url = format!("{}/api/tags", self.base_url);
        let response = self
            .http
            .get(url)
            .send()
            .await
            .context("failed to connect to Ollama")?
            .error_for_status()
            .context("Ollama returned an error while listing models")?;
        let mut models = response.json::<TagsResponse>().await?.models;
        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(models)
    }

    pub async fn chat_stream<F>(
        &self,
        model: &str,
        messages: &[ChatMessage],
        mut on_delta: F,
    ) -> Result<String>
    where
        F: FnMut(ChatDelta) + Send,
    {
        if model.is_empty() {
            bail!("no model selected");
        }

        let url = format!("{}/api/chat", self.base_url);
        let request = ChatRequest {
            model,
            messages,
            stream: true,
        };
        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .context("failed to connect to Ollama")?
            .error_for_status()
            .context("Ollama returned an error while chatting")?;

        let mut stream = response.bytes_stream();
        let mut pending = String::new();
        let mut full = String::new();
        let mut thinking_open = false;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("failed to read Ollama stream")?;
            pending.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(newline) = pending.find('\n') {
                let line = pending[..newline].trim().to_string();
                pending = pending[newline + 1..].to_string();
                if line.is_empty() {
                    continue;
                }

                let chunk: ChatChunk = serde_json::from_str(&line)
                    .with_context(|| format!("invalid Ollama stream chunk: {line}"))?;
                if let Some(error) = chunk.error {
                    bail!(error);
                }
                if let Some(message) = chunk.message {
                    if let Some(thinking) = message.thinking.filter(|value| !value.is_empty()) {
                        on_delta(ChatDelta::Thinking(thinking.clone()));
                        if !thinking_open {
                            full.push_str("<think>");
                            thinking_open = true;
                        }
                        full.push_str(&thinking);
                    }
                    if let Some(content) = message.content.filter(|value| !value.is_empty()) {
                        if thinking_open {
                            full.push_str("</think>");
                            thinking_open = false;
                        }
                        on_delta(ChatDelta::Content(content.clone()));
                        full.push_str(&content);
                    }
                } else if let Some(delta) = chunk.response.filter(|value| !value.is_empty()) {
                    if thinking_open {
                        full.push_str("</think>");
                        thinking_open = false;
                    }
                    on_delta(ChatDelta::Content(delta.clone()));
                    full.push_str(&delta);
                }
                if chunk.done {
                    if thinking_open {
                        full.push_str("</think>");
                    }
                    return Ok(full);
                }
            }
        }

        if !pending.trim().is_empty() {
            let chunk: ChatChunk = serde_json::from_str(pending.trim())
                .with_context(|| format!("invalid Ollama stream chunk: {}", pending.trim()))?;
            if let Some(error) = chunk.error {
                bail!(error);
            }
            if let Some(message) = chunk.message {
                if let Some(thinking) = message.thinking.filter(|value| !value.is_empty()) {
                    on_delta(ChatDelta::Thinking(thinking.clone()));
                    if !thinking_open {
                        full.push_str("<think>");
                        thinking_open = true;
                    }
                    full.push_str(&thinking);
                }
                if let Some(content) = message.content.filter(|value| !value.is_empty()) {
                    if thinking_open {
                        full.push_str("</think>");
                        thinking_open = false;
                    }
                    on_delta(ChatDelta::Content(content.clone()));
                    full.push_str(&content);
                }
            } else if let Some(delta) = chunk.response.filter(|value| !value.is_empty()) {
                if thinking_open {
                    full.push_str("</think>");
                    thinking_open = false;
                }
                on_delta(ChatDelta::Content(delta.clone()));
                full.push_str(&delta);
            }
        }

        if thinking_open {
            full.push_str("</think>");
        }

        Ok(full)
    }
}

pub fn system_prompt(agents_md: Option<&str>) -> ChatMessage {
    let agents_section = agents_md
        .map(|content| {
            format!(
                "\n\nProject instructions from AGENTS.md:\n---\n{}\n---",
                content.trim()
            )
        })
        .unwrap_or_else(|| {
            "\n\nNo AGENTS.md was found. The user can run /init to create one.".to_string()
        });

    ChatMessage {
        role: "system".to_string(),
        content: {
            let mut prompt = r#"You are ollo-code, a local autonomous coding agent running in a terminal.

The full conversation history in this chat is your memory. Use previous user and assistant messages when answering. If the user asks what they just said or refers to earlier messages, answer from the messages already in this conversation instead of claiming you cannot remember.
Do not output evaluation JSON, benchmark metadata, or self-critique wrappers. Reply in plain language unless you are emitting a tool call.

You may inspect and modify files by emitting exactly one fenced JSON tool call when a tool is needed.
After a tool result is returned, continue from the result. Tool results are real even when ok is false. Do not claim a supported tool was unrecognized.

Tool call format:
```json
{"tool":"read","path":"src/main.rs"}
```

Supported tools:
- bash: {"tool":"bash","command":"cargo check"}
- read: {"tool":"read","path":"relative/path"}
- write: {"tool":"write","path":"relative/path","content":"full file content"}
- edit: {"tool":"edit","path":"relative/path","old":"exact old text","new":"replacement text"}
- list: {"tool":"list","path":"."}
- search: {"tool":"search","query":"symbol or text","path":"optional/path"}
- patch: {"tool":"patch","patch":"*** Begin Patch\n..."}

The required core tools are bash, read, write, and edit. Prefer read before edit/write. Use search before broad reads. Keep changes focused."#
                .to_string();
            prompt.push_str(&agents_section);
            prompt
        },
    }
}

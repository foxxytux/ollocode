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

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    message: Option<ChatMessage>,
    response: Option<String>,
    done: bool,
    error: Option<String>,
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
        F: FnMut(String) + Send,
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
                let delta = chunk
                    .message
                    .map(|message| message.content)
                    .or(chunk.response)
                    .unwrap_or_default();
                if !delta.is_empty() {
                    on_delta(delta.clone());
                    full.push_str(&delta);
                }
                if chunk.done {
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
            let delta = chunk
                .message
                .map(|message| message.content)
                .or(chunk.response)
                .unwrap_or_default();
            if !delta.is_empty() {
                on_delta(delta.clone());
                full.push_str(&delta);
            }
        }

        Ok(full)
    }
}

pub fn system_prompt() -> ChatMessage {
    ChatMessage {
        role: "system".to_string(),
        content: r#"You are Ollo Code, a local autonomous coding agent running in a terminal.

You may inspect and modify files by emitting exactly one fenced JSON tool call when a tool is needed.
After a tool result is returned, continue from the result. Do not invent tool results.

Tool call format:
```json
{"tool":"read_file","path":"src/main.rs"}
```

Supported tools:
- list_files: {"tool":"list_files","path":"."}
- read_file: {"tool":"read_file","path":"relative/path"}
- write_file: {"tool":"write_file","path":"relative/path","content":"full file content"}
- apply_patch: {"tool":"apply_patch","patch":"*** Begin Patch\n..."}
- run_command: {"tool":"run_command","command":"cargo check"}

Prefer reading before editing. Keep changes focused."#
            .to_string(),
    }
}

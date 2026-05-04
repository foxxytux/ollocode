use anyhow::{Context, Result, bail};
use futures_util::{StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Model {
    pub fn supports_tools(&self) -> bool {
        self.capabilities.iter().any(|capability| {
            capability.eq_ignore_ascii_case("tools") || capability.contains("tool")
        })
    }
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

#[derive(Debug, Deserialize)]
struct ShowResponse {
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OllamaToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OllamaToolSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OllamaToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OllamaToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OllamaToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    pub function: OllamaToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OllamaToolCallFunction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    pub name: String,
    pub arguments: Value,
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
    tools: &'a [OllamaToolSpec],
    think: bool,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    message: Option<ChatMessage>,
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

        let client = self.clone();
        let capability_futures = models.iter().map(move |model| {
            let client = client.clone();
            let name = model.name.clone();
            async move {
                let capabilities = client
                    .show_model_capabilities(&name)
                    .await
                    .unwrap_or_default();
                (name, capabilities)
            }
        });
        let capabilities = join_all(capability_futures).await;

        for model in &mut models {
            if let Some((_, capability_list)) =
                capabilities.iter().find(|(name, _)| name == &model.name)
            {
                model.capabilities = capability_list.clone();
            }
        }

        models.retain(Model::supports_tools);
        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(models)
    }

    async fn show_model_capabilities(&self, model: &str) -> Result<Vec<String>> {
        let url = format!("{}/api/show", self.base_url);
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
            .context("failed to connect to Ollama")?
            .error_for_status()
            .context("Ollama returned an error while loading model details")?;
        let response = response.json::<ShowResponse>().await?;
        Ok(response.capabilities)
    }

    pub async fn chat_stream<F>(
        &self,
        model: &str,
        messages: &[ChatMessage],
        tools: &[OllamaToolSpec],
        mut on_delta: F,
    ) -> Result<ChatMessage>
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
            tools,
            think: true,
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
        let mut content = String::new();
        let mut thinking = String::new();
        let mut tool_calls = Vec::new();

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
                    if let Some(delta) = message.thinking.filter(|value| !value.is_empty()) {
                        thinking.push_str(&delta);
                        on_delta(ChatDelta::Thinking(delta));
                    }
                    if !message.content.is_empty() {
                        content.push_str(&message.content);
                        on_delta(ChatDelta::Content(message.content));
                    }
                    if !message.tool_calls.is_empty() {
                        tool_calls.extend(message.tool_calls);
                    }
                }
                if chunk.done {
                    return Ok(ChatMessage {
                        role: "assistant".to_string(),
                        content,
                        thinking: if thinking.is_empty() {
                            None
                        } else {
                            Some(thinking)
                        },
                        tool_calls,
                        tool_name: None,
                    });
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
                if let Some(delta) = message.thinking.filter(|value| !value.is_empty()) {
                    thinking.push_str(&delta);
                    on_delta(ChatDelta::Thinking(delta));
                }
                if !message.content.is_empty() {
                    content.push_str(&message.content);
                    on_delta(ChatDelta::Content(message.content));
                }
                if !message.tool_calls.is_empty() {
                    tool_calls.extend(message.tool_calls);
                }
            }
        }

        Ok(ChatMessage {
            role: "assistant".to_string(),
            content,
            thinking: if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            tool_calls,
            tool_name: None,
        })
    }
}

pub fn assistant_tool_call_summary(tool_calls: &[OllamaToolCall]) -> String {
    if tool_calls.is_empty() {
        return String::new();
    }

    tool_calls
        .iter()
        .map(|call| {
            let arguments = if call.function.arguments.is_null() {
                String::new()
            } else {
                format!(" {}", call.function.arguments)
            };
            format!("Requested tool: {}{}", call.function.name, arguments)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn standard_tools() -> Vec<OllamaToolSpec> {
    vec![
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "bash".to_string(),
                description: "Run a shell command in the workspace root.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to run"
                        }
                    }
                }),
            },
        },
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "read".to_string(),
                description: "Read a file or list a directory in the workspace.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative file or directory path"
                        }
                    }
                }),
            },
        },
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "write".to_string(),
                description: "Write full file contents to a workspace path.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative file path"
                        },
                        "content": {
                            "type": "string",
                            "description": "Full file content to write"
                        }
                    }
                }),
            },
        },
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "edit".to_string(),
                description: "Replace exact text once in a workspace file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["path", "old", "new"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative file path"
                        },
                        "old": {
                            "type": "string",
                            "description": "Exact text to replace"
                        },
                        "new": {
                            "type": "string",
                            "description": "Replacement text"
                        }
                    }
                }),
            },
        },
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "list".to_string(),
                description: "List the contents of a directory in the workspace.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative directory path"
                        }
                    }
                }),
            },
        },
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "search".to_string(),
                description: "Search for text in the workspace with ripgrep.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative path to limit the search"
                        }
                    }
                }),
            },
        },
        OllamaToolSpec {
            kind: "function".to_string(),
            function: OllamaToolFunction {
                name: "patch".to_string(),
                description: "Apply an apply_patch-style patch.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "required": ["patch"],
                    "properties": {
                        "patch": {
                            "type": "string",
                            "description": "Patch text in apply_patch format"
                        }
                    }
                }),
            },
        },
    ]
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
Do not output evaluation JSON, benchmark metadata, or self-critique wrappers. Reply in plain language unless you are emitting a tool response.
Use the provided native tools when you need to inspect or change the workspace. Do not invent your own tool syntax in assistant content.
Prefer the smallest useful tool call, and continue from tool results until the task is complete.
When you receive a tool result, either answer the user directly or request a different tool if it adds new information. Do not repeat the same tool call with the same arguments, and do not keep calling tools once the task is already answered.
"#
            .to_string();
            prompt.push_str(&agents_section);
            prompt
        },
        thinking: None,
        tool_calls: Vec::new(),
        tool_name: None,
    }
}

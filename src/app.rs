use crate::{
    agents,
    config::{Config, ConversationState},
    ollama::{ChatDelta, ChatMessage, Model, OllamaClient, system_prompt},
    tools::{ToolCall, ToolRunner, extract_tool_call},
};
use anyhow::Result;
use chrono::{DateTime, Local};
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct TranscriptItem {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Local>,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ModelsLoaded(Result<Vec<Model>, String>),
    AssistantDelta(String),
    AssistantThinkingDelta(String),
    AssistantDone(Result<String, String>),
    ToolDone(String),
    CommandDone(String),
}

pub struct App {
    pub cwd: PathBuf,
    pub config: Config,
    pub client: OllamaClient,
    pub tools: ToolRunner,
    pub models: Vec<Model>,
    pub selected_model: Option<String>,
    pub input: String,
    pub input_cursor: usize,
    pub command_cursor: usize,
    pub history_cursor: Option<usize>,
    pub transcript: Vec<TranscriptItem>,
    pub transcript_scroll: usize,
    pub status: String,
    pub busy: bool,
    pub should_quit: bool,
    pub tx: mpsc::UnboundedSender<AppEvent>,
    pub rx: mpsc::UnboundedReceiver<AppEvent>,
    messages: Vec<ChatMessage>,
    pending_assistant: String,
    response_start: Option<usize>,
    stream_role: Option<String>,
    agents_md: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "/help",
        usage: "/help",
        description: "show commands",
    },
    CommandSpec {
        name: "/init",
        usage: "/init",
        description: "create AGENTS.md",
    },
    CommandSpec {
        name: "/agents",
        usage: "/agents",
        description: "reload AGENTS.md",
    },
    CommandSpec {
        name: "/tools",
        usage: "/tools",
        description: "show model-callable tools",
    },
    CommandSpec {
        name: "/model",
        usage: "/model <name>",
        description: "switch model",
    },
    CommandSpec {
        name: "/models",
        usage: "/models",
        description: "list Ollama models",
    },
    CommandSpec {
        name: "/bash",
        usage: "/bash <command>",
        description: "run shell command",
    },
    CommandSpec {
        name: "/read",
        usage: "/read <path>",
        description: "read file or list directory",
    },
    CommandSpec {
        name: "/clear",
        usage: "/clear",
        description: "clear transcript",
    },
    CommandSpec {
        name: "/context",
        usage: "/context",
        description: "show restored context usage",
    },
    CommandSpec {
        name: "/pwd",
        usage: "/pwd",
        description: "show workspace path",
    },
];

impl App {
    pub async fn new(
        cwd: PathBuf,
        config: Config,
        client: OllamaClient,
        tools: ToolRunner,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let agents_md = agents::load(&cwd);
        let restored = ConversationState::load(&cwd).unwrap_or_default();
        let mut messages = vec![system_prompt(agents_md.as_deref())];
        messages.extend(
            restored
                .messages
                .into_iter()
                .filter(|message| message.role != "system"),
        );
        let mut app = Self {
            cwd,
            selected_model: config.selected_model.clone(),
            config,
            client,
            tools,
            models: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            command_cursor: 0,
            history_cursor: None,
            transcript: Vec::new(),
            transcript_scroll: 0,
            status: "Loading Ollama models".to_string(),
            busy: false,
            should_quit: false,
            tx,
            rx,
            messages,
            pending_assistant: String::new(),
            response_start: None,
            stream_role: None,
            agents_md,
        };
        app.restore_transcript_from_messages();
        app.refresh_models();
        app.report_agents_status();
        app
    }

    pub fn refresh_models(&mut self) {
        self.status = "Refreshing Ollama models".to_string();
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client
                .list_models()
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(AppEvent::ModelsLoaded(result));
        });
    }

    pub fn select_next_model(&mut self) {
        self.select_model(1);
    }

    pub fn select_previous_model(&mut self) {
        self.select_model(-1);
    }

    fn select_model(&mut self, delta: isize) {
        if self.models.is_empty() {
            self.status = "No Ollama models available".to_string();
            return;
        }
        let current = self
            .selected_model
            .as_ref()
            .and_then(|name| self.models.iter().position(|model| &model.name == name))
            .unwrap_or(0);
        let len = self.models.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        self.selected_model = Some(self.models[next].name.clone());
        self.config.selected_model = self.selected_model.clone();
        self.status = format!("Selected {}", self.models[next].name);
        let _ = self.config.save();
    }

    pub fn select_model_index(&mut self, index: usize) {
        if let Some(model) = self.models.get(index) {
            self.selected_model = Some(model.name.clone());
            self.config.selected_model = self.selected_model.clone();
            self.status = format!("Selected {}", model.name);
            let _ = self.config.save();
        }
    }

    pub fn context_percent(&self) -> Option<u16> {
        let limit = self.selected_model_context_limit()?;
        let used = self.approx_context_tokens();
        Some(((used.saturating_mul(100)) / limit).min(999) as u16)
    }

    pub fn context_label(&self) -> String {
        match (self.context_percent(), self.selected_model_context_limit()) {
            (Some(percent), Some(limit)) => {
                format!("ctx {percent}% ~{}/{}", self.approx_context_tokens(), limit)
            }
            _ => format!("ctx ~{} tokens", self.approx_context_tokens()),
        }
    }

    pub fn scroll_transcript(&mut self, delta: isize) {
        let max_scroll = self.transcript.len().saturating_mul(6);
        if delta < 0 {
            self.transcript_scroll = self.transcript_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.transcript_scroll = (self.transcript_scroll + delta as usize).min(max_scroll);
        }
    }

    pub fn input_insert(&mut self, ch: char) {
        self.history_cursor = None;
        self.input.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
        self.clamp_command_cursor();
    }

    pub fn input_insert_str(&mut self, text: &str) {
        self.history_cursor = None;
        self.input.insert_str(self.input_cursor, text);
        self.input_cursor += text.len();
        self.clamp_command_cursor();
    }

    pub fn input_backspace(&mut self) {
        self.history_cursor = None;
        if let Some(previous) = previous_boundary(&self.input, self.input_cursor) {
            self.input.drain(previous..self.input_cursor);
            self.input_cursor = previous;
        }
        self.clamp_command_cursor();
    }

    pub fn input_delete(&mut self) {
        self.history_cursor = None;
        if let Some(next) = next_boundary(&self.input, self.input_cursor) {
            self.input.drain(self.input_cursor..next);
        }
        self.clamp_command_cursor();
    }

    pub fn input_left(&mut self) {
        if let Some(previous) = previous_boundary(&self.input, self.input_cursor) {
            self.input_cursor = previous;
        }
    }

    pub fn input_right(&mut self) {
        if let Some(next) = next_boundary(&self.input, self.input_cursor) {
            self.input_cursor = next;
        }
    }

    pub fn input_home(&mut self) {
        self.input_cursor = 0;
    }

    pub fn input_end(&mut self) {
        self.input_cursor = self.input.len();
    }

    pub fn history_previous(&mut self) {
        if self.config.prompt_history.is_empty() {
            return;
        }
        let next = self
            .history_cursor
            .map(|cursor| cursor.saturating_sub(1))
            .unwrap_or_else(|| self.config.prompt_history.len().saturating_sub(1));
        self.history_cursor = Some(next);
        self.input = self.config.prompt_history[next].clone();
        self.input_cursor = self.input.len();
    }

    pub fn history_next(&mut self) {
        let Some(cursor) = self.history_cursor else {
            return;
        };
        if cursor + 1 >= self.config.prompt_history.len() {
            self.history_cursor = None;
            self.input.clear();
        } else {
            self.history_cursor = Some(cursor + 1);
            self.input = self.config.prompt_history[cursor + 1].clone();
        }
        self.input_cursor = self.input.len();
    }

    pub fn commands_active(&self) -> bool {
        self.input.starts_with('/')
    }

    pub fn command_selection_up(&mut self) {
        if !self.commands_active() {
            return;
        }
        let len = self.command_suggestions().len();
        if len == 0 {
            self.command_cursor = 0;
        } else {
            self.command_cursor = self.command_cursor.saturating_sub(1);
        }
    }

    pub fn command_selection_down(&mut self) {
        if !self.commands_active() {
            return;
        }
        let len = self.command_suggestions().len();
        if len == 0 {
            self.command_cursor = 0;
        } else {
            self.command_cursor = (self.command_cursor + 1).min(len - 1);
        }
    }

    pub fn selected_command_index(&self) -> usize {
        let len = self.command_suggestions().len();
        if len == 0 {
            0
        } else {
            self.command_cursor.min(len - 1)
        }
    }

    pub fn submit_prompt(&mut self) {
        if self.busy {
            self.status = "Wait for the current request to finish".to_string();
            return;
        }
        if self.complete_selected_command() {
            return;
        }
        let prompt = self.input.trim().to_string();
        if prompt.is_empty() {
            return;
        }

        self.input.clear();
        self.input_cursor = 0;
        self.command_cursor = 0;
        self.history_cursor = None;
        self.config.prompt_history.push(prompt.clone());
        if self.config.prompt_history.len() > 100 {
            self.config.prompt_history.remove(0);
        }
        let _ = self.config.save();

        if prompt.starts_with('/') {
            self.handle_command(&prompt);
            return;
        }

        let Some(model) = self.selected_model.clone() else {
            self.status = "Select an Ollama model first".to_string();
            return;
        };

        self.push_transcript("user", prompt.clone());
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: prompt,
        });
        self.save_conversation();
        self.start_chat(model);
    }

    fn start_chat(&mut self, model: String) {
        self.busy = true;
        self.transcript_scroll = 0;
        self.pending_assistant.clear();
        self.response_start = Some(self.transcript.len());
        self.stream_role = None;
        self.status = format!("Streaming from {model}");
        let client = self.client.clone();
        let messages = self.messages.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let tx_delta = tx.clone();
            let result = client
                .chat_stream(&model, &messages, move |delta| {
                    let event = match delta {
                        ChatDelta::Content(content) => AppEvent::AssistantDelta(content),
                        ChatDelta::Thinking(thinking) => AppEvent::AssistantThinkingDelta(thinking),
                    };
                    let _ = tx_delta.send(event);
                })
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(AppEvent::AssistantDone(result));
        });
    }

    pub async fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.handle_event(event).await;
        }
    }

    async fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ModelsLoaded(Ok(models)) => {
                self.models = models;
                if self.models.is_empty() {
                    self.status = "No Ollama models found. Run `ollama pull <model>`.".to_string();
                    self.selected_model = None;
                } else if self
                    .selected_model
                    .as_ref()
                    .is_none_or(|selected| !self.models.iter().any(|model| &model.name == selected))
                {
                    self.selected_model = Some(self.models[0].name.clone());
                    self.config.selected_model = self.selected_model.clone();
                    let _ = self.config.save();
                    self.status = format!("Selected {}", self.models[0].name);
                } else {
                    self.status = format!("Loaded {} Ollama models", self.models.len());
                }
            }
            AppEvent::ModelsLoaded(Err(error)) => {
                self.status = format!("Ollama error: {error}");
            }
            AppEvent::AssistantDelta(delta) => {
                self.transcript_scroll = 0;
                self.pending_assistant.push_str(&delta);
                self.append_stream_delta("assistant", delta);
            }
            AppEvent::AssistantThinkingDelta(delta) => {
                self.transcript_scroll = 0;
                self.append_stream_delta("thinking", delta);
            }
            AppEvent::AssistantDone(Ok(content)) => {
                self.busy = false;
                self.stream_role = None;
                self.messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: content.clone(),
                });
                self.save_conversation();
                self.replace_latest_assistant_with_parts(&content);
                self.status = "Assistant response complete".to_string();

                match extract_tool_call(&content) {
                    Ok(Some(call)) => {
                        self.status = "Running requested tool".to_string();
                        let tools = self.tools.clone();
                        let tx = self.tx.clone();
                        tokio::spawn(async move {
                            let result = tools.run(call).await;
                            let content = serde_json::to_string_pretty(&result)
                                .unwrap_or_else(|_| "{\"ok\":false}".to_string());
                            let _ = tx.send(AppEvent::ToolDone(content));
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.status = format!("Tool parse error: {error:#}");
                    }
                }
            }
            AppEvent::AssistantDone(Err(error)) => {
                self.busy = false;
                self.response_start = None;
                self.stream_role = None;
                self.status = format!("Chat error: {error}");
                if let Some(item) = self.transcript.last_mut() {
                    if item.role == "assistant" && item.content.is_empty() {
                        item.content = self.status.clone();
                    }
                }
            }
            AppEvent::ToolDone(content) => {
                self.push_transcript("tool", content.clone());
                self.messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: format!(
                        "Tool result for your previous tool call:\n{content}\nContinue from this result. If ok is false, recover by using another supported tool or explain the failure."
                    ),
                });
                self.save_conversation();
                if let Some(model) = self.selected_model.clone() {
                    self.start_chat(model);
                }
            }
            AppEvent::CommandDone(content) => {
                self.push_transcript("system", content.clone());
                self.status = content
                    .lines()
                    .next()
                    .unwrap_or("Command complete")
                    .to_string();
            }
        }
    }

    fn push_transcript(&mut self, role: &str, content: String) {
        self.transcript_scroll = 0;
        self.transcript.push(TranscriptItem {
            role: role.to_string(),
            content,
            timestamp: Local::now(),
        });
    }

    fn replace_latest_assistant_with_parts(&mut self, content: &str) {
        let (thinking, assistant) = split_thinking(content);
        if let Some(start) = self.response_start.take() {
            self.transcript.truncate(start);
        } else if matches!(self.transcript.last(), Some(item) if item.role == "assistant") {
            self.transcript.pop();
        }
        if !thinking.trim().is_empty() {
            self.push_transcript("thinking", thinking);
        }
        if !assistant.trim().is_empty() {
            self.push_transcript("assistant", assistant);
        }
    }

    fn append_stream_delta(&mut self, role: &str, delta: String) {
        match self.stream_role.as_deref() {
            Some(current) if current == role => {
                if let Some(item) = self.transcript.last_mut() {
                    item.content.push_str(&delta);
                    return;
                }
            }
            Some(_) => {}
            None => {}
        }

        self.stream_role = Some(role.to_string());
        self.push_transcript(role, delta);
    }

    fn handle_command(&mut self, command: &str) {
        self.push_transcript("command", command.to_string());
        let mut parts = command.split_whitespace();
        let name = parts.next().unwrap_or_default();
        let output = match name {
            "/help" => self.command_help(),
            "/init" => self.command_init(),
            "/agents" => self.command_agents(),
            "/tools" => self.command_tools(),
            "/model" => {
                let model = parts.collect::<Vec<_>>().join(" ");
                self.command_model(&model)
            }
            "/models" => self.command_models(),
            "/bash" => {
                let command = parts.collect::<Vec<_>>().join(" ");
                self.command_tool(ToolCall::Bash { command }, "Running bash command")
            }
            "/read" => {
                let path = parts.collect::<Vec<_>>().join(" ");
                self.command_tool(ToolCall::Read { path }, "Reading file")
            }
            "/clear" => {
                self.transcript.clear();
                self.messages.truncate(1);
                let _ = ConversationState::clear(&self.cwd);
                self.status = "Transcript cleared".to_string();
                return;
            }
            "/context" => self.context_status(),
            "/pwd" => self.cwd.display().to_string(),
            "/" => self.command_help(),
            unknown => format!("Unknown command `{unknown}`. Run /help for commands."),
        };
        self.push_transcript("system", output.clone());
        self.status = output
            .lines()
            .next()
            .unwrap_or("Command complete")
            .to_string();
    }

    fn command_help(&self) -> String {
        let mut lines = vec!["Commands:".to_string()];
        lines.extend(
            COMMANDS
                .iter()
                .map(|command| format!("{} - {}", command.usage, command.description)),
        );
        lines.join("\n")
    }

    fn command_tools(&self) -> String {
        [
            "Model tools:",
            "bash { command } - run a shell command",
            "read { path } - read a file",
            "write { path, content } - write a full file",
            "edit { path, old, new } - replace exact text once",
            "list { path } - list a directory",
            "search { query, path? } - search text with rg",
            "patch { patch } - apply an apply_patch-style patch",
            "",
            "Core tools guaranteed here: bash, read, write, edit.",
        ]
        .join("\n")
    }

    fn command_init(&mut self) -> String {
        match agents::init(&self.cwd) {
            Ok(path) => {
                self.reload_agents();
                format!("Created {}", path.display())
            }
            Err(error) => format!("{error:#}"),
        }
    }

    fn command_agents(&mut self) -> String {
        self.reload_agents();
        match &self.agents_md {
            Some(content) => format!(
                "Loaded AGENTS.md ({} bytes). Its instructions are included in future prompts.",
                content.len()
            ),
            None => "No AGENTS.md found. Run /init to create one.".to_string(),
        }
    }

    fn command_model(&mut self, model: &str) -> String {
        if model.trim().is_empty() {
            return "Usage: /model <exact model name>".to_string();
        }
        if let Some(index) = self.models.iter().position(|item| item.name == model) {
            self.select_model_index(index);
            format!("Selected {model}")
        } else {
            format!("Model `{model}` is not loaded. Run /models to see available models.")
        }
    }

    fn command_models(&self) -> String {
        if self.models.is_empty() {
            return "No models loaded. Run Ctrl+M to refresh Ollama models.".to_string();
        }
        self.models
            .iter()
            .map(|model| {
                if self.selected_model.as_deref() == Some(model.name.as_str()) {
                    format!("> {}", model.name)
                } else {
                    format!("  {}", model.name)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn command_tool(&mut self, call: ToolCall, status: &str) -> String {
        let tools = self.tools.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = tools.run(call).await;
            let prefix = if result.ok { "ok" } else { "error" };
            let _ = tx.send(AppEvent::CommandDone(format!(
                "{prefix}\n{}",
                result.output
            )));
        });
        status.to_string()
    }

    fn context_status(&self) -> String {
        format!(
            "{}\n{} persisted conversation messages",
            self.context_label(),
            self.messages
                .iter()
                .filter(|message| message.role != "system")
                .count()
        )
    }

    fn reload_agents(&mut self) {
        self.agents_md = agents::load(&self.cwd);
        if let Some(system) = self.messages.first_mut() {
            *system = system_prompt(self.agents_md.as_deref());
        }
        self.save_conversation();
    }

    fn report_agents_status(&mut self) {
        if self.agents_md.is_some() {
            self.push_transcript("system", "Loaded AGENTS.md instructions.".to_string());
        } else {
            self.push_transcript(
                "system",
                "No AGENTS.md found. Run /init to create one.".to_string(),
            );
        }
    }

    pub fn command_suggestions(&self) -> Vec<CommandSpec> {
        if !self.input.starts_with('/') {
            return Vec::new();
        }
        let prefix = self
            .input
            .split_whitespace()
            .next()
            .unwrap_or(self.input.as_str());
        let suggestions: Vec<CommandSpec> = COMMANDS
            .iter()
            .copied()
            .filter(|command| command.name.starts_with(prefix))
            .collect();
        if suggestions.is_empty() && prefix == "/" {
            COMMANDS.to_vec()
        } else {
            suggestions
        }
    }

    fn complete_selected_command(&mut self) -> bool {
        if !self.commands_active() {
            return false;
        }
        let prefix = self
            .input
            .split_whitespace()
            .next()
            .unwrap_or(self.input.as_str());
        if COMMANDS.iter().any(|command| command.name == prefix) {
            return false;
        }
        let suggestions = self.command_suggestions();
        let Some(command) = suggestions.get(self.selected_command_index()) else {
            return false;
        };
        self.input = format!("{} ", command.name);
        self.input_cursor = self.input.len();
        self.command_cursor = 0;
        self.status = format!("Completed {}", command.name);
        true
    }

    fn clamp_command_cursor(&mut self) {
        let len = self.command_suggestions().len();
        if len == 0 {
            self.command_cursor = 0;
        } else {
            self.command_cursor = self.command_cursor.min(len - 1);
        }
    }

    fn restore_transcript_from_messages(&mut self) {
        let restored: Vec<(String, String)> = self
            .messages
            .iter()
            .filter(|message| message.role != "system")
            .flat_map(|message| {
                let role = if message
                    .content
                    .starts_with("Tool result for your previous tool call:")
                {
                    "tool".to_string()
                } else {
                    message.role.clone()
                };
                if role == "assistant" {
                    let (thinking, assistant) = split_thinking(&message.content);
                    let mut items = Vec::new();
                    if !thinking.trim().is_empty() {
                        items.push(("thinking".to_string(), thinking));
                    }
                    if !assistant.trim().is_empty() {
                        items.push(("assistant".to_string(), assistant));
                    }
                    items
                } else {
                    vec![(role, message.content.clone())]
                }
            })
            .collect();
        for (role, content) in restored {
            self.push_transcript(&role, content);
        }
        if !self.transcript.is_empty() {
            self.status = format!("Restored {} conversation messages", self.transcript.len());
        }
    }

    fn save_conversation(&self) {
        let _ = ConversationState::save(&self.cwd, &self.messages);
    }

    fn approx_context_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|message| approximate_tokens(&message.content) + 4)
            .sum()
    }

    fn selected_model_context_limit(&self) -> Option<usize> {
        let name = self.selected_model.as_ref()?;
        let lower = name.to_lowercase();
        let details = self
            .models
            .iter()
            .find(|model| &model.name == name)
            .and_then(|model| model.details.as_ref());
        let params = details
            .and_then(|details| details.parameter_size.as_deref())
            .unwrap_or(&lower)
            .to_lowercase();

        if lower.contains("granite4") || lower.contains("qwen3.5") || lower.contains("gemma4") {
            Some(32_768)
        } else if lower.contains("nemotron") {
            Some(128_000)
        } else if params.contains("0.4b") || params.contains("350m") {
            Some(8_192)
        } else if params.contains("0.8b") || params.contains("2b") || params.contains("4b") {
            Some(32_768)
        } else {
            Some(8_192)
        }
    }
}

fn approximate_tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

fn split_thinking(content: &str) -> (String, String) {
    let mut thinking = String::new();
    let mut assistant = String::new();
    let mut rest = content;

    loop {
        let Some(start) = rest.find("<think>") else {
            if !rest.trim().is_empty() {
                if !assistant.is_empty() {
                    assistant.push('\n');
                }
                assistant.push_str(rest.trim());
            }
            break;
        };

        let before = &rest[..start];
        if !before.trim().is_empty() {
            if !assistant.is_empty() {
                assistant.push('\n');
            }
            assistant.push_str(before.trim());
        }

        let thinking_start = start + "<think>".len();
        if let Some(end) = rest[thinking_start..].find("</think>") {
            let thinking_end = thinking_start + end;
            let block = rest[thinking_start..thinking_end].trim();
            if !block.is_empty() {
                if !thinking.is_empty() {
                    thinking.push('\n');
                }
                thinking.push_str(block);
            }
            rest = &rest[thinking_end + "</think>".len()..];
        } else {
            let block = rest[thinking_start..].trim();
            if !block.is_empty() {
                if !thinking.is_empty() {
                    thinking.push('\n');
                }
                thinking.push_str(block);
            }
            break;
        }
    }

    (thinking, assistant)
}

fn previous_boundary(text: &str, cursor: usize) -> Option<usize> {
    if cursor == 0 {
        return None;
    }
    text[..cursor].char_indices().last().map(|(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> Option<usize> {
    if cursor >= text.len() {
        return None;
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .or(Some(text.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let (tx, rx) = mpsc::unbounded_channel();
        let cwd = std::env::current_dir().unwrap();
        App {
            cwd: cwd.clone(),
            selected_model: None,
            config: Config::default(),
            client: OllamaClient::new("http://127.0.0.1:11434".to_string()),
            tools: ToolRunner::new(cwd),
            models: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            command_cursor: 0,
            history_cursor: None,
            transcript: Vec::new(),
            transcript_scroll: 0,
            status: String::new(),
            busy: false,
            should_quit: false,
            tx,
            rx,
            messages: vec![system_prompt(None)],
            pending_assistant: String::new(),
            response_start: None,
            stream_role: None,
            agents_md: None,
        }
    }

    #[test]
    fn slash_predicts_all_commands() {
        let mut app = test_app();
        app.input = "/".to_string();
        app.input_cursor = app.input.len();

        let suggestions = app.command_suggestions();
        assert!(suggestions.iter().any(|command| command.name == "/init"));
        assert!(suggestions.iter().any(|command| command.name == "/bash"));
    }

    #[test]
    fn partial_slash_predicts_matching_commands() {
        let mut app = test_app();
        app.input = "/mo".to_string();
        app.input_cursor = app.input.len();

        let names: Vec<&str> = app
            .command_suggestions()
            .iter()
            .map(|command| command.name)
            .collect();
        assert_eq!(names, vec!["/model", "/models"]);
    }

    #[test]
    fn arrows_scroll_command_selection() {
        let mut app = test_app();
        app.input = "/mo".to_string();
        app.input_cursor = app.input.len();

        assert_eq!(app.selected_command_index(), 0);
        app.command_selection_down();
        assert_eq!(app.selected_command_index(), 1);
        app.command_selection_down();
        assert_eq!(app.selected_command_index(), 1);
        app.command_selection_up();
        assert_eq!(app.selected_command_index(), 0);
    }

    #[test]
    fn context_percent_uses_selected_model() {
        let mut app = test_app();
        app.selected_model = Some("granite4:350m".to_string());
        app.models = vec![Model {
            name: "granite4:350m".to_string(),
            size: None,
            modified_at: None,
            details: None,
        }];
        app.messages.push(ChatMessage {
            role: "user".to_string(),
            content: "hello".repeat(100),
        });

        assert!(app.context_percent().is_some());
        assert!(app.context_label().starts_with("ctx "));
    }

    #[test]
    fn splits_thinking_blocks_for_display() {
        let (thinking, assistant) = split_thinking("<think>checking files</think>\nDone.");
        assert_eq!(thinking, "checking files");
        assert_eq!(assistant, "Done.");
    }

    #[test]
    fn enter_completes_partial_command() {
        let mut app = test_app();
        app.input = "/mo".to_string();
        app.input_cursor = app.input.len();
        app.command_selection_down();

        assert!(app.complete_selected_command());
        assert_eq!(app.input, "/models ");
    }

    #[test]
    fn exact_command_does_not_autocomplete() {
        let mut app = test_app();
        app.input = "/models".to_string();
        app.input_cursor = app.input.len();

        assert!(!app.complete_selected_command());
    }
}

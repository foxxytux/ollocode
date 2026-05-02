use crate::{
    config::Config,
    ollama::{ChatMessage, Model, OllamaClient, system_prompt},
    tools::{ToolRunner, extract_tool_call},
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
    AssistantDone(Result<String, String>),
    ToolDone(String),
}

pub struct App {
    pub cwd: PathBuf,
    pub config: Config,
    pub client: OllamaClient,
    pub tools: ToolRunner,
    pub models: Vec<Model>,
    pub selected_model: Option<String>,
    pub input: String,
    pub transcript: Vec<TranscriptItem>,
    pub status: String,
    pub busy: bool,
    pub should_quit: bool,
    pub tx: mpsc::UnboundedSender<AppEvent>,
    pub rx: mpsc::UnboundedReceiver<AppEvent>,
    messages: Vec<ChatMessage>,
    pending_assistant: String,
}

impl App {
    pub async fn new(
        cwd: PathBuf,
        config: Config,
        client: OllamaClient,
        tools: ToolRunner,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = Self {
            cwd,
            selected_model: config.selected_model.clone(),
            config,
            client,
            tools,
            models: Vec::new(),
            input: String::new(),
            transcript: Vec::new(),
            status: "Loading Ollama models".to_string(),
            busy: false,
            should_quit: false,
            tx,
            rx,
            messages: vec![system_prompt()],
            pending_assistant: String::new(),
        };
        app.refresh_models();
        app
    }

    pub fn refresh_models(&mut self) {
        self.status = "Refreshing Ollama models".to_string();
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.list_models().await.map_err(|error| format!("{error:#}"));
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

    pub fn submit_prompt(&mut self) {
        if self.busy {
            self.status = "Wait for the current request to finish".to_string();
            return;
        }
        let prompt = self.input.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let Some(model) = self.selected_model.clone() else {
            self.status = "Select an Ollama model first".to_string();
            return;
        };

        self.input.clear();
        self.config.prompt_history.push(prompt.clone());
        if self.config.prompt_history.len() > 100 {
            self.config.prompt_history.remove(0);
        }
        let _ = self.config.save();

        self.push_transcript("user", prompt.clone());
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: prompt,
        });
        self.start_chat(model);
    }

    fn start_chat(&mut self, model: String) {
        self.busy = true;
        self.pending_assistant.clear();
        self.push_transcript("assistant", String::new());
        self.status = format!("Streaming from {model}");
        let client = self.client.clone();
        let messages = self.messages.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let tx_delta = tx.clone();
            let result = client
                .chat_stream(&model, &messages, move |delta| {
                    let _ = tx_delta.send(AppEvent::AssistantDelta(delta));
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
                self.pending_assistant.push_str(&delta);
                if let Some(item) = self.transcript.last_mut() {
                    item.content.push_str(&delta);
                }
            }
            AppEvent::AssistantDone(Ok(content)) => {
                self.busy = false;
                self.messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: content.clone(),
                });
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
                    role: "tool".to_string(),
                    content: content.clone(),
                });
                if let Some(model) = self.selected_model.clone() {
                    self.start_chat(model);
                }
            }
        }
    }

    fn push_transcript(&mut self, role: &str, content: String) {
        self.transcript.push(TranscriptItem {
            role: role.to_string(),
            content,
            timestamp: Local::now(),
        });
    }
}

use crate::ollama::ChatMessage;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

const MAX_PERSISTED_MESSAGES: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ollama_host: String,
    pub selected_model: Option<String>,
    pub prompt_history: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama_host: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
            selected_model: None,
            prompt_history: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self =
            toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
        if let Ok(host) = std::env::var("OLLAMA_HOST") {
            config.ollama_host = host;
        }
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self)?;
        fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("ollocode").join("config.toml"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationState {
    pub messages: Vec<ChatMessage>,
}

impl ConversationState {
    pub fn load(cwd: &Path) -> Result<Self> {
        let Some(path) = conversation_path(cwd) else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn save(cwd: &Path, messages: &[ChatMessage]) -> Result<()> {
        let Some(path) = conversation_path(cwd) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let start = messages.len().saturating_sub(MAX_PERSISTED_MESSAGES);
        let state = Self {
            messages: messages[start..].to_vec(),
        };
        let raw = toml::to_string_pretty(&state)?;
        fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn clear(cwd: &Path) -> Result<()> {
        let Some(path) = conversation_path(cwd) else {
            return Ok(());
        };
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
        Ok(())
    }
}

fn conversation_path(cwd: &Path) -> Option<PathBuf> {
    dirs::config_dir().map(|dir| {
        dir.join("ollocode")
            .join("conversations")
            .join(format!("{}.toml", workspace_id(cwd)))
    })
}

fn workspace_id(cwd: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    cwd.display().to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

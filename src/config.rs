use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

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
        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
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

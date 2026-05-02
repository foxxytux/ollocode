use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct ToolRunner {
    root: PathBuf,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolCall {
    ListFiles { path: String },
    ReadFile { path: String },
    WriteFile { path: String, content: String },
    ApplyPatch { patch: String },
    RunCommand { command: String },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

impl ToolRunner {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn run(&self, call: ToolCall) -> ToolResult {
        match self.run_inner(call).await {
            Ok(output) => ToolResult { ok: true, output },
            Err(error) => ToolResult {
                ok: false,
                output: format!("{error:#}"),
            },
        }
    }

    async fn run_inner(&self, call: ToolCall) -> Result<String> {
        match call {
            ToolCall::ListFiles { path } => self.list_files(&path),
            ToolCall::ReadFile { path } => self.read_file(&path),
            ToolCall::WriteFile { path, content } => self.write_file(&path, &content),
            ToolCall::ApplyPatch { patch } => self.apply_patch(&patch).await,
            ToolCall::RunCommand { command } => self.run_command(&command).await,
        }
    }

    fn list_files(&self, path: &str) -> Result<String> {
        let path = self.workspace_path(path)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&path).with_context(|| format!("failed to read {}", path.display()))? {
            let entry = entry?;
            let kind = if entry.file_type()?.is_dir() { "/" } else { "" };
            entries.push(format!("{}{}", entry.file_name().to_string_lossy(), kind));
        }
        entries.sort();
        Ok(entries.join("\n"))
    }

    fn read_file(&self, path: &str) -> Result<String> {
        let path = self.workspace_path(path)?;
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
    }

    fn write_file(&self, path: &str, content: &str) -> Result<String> {
        let path = self.workspace_path(path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(format!("wrote {}", path.strip_prefix(&self.root).unwrap_or(&path).display()))
    }

    async fn apply_patch(&self, patch: &str) -> Result<String> {
        if !patch.contains("*** Begin Patch") || !patch.contains("*** End Patch") {
            bail!("patch must use apply_patch format");
        }

        let mut child = Command::new("apply_patch")
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn apply_patch")?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(patch.as_bytes()).await?;
        }

        let output = child.wait_with_output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            bail!("apply_patch failed\n{stdout}{stderr}");
        }
        Ok(format!("{stdout}{stderr}"))
    }

    async fn run_command(&self, command: &str) -> Result<String> {
        let output = Command::new("/bin/sh")
            .arg("-lc")
            .arg(command)
            .current_dir(&self.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("failed to run command: {command}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let text = format!("status: {}\n{}{}", output.status, stdout, stderr);
        if !output.status.success() {
            bail!(text);
        }
        Ok(text)
    }

    pub fn workspace_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        if path.is_absolute() {
            bail!("absolute paths are not allowed");
        }
        for component in path.components() {
            if matches!(component, Component::ParentDir | Component::Prefix(_) | Component::RootDir) {
                bail!("path escapes workspace");
            }
        }
        Ok(self.root.join(path))
    }
}

pub fn extract_tool_call(text: &str) -> Result<Option<ToolCall>> {
    for block in fenced_json_blocks(text) {
        if let Ok(call) = serde_json::from_str::<ToolCall>(&block) {
            return Ok(Some(call));
        }
    }
    if let Ok(call) = serde_json::from_str::<ToolCall>(text.trim()) {
        return Ok(Some(call));
    }
    Ok(None)
}

fn fenced_json_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed.starts_with("```json") {
            in_block = true;
            current.clear();
            continue;
        }
        if in_block && trimmed == "```" {
            in_block = false;
            blocks.push(current.trim().to_string());
            current.clear();
            continue;
        }
        if in_block {
            current.push_str(line);
            current.push('\n');
        }
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_tool_call() {
        let text = "Use this:\n```json\n{\"tool\":\"read_file\",\"path\":\"Cargo.toml\"}\n```";
        let call = extract_tool_call(text).unwrap().unwrap();
        assert_eq!(
            call,
            ToolCall::ReadFile {
                path: "Cargo.toml".to_string()
            }
        );
    }

    #[test]
    fn rejects_path_escape() {
        let root = tempfile::tempdir().unwrap();
        let runner = ToolRunner::new(root.path().to_path_buf());
        assert!(runner.workspace_path("../secret").is_err());
        assert!(runner.workspace_path("/tmp/secret").is_err());
    }
}

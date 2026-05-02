use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCall {
    Bash {
        command: String,
    },
    Read {
        path: String,
    },
    Write {
        path: String,
        content: String,
    },
    Edit {
        path: String,
        old: String,
        new: String,
    },
    List {
        path: String,
    },
    Search {
        query: String,
        path: Option<String>,
    },
    Patch {
        patch: String,
    },
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
            ToolCall::Bash { command } => self.run_command(&command).await,
            ToolCall::Read { path } => self.read_file(&path),
            ToolCall::Write { path, content } => self.write_file(&path, &content),
            ToolCall::Edit { path, old, new } => self.edit_file(&path, &old, &new),
            ToolCall::List { path } => self.list_files(&path),
            ToolCall::Search { query, path } => self.search(&query, path.as_deref()).await,
            ToolCall::Patch { patch } => self.apply_patch(&patch),
        }
    }

    fn list_files(&self, path: &str) -> Result<String> {
        let path = self.workspace_path(path)?;
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&path).with_context(|| format!("failed to read {}", path.display()))?
        {
            let entry = entry?;
            let kind = if entry.file_type()?.is_dir() { "/" } else { "" };
            entries.push(format!("{}{}", entry.file_name().to_string_lossy(), kind));
        }
        entries.sort();
        Ok(entries.join("\n"))
    }

    fn read_file(&self, path: &str) -> Result<String> {
        let path = self.workspace_path(path)?;
        if path.is_dir() {
            let relative = path.strip_prefix(&self.root).unwrap_or(&path);
            return self.list_files(relative.to_string_lossy().as_ref());
        }
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
    }

    fn write_file(&self, path: &str, content: &str) -> Result<String> {
        let path = self.workspace_path(path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(format!(
            "wrote {}",
            path.strip_prefix(&self.root).unwrap_or(&path).display()
        ))
    }

    fn edit_file(&self, path: &str, old: &str, new: &str) -> Result<String> {
        if old.is_empty() {
            bail!("old text cannot be empty");
        }
        let path_buf = self.workspace_path(path)?;
        let content = fs::read_to_string(&path_buf)
            .with_context(|| format!("failed to read {}", path_buf.display()))?;
        if !content.contains(old) {
            bail!("old text was not found in {path}");
        }
        let updated = content.replacen(old, new, 1);
        fs::write(&path_buf, updated)
            .with_context(|| format!("failed to write {}", path_buf.display()))?;
        Ok(format!("edited {path}"))
    }

    fn apply_patch(&self, patch: &str) -> Result<String> {
        if !patch.contains("*** Begin Patch") || !patch.contains("*** End Patch") {
            bail!("patch must use apply_patch format");
        }

        let mut changed = Vec::new();
        let lines: Vec<&str> = patch.lines().collect();
        let mut index = 0;

        while index < lines.len() {
            let line = lines[index];
            if let Some(path) = line.strip_prefix("*** Add File: ") {
                index += 1;
                let mut content = String::new();
                while index < lines.len() && !lines[index].starts_with("*** ") {
                    let add_line = lines[index]
                        .strip_prefix('+')
                        .ok_or_else(|| anyhow::anyhow!("add file lines must start with +"))?;
                    content.push_str(add_line);
                    content.push('\n');
                    index += 1;
                }
                self.write_file(path, &content)?;
                changed.push(format!("added {path}"));
                continue;
            }

            if let Some(path) = line.strip_prefix("*** Delete File: ") {
                let path_buf = self.workspace_path(path)?;
                fs::remove_file(&path_buf)
                    .with_context(|| format!("failed to delete {}", path_buf.display()))?;
                changed.push(format!("deleted {path}"));
                index += 1;
                continue;
            }

            if let Some(path) = line.strip_prefix("*** Update File: ") {
                index += 1;
                let mut old = Vec::new();
                let mut new = Vec::new();
                while index < lines.len() && !lines[index].starts_with("*** ") {
                    let patch_line = lines[index];
                    if patch_line.starts_with("@@") {
                        index += 1;
                        continue;
                    }
                    match patch_line.chars().next() {
                        Some(' ') => {
                            old.push(&patch_line[1..]);
                            new.push(&patch_line[1..]);
                        }
                        Some('-') => old.push(&patch_line[1..]),
                        Some('+') => new.push(&patch_line[1..]),
                        _ => bail!("update lines must start with space, -, +, or @@"),
                    }
                    index += 1;
                }
                self.apply_update(path, &old, &new)?;
                changed.push(format!("updated {path}"));
                continue;
            }

            index += 1;
        }

        if changed.is_empty() {
            bail!("patch did not contain any supported file operations");
        }

        Ok(changed.join("\n"))
    }

    fn apply_update(&self, path: &str, old: &[&str], new: &[&str]) -> Result<()> {
        let path_buf = self.workspace_path(path)?;
        let content = fs::read_to_string(&path_buf)
            .with_context(|| format!("failed to read {}", path_buf.display()))?;
        let old_text = join_patch_lines(old, content.ends_with('\n'));
        let new_text = join_patch_lines(new, content.ends_with('\n'));
        if !content.contains(&old_text) {
            bail!("update hunk did not match {path}");
        }
        let updated = content.replacen(&old_text, &new_text, 1);
        fs::write(&path_buf, updated)
            .with_context(|| format!("failed to write {}", path_buf.display()))?;
        Ok(())
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

    async fn search(&self, query: &str, path: Option<&str>) -> Result<String> {
        if query.trim().is_empty() {
            bail!("query cannot be empty");
        }
        let path = path.unwrap_or(".");
        let checked_path = self.workspace_path(path)?;
        let relative_path = checked_path
            .strip_prefix(&self.root)
            .unwrap_or(&checked_path);
        let output = Command::new("rg")
            .arg("--line-number")
            .arg("--hidden")
            .arg("--glob")
            .arg("!target")
            .arg("--glob")
            .arg("!.git")
            .arg(query)
            .arg(relative_path)
            .current_dir(&self.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("failed to run rg")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(1) {
            return Ok("no matches".to_string());
        }
        if !output.status.success() {
            bail!("search failed\n{stdout}{stderr}");
        }
        Ok(stdout.to_string())
    }

    pub fn workspace_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        if path.is_absolute() {
            bail!("absolute paths are not allowed");
        }
        for component in path.components() {
            if matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            ) {
                bail!("path escapes workspace");
            }
        }
        Ok(self.root.join(path))
    }
}

impl<'de> Deserialize<'de> for ToolCall {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let tool = value
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("tool field is required"))?;

        let string_field = |name: &str| -> Result<String, D::Error> {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| serde::de::Error::custom(format!("{name} field is required")))
        };

        match tool {
            "bash" | "run_command" => Ok(Self::Bash {
                command: string_field("command")?,
            }),
            "read" | "read_file" => Ok(Self::Read {
                path: string_field("path")?,
            }),
            "write" | "write_file" => Ok(Self::Write {
                path: string_field("path")?,
                content: string_field("content")?,
            }),
            "edit" => Ok(Self::Edit {
                path: string_field("path")?,
                old: string_field("old")?,
                new: string_field("new")?,
            }),
            "list" | "list_files" => Ok(Self::List {
                path: string_field("path")?,
            }),
            "search" | "grep" => {
                let path = value
                    .get("path")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                Ok(Self::Search {
                    query: string_field("query")?,
                    path,
                })
            }
            "patch" | "apply_patch" => Ok(Self::Patch {
                patch: string_field("patch")?,
            }),
            unknown => Err(serde::de::Error::custom(format!(
                "unknown tool `{unknown}`"
            ))),
        }
    }
}

fn join_patch_lines(lines: &[&str], trailing_newline: bool) -> String {
    let mut text = lines.join("\n");
    if trailing_newline && !text.is_empty() {
        text.push('\n');
    }
    text
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
            ToolCall::Read {
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

    #[tokio::test]
    async fn supports_core_four_tool_names() {
        let root = tempfile::tempdir().unwrap();
        let runner = ToolRunner::new(root.path().to_path_buf());
        runner
            .run(ToolCall::Write {
                path: "hello.txt".to_string(),
                content: "hello world\n".to_string(),
            })
            .await;
        let edit = runner
            .run(ToolCall::Edit {
                path: "hello.txt".to_string(),
                old: "world".to_string(),
                new: "ollo".to_string(),
            })
            .await;
        let read = runner
            .run(ToolCall::Read {
                path: "hello.txt".to_string(),
            })
            .await;
        let bash = runner
            .run(ToolCall::Bash {
                command: "printf ok".to_string(),
            })
            .await;

        assert!(edit.ok, "{}", edit.output);
        assert_eq!(read.output, "hello ollo\n");
        assert!(bash.ok, "{}", bash.output);
        assert!(bash.output.contains("ok"));
    }

    #[tokio::test]
    async fn read_directory_lists_entries() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("hello.txt"), "hello\n").unwrap();
        let runner = ToolRunner::new(root.path().to_path_buf());
        let result = runner
            .run(ToolCall::Read {
                path: ".".to_string(),
            })
            .await;

        assert!(result.ok, "{}", result.output);
        assert!(result.output.contains("hello.txt"));
    }

    #[test]
    fn parses_search_tool() {
        let call: ToolCall =
            serde_json::from_str(r#"{"tool":"search","query":"fn main","path":"src"}"#).unwrap();
        assert_eq!(
            call,
            ToolCall::Search {
                query: "fn main".to_string(),
                path: Some("src".to_string())
            }
        );
    }

    #[tokio::test]
    async fn applies_add_file_patch_without_external_binary() {
        let root = tempfile::tempdir().unwrap();
        let runner = ToolRunner::new(root.path().to_path_buf());
        let result = runner
            .run(ToolCall::Patch {
                patch: "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch"
                    .to_string(),
            })
            .await;

        assert!(result.ok, "{}", result.output);
        assert_eq!(
            fs::read_to_string(root.path().join("hello.txt")).unwrap(),
            "hello\n"
        );
    }

    #[tokio::test]
    async fn applies_update_patch_without_external_binary() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("hello.txt"), "hello\nworld\n").unwrap();
        let runner = ToolRunner::new(root.path().to_path_buf());
        let result = runner
            .run(ToolCall::Patch {
                patch: "*** Begin Patch\n*** Update File: hello.txt\n@@\n hello\n-world\n+ollo\n*** End Patch"
                    .to_string(),
            })
            .await;

        assert!(result.ok, "{}", result.output);
        assert_eq!(
            fs::read_to_string(root.path().join("hello.txt")).unwrap(),
            "hello\nollo\n"
        );
    }
}

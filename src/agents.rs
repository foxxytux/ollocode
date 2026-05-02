use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
};

const AGENTS_FILE: &str = "AGENTS.md";

pub fn load(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(AGENTS_FILE)).ok()
}

pub fn init(root: &Path) -> Result<PathBuf> {
    let path = root.join(AGENTS_FILE);
    if path.exists() {
        bail!("AGENTS.md already exists");
    }
    fs::write(&path, default_agents_md())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn default_agents_md() -> &'static str {
    r#"# AGENTS.md

## Project

Describe what this project does, the main entry points, and any important constraints.

## Build And Test

- Add the normal build command here.
- Add the normal test command here.

## Coding Guidelines

- Keep changes focused.
- Prefer existing project patterns.
- Run relevant checks before considering work complete.

## Agent Notes

- Mention files or directories that require extra care.
- Mention generated files, vendored code, or paths agents should avoid editing.
"#
}

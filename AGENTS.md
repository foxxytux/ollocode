# AGENTS.md

## Project

ollo-code is a local-first Rust TUI coding agent for Ollama models. It uses Ratatui/Crossterm for the terminal UI and Ollama's local HTTP API for model listing and streaming chat.

## Build And Test

- Format: `cargo fmt`
- Compile: `cargo check`
- Test: `cargo test`
- Run locally: `cargo run`

## Coding Guidelines

- Keep the display name lowercase: `ollo-code`.
- Preserve the chat-first terminal workflow.
- Prefer small, focused commits.
- Keep model-callable core tools available: `bash`, `read`, `write`, and `edit`.
- Run `cargo fmt`, `cargo check`, and `cargo test` before committing.

## Runtime Notes

- Ollama is expected at `http://127.0.0.1:11434` unless `OLLAMA_HOST` is set.
- Conversation state is persisted per workspace in the user config directory.
- `AGENTS.md` content is loaded into the model system prompt.
- Model thinking should be displayed when Ollama emits thinking deltas or `<think>...</think>` blocks.

## GitHub

- Public repository: `https://github.com/foxxytux/ollocode`
- Remote name: `origin`
- Default branch currently used by this repo: `master`
- After completing requested changes, push with `git push`.

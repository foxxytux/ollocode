# Ollo Code

A local-first terminal coding agent for Ollama models.

## Run

Start Ollama, then run:

```sh
cargo run
```

The app reads models from Ollama's local API and lets you switch models in the TUI.

## Keys

- `Enter`: send prompt
- `Ctrl+J` / `Ctrl+K`: select next/previous model
- `Ctrl+M`: refresh models from Ollama
- `Up` / `Down`: browse prompt history
- `Left` / `Right`, `Home` / `End`: edit the prompt
- `PageUp` / `PageDown`: scroll transcript
- `Ctrl+C`: quit

## Mouse

- Click a model in the model pane to select it.
- Use the mouse wheel over the transcript to scroll.
- Use the mouse wheel over the model pane to switch models.
- Click inside the prompt to move the cursor.

## Tool Calls

The model can request local tools by emitting fenced JSON:

```json
{"tool":"read_file","path":"src/main.rs"}
```

Supported tools:

- `list_files`
- `read_file`
- `write_file`
- `apply_patch`
- `run_command`

Tools run from the workspace root. File paths are restricted to the workspace.

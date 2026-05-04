# ollo-code

A local-first terminal coding agent for Ollama models.

## Run

Start Ollama, then run:

```sh
cargo run
```

The app reads models from Ollama's local API and lets you switch models in the TUI.
Conversation context is restored per workspace, and the header shows an approximate context-window percentage for the selected model.
Model output wrapped in `<think>...</think>` is shown in the transcript as `thinking`.
Only models that advertise native Ollama tool support are shown and selectable.

## Install

```sh
./scripts/install.sh
```

That builds the release binary and installs `ollo-code` into `~/.local/bin`.

## Keys

- `Enter`: send prompt
- `Enter` while typing a partial `/` command: autocomplete the selected command
- `Ctrl+J` / `Ctrl+K`: select next/previous model
- `Ctrl+M`: refresh models from Ollama
- `Up` / `Down`: browse prompt history
- `Up` / `Down` while typing `/`: move through command suggestions
- `Left` / `Right`, `Home` / `End`: edit the prompt
- `PageUp` / `PageDown`: scroll transcript
- Paste: bracketed terminal paste inserts text at the cursor.
- `Ctrl+C`: quit

## Commands

- `/help`: show commands.
- `/`: show commands.
- `/init`: create `AGENTS.md` in the workspace.
- `/agents`: reload and show current `AGENTS.md` status.
- `/tools`: show model-callable tools.
- `/model <name>`: switch to an Ollama model by exact name.
- `/models`: list available Ollama models.
- `/bash <command>`: run a local shell command from the workspace.
- `/read <path>`: read a workspace file into the transcript.
- `/clear`: clear the transcript.
- `/context`: show restored context usage.
- `/pwd`: show the current workspace path.

## Mouse

- Click a model in the model pane to select it.
- Use the mouse wheel over the transcript to scroll.
- Use the mouse wheel over the model pane to switch models.
- Click inside the prompt to move the cursor.

Command suggestions appear only while typing `/` commands.

## Tool Calls

Ollama tool-capable models can call local tools through the native `/api/chat` `tools` field.
Supported tools:

- `bash`
- `read`
- `write`
- `edit`
- `list`
- `search`
- `patch`

The app passes those tools directly to Ollama and only keeps models that report tool support in `/api/show`.
Tool results are fed back to the model as native `tool` messages.

Tools run from the workspace root. File paths are restricted to the workspace.
Use one tool call at a time.

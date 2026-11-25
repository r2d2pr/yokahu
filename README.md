<p align="center">
  <img src="assets/logo.jpg" alt="Yokahu Logo" width="260" />
</p>

<h1 align="center">Yokahu</h1>

<p align="center">
  Fast, native Rust GPU desktop workspace for local &amp; remote AI models with workspace agent tools, customizable UI themes, and user-configurable MCP servers.
</p>

---

## Features

- **Native Rust GPU UI (`yokahu-ui`)**: Built with `eframe` / `egui` + `wgpu`. Zero Electron/browser overhead, instant startup, offline-first.
- **Use Any Model or Provider**:
  - **Ollama**: Auto-discovers installed models + built-in **Pull Model** directly from the UI (`qwen3.5:9b`, `llama3.2`, `deepseek-r1`, etc.).
  - **OpenAI-Compatible (`http://` & `https://`)**: Connect to LM Studio, `llama-server`, vLLM, or remote APIs via pure-Rust TLS (`ureq` + `rustls`), with full streaming `tool_calls` support.
- **Side-by-Side Model Compare & Chat Forking**:
  - Enable **Compare 2 models side-by-side** in `Settings` to stream two models simultaneously.
  - **Fork** any assistant message into a branched conversation, or **Edit** / **Regenerate** any turn.
- **Workspace Agent, Diffs & `.yokahu.md` Memory**:
  - Select a local folder in `Files` for `list_dir`, `read_file`, `search_files`, `write_file`, and `run_command` tools, gated by `Safe (Ask Writes)`, `Read-Only`, or `Auto-Approve`.
  - Colored unified diffs (`-` / `+`) on `write_file` approvals, editable file preview (`.yokahu.md` auto-loaded into system prompt), and **Apply to File...** on syntax-highlighted code blocks.
- **Custom MCP Servers (Stdio & Streamable HTTP/S)**:
  - Persistent stdio MCP process pool + Streamable HTTP/HTTPS support via **MCP Servers** or `%LOCALAPPDATA%\Yokahu\mcp_config.json`.
  - In-app **New MCP Server** generator (scaffolds a Python or Rust MCP server and auto-registers it) + starter templates in `examples/custom-mcp/`.
  - Includes `yokahu-mcp` (`qwen_ask`, `qwen_analyze_file`, `qwen_batch`, `qwen_status`).
- **Customizable Appearance & Power UX**:
  - 5 curated themes (`Obsidian Crimson`, `Midnight Blue`, `Emerald Slate`, `Apple Light`, `Pure Black`) + live RGB color pickers.
  - Saved **Personas**, `Ctrl+K` Command Palette, `Ctrl+V` clipboard image paste, `@filename` autocomplete, pinned/renamed chats, Read Aloud TTS, and live Mic / Whisper CLI transcription.

---

## Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Ctrl+K` | Open Command Palette (jump to chats, models, personas, themes) |
| `Ctrl+N` | New Conversation |
| `Ctrl+B` | Toggle Sidebar |
| `Ctrl+V` | Paste image from clipboard |
| `Esc` | Stop generation or close modal |
| `Enter` / `Shift+Enter` | Send message / Newline |

---

## Quick Start

### Prerequisites
1. [Rust](https://rustup.rs/) (`cargo`) — or download prebuilt archives from [Releases](https://github.com/r2d2pr/yokahu/releases)
2. A local model runner such as [Ollama](https://ollama.com/) (or LM Studio / `llama.cpp`)

### Build & Run
```bash
git clone https://github.com/r2d2pr/yokahu.git
cd yokahu
cargo test
cargo build --release
./target/release/yokahu-ui
```

### License
MIT

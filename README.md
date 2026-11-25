<p align="center">
  <img src="assets/logo.jpg" alt="Yokahu Logo" width="260" />
</p>

<h1 align="center">Yokahu</h1>

<p align="center">
  Fast, native Rust GPU desktop workspace for local &amp; custom AI models with workspace agent tools and user-configurable MCP servers.
</p>

---

## Features

- **Native Rust GPU UI (`yokahu-ui`)**: Built with `eframe` / `egui` + `wgpu`. Zero Electron/browser overhead, instant startup, offline-first.
- **Use Any Model or Provider**:
  - **Ollama** (auto-discovers installed models + built-in **Pull Model** directly from the UI, e.g. `qwen3.5:9b`, `llama3.2`, `deepseek-r1`).
  - **OpenAI-Compatible** endpoints (LM Studio, `llama-server`, vLLM, or remote APIs with custom Base URL + optional API Key).
- **Custom MCP Servers**:
  - Add any stdio MCP server directly in the UI (**MCP Servers**) or via `%LOCALAPPDATA%\Yokahu\mcp_config.json`.
  - Automatically discovers tools (`initialize` + `tools/list`) and lets your chosen model call them.
- **Workspace Agent & Safety Modes**:
  - Pick any local folder so the model can `list_dir`, `read_file`, `search_files`, `write_file`, and `run_command`.
  - Switch between `Safe (Ask Writes)`, `Read-Only`, and `Auto-Approve`.
- **Bundled `yokahu-mcp` Server**:
  - Included zero-dependency Rust stdio MCP server (`qwen_ask`, `qwen_analyze_file`, `qwen_batch`, `qwen_status`) for delegating tasks to any local Ollama model (`YOKAHU_MODEL`).
- **In-App MCP Generator & Starters**:
  - Scaffold a new Python or Rust MCP server directly from **MCP Servers** in Yokahu, or copy from `examples/custom-mcp/`.

---

## Quick Start

### Prerequisites
1. [Rust](https://rustup.rs/) (`cargo`)
2. A local model runner such as [Ollama](https://ollama.com/) (or LM Studio / `llama.cpp`)

### Build & Run
```bash
git clone https://github.com/r2d2pr/yokahu.git
cd yokahu
cargo build --release
```

Run the UI:
```bash
./target/release/yokahu-ui
```

### Adding Your Own MCP Servers
Open **MCP Servers** in Yokahu's sidebar (or edit `%LOCALAPPDATA%\Yokahu\mcp_config.json`):

```json
{
  "mcpServers": {
    "my-local-worker": {
      "command": "C:\\path\\to\\qwen-mcp.exe",
      "args": [],
      "env": {
        "OLLAMA_HOST": "http://127.0.0.1:11434",
        "QWEN_SLAVE_MODEL": "qwen3.5:9b"
      },
      "enabled": true
    }
  }
}
```

Any stdio MCP server implementing JSON-RPC 2.0 (`initialize`, `tools/list`, `tools/call`) works out of the box.

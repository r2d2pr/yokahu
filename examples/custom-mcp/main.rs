//! Minimal zero-dependency Rust stdio MCP server template (`rustc main.rs`).
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines().flatten() {
        let t = line.trim();
        if t.is_empty() || t.contains("\"notifications/initialized\"") {
            continue;
        }
        // Replace with serde_json in a full Cargo project.
        if t.contains("\"initialize\"") {
            let _ = writeln!(out, r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2024-11-05","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"custom-rust-mcp","version":"0.1.0"}}}}}}"#);
        } else if t.contains("\"tools/list\"") {
            let _ = writeln!(out, r#"{{"jsonrpc":"2.0","id":2,"result":{{"tools":[{{"name":"hello","description":"Example Rust MCP tool","inputSchema":{{"type":"object","properties":{{"name":{{"type":"string"}}}},"required":["name"]}}}}]}}}}"#);
        } else if t.contains("\"tools/call\"") {
            let _ = writeln!(out, r#"{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":"Hello from custom Rust MCP server!"}}]}}}}"#);
        }
        let _ = out.flush();
    }
}

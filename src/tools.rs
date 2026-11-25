use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyMode {
    Safe,
    ReadOnly,
    AutoApprove,
}

impl Default for SafetyMode {
    fn default() -> Self {
        SafetyMode::Safe
    }
}

impl SafetyMode {
    pub fn label(self) -> &'static str {
        match self {
            SafetyMode::Safe => "Safe (Ask Writes)",
            SafetyMode::ReadOnly => "Read-Only",
            SafetyMode::AutoApprove => "Auto-Approve",
        }
    }
}

pub fn ollama_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and subdirectories in a directory inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative or absolute directory path (defaults to workspace root)." }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a text file with line numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to read." },
                        "start_line": { "type": "integer", "description": "1-based start line (default 1)." },
                        "end_line": { "type": "integer", "description": "1-based end line (default 250)." }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_files",
                "description": "Case-insensitive text search across files in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text to search for." },
                        "path": { "type": "string", "description": "Optional subdirectory to search." }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a file in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Target file path." },
                        "content": { "type": "string", "description": "Full file contents." }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a PowerShell command in the workspace directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "PowerShell command to execute." }
                    },
                    "required": ["command"]
                }
            }
        }
    ])
}

pub fn is_mutating_tool(name: &str) -> bool {
    matches!(name, "write_file" | "run_command")
}

fn resolve_path(workspace: Option<&Path>, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else if let Some(ws) = workspace {
        ws.join(p)
    } else {
        p
    }
}

fn is_within_workspace(workspace: Option<&Path>, target: &Path) -> bool {
    let Some(ws) = workspace else { return true };
    let Ok(ws_canon) = fs::canonicalize(ws) else { return true };
    if let Ok(t_canon) = fs::canonicalize(target) {
        return t_canon.starts_with(&ws_canon);
    }
    // For new files, check parent
    if let Some(parent) = target.parent() {
        if let Ok(p_canon) = fs::canonicalize(parent) {
            return p_canon.starts_with(&ws_canon);
        }
    }
    target.starts_with(ws)
}

pub fn preview_tool(name: &str, args: &Value) -> String {
    match name {
        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let preview: String = content.lines().take(20).collect::<Vec<_>>().join("\n");
            let total_lines = content.lines().count();
            format!("Write file: {}\n({} lines)\n---\n{}", path, total_lines, preview)
        }
        "run_command" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format!("Run PowerShell:\n{}", cmd)
        }
        _ => format!("{}({})", name, args),
    }
}

pub fn execute_tool(workspace: Option<&Path>, name: &str, args: &Value) -> String {
    match name {
        "list_dir" => {
            let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let dir = resolve_path(workspace, raw);
            let Ok(entries) = fs::read_dir(&dir) else {
                return format!("Error: cannot read directory {}", dir.display());
            };
            let mut items = Vec::new();
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if matches!(fname.as_str(), ".git" | "target" | "node_modules") {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                items.push(if is_dir { format!("{}/", fname) } else { fname });
                if items.len() >= 200 {
                    break;
                }
            }
            items.sort();
            items.join("\n")
        }
        "read_file" => {
            let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = resolve_path(workspace, raw);
            let Ok(text) = fs::read_to_string(&path) else {
                return format!("Error: cannot read file {}", path.display());
            };
            let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
            let end = args.get("end_line").and_then(|v| v.as_u64()).unwrap_or(250).max(1) as usize;
            let lines: Vec<&str> = text.lines().collect();
            let s = (start - 1).min(lines.len());
            let e = end.min(lines.len());
            let mut out = String::new();
            for (idx, line) in lines[s..e].iter().enumerate() {
                out.push_str(&format!("{}: {}\n", s + idx + 1, line));
            }
            if out.is_empty() { "(empty)".to_string() } else { out }
        }
        "search_files" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            if query.is_empty() {
                return "Error: empty query".to_string();
            }
            let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let root = resolve_path(workspace, raw);
            let mut matches = Vec::new();
            search_walk(&root, &query, &mut matches);
            if matches.is_empty() {
                "No matches found.".to_string()
            } else {
                matches.join("\n")
            }
        }
        "write_file" => {
            let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let path = resolve_path(workspace, raw);
            if !is_within_workspace(workspace, &path) {
                return format!("Blocked: {} is outside workspace.", path.display());
            }
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::write(&path, content) {
                Ok(_) => format!("Wrote {} bytes to {}", content.len(), path.display()),
                Err(e) => format!("Error writing {}: {}", path.display(), e),
            }
        }
        "run_command" => {
            let cmd_str = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", cmd_str]);
            if let Some(ws) = workspace {
                cmd.current_dir(ws);
            }
            match cmd.output() {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let code = out.status.code().unwrap_or(-1);
                    format!("Exit: {}\n{}{}", code, stdout, stderr)
                }
                Err(e) => format!("Failed to run command: {}", e),
            }
        }
        other => format!("Unknown tool: {}", other),
    }
}

fn search_walk(dir: &Path, query: &str, matches: &mut Vec<String>) {
    if matches.len() >= 50 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if matches.len() >= 50 {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        if path.is_dir() {
            search_walk(&path, query, matches);
        } else if let Ok(text) = fs::read_to_string(&path) {
            for (i, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(query) {
                    matches.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                    if matches.len() >= 50 {
                        break;
                    }
                }
            }
        }
    }
}

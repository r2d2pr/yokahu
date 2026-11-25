use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

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
                "name": "edit_file",
                "description": "Surgically replace an exact target string in an existing workspace file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Target file path." },
                        "target": { "type": "string", "description": "Exact existing text to replace." },
                        "replacement": { "type": "string", "description": "Replacement text." }
                    },
                    "required": ["path", "target", "replacement"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_files",
                "description": "Find files in the workspace matching a filename substring or extension.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Case-insensitive filename or extension substring (e.g. '.rs' or 'cargo')." },
                        "path": { "type": "string", "description": "Optional subdirectory." }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Command to execute." }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "spawn_background",
                "description": "Start a long-running command in the background and return its task ID.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Command to run in background." }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "manage_tasks",
                "description": "List, inspect logs/status, or kill background tasks.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["list", "status", "kill"] },
                        "id": { "type": "integer", "description": "Task ID for status/kill." }
                    },
                    "required": ["action"]
                }
            }
        }
    ])
}

pub fn agent_ui_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "update_steps",
                "description": "Update the live task checklist shown to the user.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "steps": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string" },
                                    "status": { "type": "string", "enum": ["pending", "in_progress", "done", "failed"] }
                                },
                                "required": ["title", "status"]
                            }
                        }
                    },
                    "required": ["steps"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "update_focus",
                "description": "Set a short 3-8 word status banner describing what you are currently doing.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "focus": { "type": "string" }
                    },
                    "required": ["focus"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "ask_question",
                "description": "Ask the user a multiple-choice question in the chat UI and wait for their answer.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "options": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["question", "options"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "invoke_subagent",
                "description": "Delegate an isolated read/research/drafting subtask to a worker subagent and return its summary.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "role": { "type": "string", "description": "Short role title e.g. 'Codebase Researcher'." },
                        "prompt": { "type": "string", "description": "Clear task and what to return." }
                    },
                    "required": ["role", "prompt"]
                }
            }
        }
    ])
}

pub fn is_mutating_tool(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "run_command" | "spawn_background")
}

pub fn is_web_tool(name: &str) -> bool {
    matches!(name, "web_search" | "fetch_url")
}

pub fn web_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web via DuckDuckGo (requires internet).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query." }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "fetch_url",
                "description": "Fetch a web page and extract readable text (up to 16 KB).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "HTTP or HTTPS URL." }
                    },
                    "required": ["url"]
                }
            }
        }
    ])
}

pub fn execute_web_tool(name: &str, args: &Value) -> String {
    match name {
        "web_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
            if query.is_empty() {
                return "Error: empty query".to_string();
            }
            let encoded: String = query
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
                    b' ' => "+".to_string(),
                    _ => format!("%{:02X}", b),
                })
                .collect();
            let url = format!("https://html.duckduckgo.com/html/?q={}", encoded);
            match ureq::get(&url).header("User-Agent", "Mozilla/5.0").call() {
                Ok(mut resp) => {
                    let html = resp.body_mut().read_to_string().unwrap_or_default();
                    let text = strip_html_tags(&html);
                    text.chars().take(8000).collect()
                }
                Err(e) => format!("Web search error: {}", e),
            }
        }
        "fetch_url" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return "Error: URL must start with http:// or https://".to_string();
            }
            match ureq::get(url).header("User-Agent", "Yokahu/0.6").call() {
                Ok(mut resp) => {
                    let raw = resp.body_mut().read_to_string().unwrap_or_default();
                    let text = strip_html_tags(&raw);
                    text.chars().take(16000).collect()
                }
                Err(e) => format!("Fetch error: {}", e),
            }
        }
        other => format!("Unknown web tool: {}", other),
    }
}

pub fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 3);
    let mut in_tag = false;
    let mut last_space = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            _ if in_tag => {}
            c if c.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            c => {
                out.push(c);
                last_space = false;
            }
        }
    }
    out.trim().to_string()
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
            format!("Write file: {} ({} lines)", path, content.lines().count())
        }
        "edit_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("Edit file: {}", path)
        }
        "run_command" | "spawn_background" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format!("{}: {}", name, cmd)
        }
        _ => format!("{}({})", name, args),
    }
}

pub fn compute_edit_diff(workspace: Option<&Path>, args: &Value) -> Vec<(char, String)> {
    let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let replacement = args.get("replacement").and_then(|v| v.as_str()).unwrap_or("");
    let path = resolve_path(workspace, raw);
    let old_content = fs::read_to_string(&path).unwrap_or_default();
    if target.is_empty() || !old_content.contains(target) {
        return vec![('-', format!("Target not found in {}", raw))];
    }
    let new_content = old_content.replace(target, replacement);
    compute_diff_lines(&old_content, &new_content)
}

pub fn compute_write_diff(workspace: Option<&Path>, args: &Value) -> Vec<(char, String)> {
    let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let new_content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let path = resolve_path(workspace, raw);
    let old_content = fs::read_to_string(&path).unwrap_or_default();
    compute_diff_lines(&old_content, new_content)
}

fn compute_diff_lines(old_content: &str, new_content: &str) -> Vec<(char, String)> {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();
    let mut diff = Vec::new();

    if old_lines.is_empty() {
        for l in new_lines.iter().take(40) {
            diff.push(('+', (*l).to_string()));
        }
        return diff;
    }

    let mut pref = 0;
    while pref < old_lines.len() && pref < new_lines.len() && old_lines[pref] == new_lines[pref] {
        pref += 1;
    }
    let mut suf = 0;
    while suf < (old_lines.len() - pref)
        && suf < (new_lines.len() - pref)
        && old_lines[old_lines.len() - 1 - suf] == new_lines[new_lines.len() - 1 - suf]
    {
        suf += 1;
    }

    let ctx_start = pref.saturating_sub(2);
    for l in &old_lines[ctx_start..pref] {
        diff.push((' ', (*l).to_string()));
    }
    for l in &old_lines[pref..old_lines.len() - suf] {
        diff.push(('-', (*l).to_string()));
        if diff.len() >= 50 {
            return diff;
        }
    }
    for l in &new_lines[pref..new_lines.len() - suf] {
        diff.push(('+', (*l).to_string()));
        if diff.len() >= 50 {
            return diff;
        }
    }
    let ctx_end = (old_lines.len() - suf + 2).min(old_lines.len());
    for l in &old_lines[old_lines.len() - suf..ctx_end] {
        diff.push((' ', (*l).to_string()));
    }
    diff
}

#[derive(Clone, Debug)]
pub struct WorkspaceEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub depth: usize,
}

pub fn list_workspace_tree(root: &Path) -> Vec<WorkspaceEntry> {
    let mut out = Vec::new();
    walk_tree(root, 0, &mut out);
    out
}

fn walk_tree(dir: &Path, depth: usize, out: &mut Vec<WorkspaceEntry>) {
    if depth > 4 || out.len() >= 300 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| {
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        (!is_dir, e.file_name().to_string_lossy().to_lowercase())
    });

    for entry in entries {
        if out.len() >= 300 {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(WorkspaceEntry {
            name,
            path: path.clone(),
            is_dir,
            depth,
        });
        if is_dir {
            walk_tree(&path, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_boundary_blocks_escape() {
        let ws = std::env::temp_dir().join("yokahu_test_ws");
        let _ = fs::create_dir_all(&ws);
        let res = execute_tool(
            Some(&ws),
            "write_file",
            &json!({"path": "../outside.txt", "content": "nope"}),
        );
        assert!(res.contains("Blocked"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn diff_detects_changes() {
        let ws = std::env::temp_dir().join("yokahu_test_diff");
        let _ = fs::create_dir_all(&ws);
        fs::write(ws.join("a.txt"), "line1\nold\nline3\n").unwrap();
        let diff = compute_write_diff(
            Some(&ws),
            &json!({"path": "a.txt", "content": "line1\nnew\nline3\n"}),
        );
        assert!(diff.iter().any(|(k, l)| *k == '-' && l == "old"));
        assert!(diff.iter().any(|(k, l)| *k == '+' && l == "new"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html_tags("<h1>Hello</h1> <p>World</p>"), "Hello World");
    }

    #[test]
    fn edit_file_and_find_files_work() {
        let ws = std::env::temp_dir().join("yokahu_test_edit");
        let _ = fs::create_dir_all(&ws);
        fs::write(ws.join("sample.rs"), "fn old_name() {}\n").unwrap();
        let res = execute_tool(
            Some(&ws),
            "edit_file",
            &json!({"path": "sample.rs", "target": "old_name", "replacement": "new_name"}),
        );
        assert!(res.contains("Replaced 1"));
        let found = execute_tool(Some(&ws), "find_files", &json!({"pattern": ".rs"}));
        assert!(found.contains("sample.rs"));
        let _ = fs::remove_dir_all(&ws);
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
        "edit_file" => {
            let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
            let replacement = args.get("replacement").and_then(|v| v.as_str()).unwrap_or("");
            let path = resolve_path(workspace, raw);
            if !is_within_workspace(workspace, &path) {
                return format!("Blocked: {} is outside workspace.", path.display());
            }
            let Ok(old) = fs::read_to_string(&path) else {
                return format!("Error: cannot read {}", path.display());
            };
            if target.is_empty() || !old.contains(target) {
                return format!("Error: target text not found in {}", path.display());
            }
            let count = old.matches(target).count();
            let updated = old.replace(target, replacement);
            match fs::write(&path, &updated) {
                Ok(_) => format!("Replaced {} occurrence(s) in {}", count, path.display()),
                Err(e) => format!("Error writing {}: {}", path.display(), e),
            }
        }
        "find_files" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let raw = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let root = resolve_path(workspace, raw);
            let mut found = Vec::new();
            find_walk(&root, &pattern, &mut found);
            if found.is_empty() {
                "No files matched.".to_string()
            } else {
                found.join("\n")
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

fn find_walk(dir: &Path, pattern: &str, out: &mut Vec<String>) {
    if out.len() >= 100 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= 100 {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        if path.is_dir() {
            find_walk(&path, pattern, out);
        } else if pattern.is_empty() || name.to_lowercase().contains(pattern) {
            out.push(path.display().to_string());
        }
    }
}

pub struct BgTask {
    pub command: String,
    pub child: Child,
    pub logs: Arc<Mutex<Vec<String>>>,
}

#[derive(Default)]
pub struct BackgroundTasks {
    next_id: Mutex<u64>,
    tasks: Mutex<HashMap<u64, BgTask>>,
}

impl BackgroundTasks {
    pub fn spawn(&self, workspace: Option<&Path>, command: &str) -> String {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-Command", command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", command]);
            c
        };
        if let Some(ws) = workspace {
            cmd.current_dir(ws);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let Ok(mut child) = cmd.spawn() else {
            return format!("Failed to spawn: {}", command);
        };

        let logs = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.stdout.take() {
            let l = logs.clone();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().flatten() {
                    if let Ok(mut g) = l.lock() {
                        g.push(line);
                        if g.len() > 200 {
                            let drop_n = g.len() - 200;
                            g.drain(..drop_n);
                        }
                    }
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let l = logs.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().flatten() {
                    if let Ok(mut g) = l.lock() {
                        g.push(format!("[err] {}", line));
                        if g.len() > 200 {
                            let drop_n = g.len() - 200;
                            g.drain(..drop_n);
                        }
                    }
                }
            });
        }

        let mut id_g = self.next_id.lock().unwrap();
        *id_g += 1;
        let id = *id_g;
        self.tasks.lock().unwrap().insert(
            id,
            BgTask {
                command: command.to_string(),
                child,
                logs,
            },
        );
        format!("Started background task #{} ({})", id, command)
    }

    pub fn manage(&self, action: &str, id: Option<u64>) -> String {
        let mut map = self.tasks.lock().unwrap();
        match action {
            "list" => {
                if map.is_empty() {
                    return "No background tasks.".to_string();
                }
                let mut lines = Vec::new();
                for (tid, t) in map.iter_mut() {
                    let st = match t.child.try_wait() {
                        Ok(Some(code)) => format!("exited ({})", code),
                        _ => "running".to_string(),
                    };
                    lines.push(format!("#{} [{}] {}", tid, st, t.command));
                }
                lines.join("\n")
            }
            "status" => {
                let Some(tid) = id else { return "Missing task id".to_string() };
                let Some(t) = map.get_mut(&tid) else {
                    return format!("Task #{} not found", tid);
                };
                let st = match t.child.try_wait() {
                    Ok(Some(code)) => format!("exited ({})", code),
                    _ => "running".to_string(),
                };
                let log_tail = t.logs.lock().unwrap().iter().rev().take(30).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                format!("Task #{} [{}]\n{}", tid, st, log_tail)
            }
            "kill" => {
                let Some(tid) = id else { return "Missing task id".to_string() };
                if let Some(mut t) = map.remove(&tid) {
                    let _ = t.child.kill();
                    let _ = t.child.wait();
                    format!("Killed task #{}", tid)
                } else {
                    format!("Task #{} not found", tid)
                }
            }
            other => format!("Unknown action: {}", other),
        }
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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct McpConfigFile {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Clone, Debug)]
pub struct DiscoveredMcpTool {
    pub server_name: String,
    pub tool_name: String,
    pub full_name: String, // mcp__<server>__<tool>
    pub description: String,
    pub input_schema: Value,
}

pub fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        return base.join("Yokahu");
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        return home.join("Library").join("Application Support").join("Yokahu");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("yokahu");
            }
        }
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        home.join(".local").join("share").join("yokahu")
    }
}

pub fn config_path() -> PathBuf {
    app_data_dir().join("mcp_config.json")
}

pub fn load_config() -> McpConfigFile {
    let p = config_path();
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &McpConfigFile) {
    let p = config_path();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = fs::write(p, text);
    }
}

fn spawn_and_exchange(cfg: &McpServerConfig, requests: &[Value]) -> Result<Vec<Value>, String> {
    let mut cmd = Command::new(&cfg.command);
    cmd.args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {}", cfg.command, e))?;

    {
        let stdin = child.stdin.as_mut().ok_or("Missing stdin")?;
        for req in requests {
            let line = req.to_string();
            writeln!(stdin, "{}", line).map_err(|e| format!("Write error: {}", e))?;
        }
        stdin.flush().map_err(|e| format!("Flush error: {}", e))?;
    }
    drop(child.stdin.take());

    let stdout = child.stdout.take().ok_or("Missing stdout")?;
    let reader = BufReader::new(stdout);
    let mut responses = Vec::new();
    for line in reader.lines().flatten() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            responses.push(v);
        }
    }
    let _ = child.wait();
    Ok(responses)
}

fn http_post_json(url: &str, body: &Value) -> Result<Value, String> {
    let trimmed = url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .ok_or_else(|| "Streamable HTTP currently supports http:// local/LAN URLs".to_string())?;
    let (host_port, path) = match without_scheme.split_once('/') {
        Some((hp, p)) => (hp, format!("/{}", p)),
        None => (without_scheme, "/".to_string()),
    };
    let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(80))
    } else {
        (host_port.to_string(), 80)
    };
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e| format!("Invalid MCP URL {}: {}", url, e))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("MCP HTTP connect failed: {}", e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));

    let payload = body.to_string();
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        host,
        port,
        payload.len(),
        payload
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("Write error: {}", e))?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw).map_err(|e| format!("Read error: {}", e))?;
    let body_part = raw.split("\r\n\r\n").nth(1).unwrap_or(&raw);
    for line in body_part.lines() {
        let t = line.trim();
        let clean = t.strip_prefix("data:").map(|s| s.trim()).unwrap_or(t);
        if clean.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(clean) {
                return Ok(v);
            }
        }
    }
    Err(format!("No JSON-RPC response from {}", url))
}

fn exchange_mcp(cfg: &McpServerConfig, requests: &[Value]) -> Result<Vec<Value>, String> {
    if !cfg.url.trim().is_empty() {
        let mut resps = Vec::new();
        for req in requests {
            if req.get("id").is_none() {
                continue;
            }
            resps.push(http_post_json(&cfg.url, req)?);
        }
        Ok(resps)
    } else {
        spawn_and_exchange(cfg, requests)
    }
}

pub fn discover_tools(cfg: &McpConfigFile) -> (Vec<DiscoveredMcpTool>, Vec<String>) {
    let mut tools = Vec::new();
    let mut errors = Vec::new();

    for (server_name, scfg) in &cfg.mcp_servers {
        if !scfg.enabled {
            continue;
        }
        let reqs = vec![
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "yokahu", "version": "0.2.0" }
                }
            }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        ];

        match exchange_mcp(scfg, &reqs) {
            Ok(resps) => {
                if let Some(list_resp) = resps.into_iter().find(|r| r.get("id") == Some(&json!(2))) {
                    if let Some(arr) = list_resp
                        .get("result")
                        .and_then(|r| r.get("tools"))
                        .and_then(|t| t.as_array())
                    {
                        for t in arr {
                            let tname = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if tname.is_empty() {
                                continue;
                            }
                            let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
                            let schema = t
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                            tools.push(DiscoveredMcpTool {
                                server_name: server_name.clone(),
                                tool_name: tname.to_string(),
                                full_name: format!("mcp__{}__{}", server_name, tname),
                                description: format!("[MCP:{}] {}", server_name, desc),
                                input_schema: schema,
                            });
                        }
                    }
                }
            }
            Err(e) => errors.push(format!("{}: {}", server_name, e)),
        }
    }

    (tools, errors)
}

pub fn to_tool_schemas(tools: &[DiscoveredMcpTool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.full_name,
                    "description": t.description,
                    "parameters": t.input_schema
                }
            })
        })
        .collect()
}

struct PersistentStdio {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl PersistentStdio {
    fn spawn(cfg: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn '{}': {}", cfg.command, e))?;
        let stdin = child.stdin.take().ok_or("Missing stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("Missing stdout")?);
        let mut proc = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        proc.initialize()?;
        Ok(proc)
    }

    fn initialize(&mut self) -> Result<(), String> {
        let id = self.next_id;
        self.next_id += 1;
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "yokahu", "version": "0.4.0" }
            }
        });
        let _ = self.rpc(init_req, id)?;
        let notif = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        writeln!(self.stdin, "{}", notif).map_err(|e| format!("Write error: {}", e))?;
        self.stdin.flush().map_err(|e| format!("Flush error: {}", e))?;
        Ok(())
    }

    fn rpc(&mut self, req: Value, expected_id: u64) -> Result<Value, String> {
        writeln!(self.stdin, "{}", req).map_err(|e| format!("Write error: {}", e))?;
        self.stdin.flush().map_err(|e| format!("Flush error: {}", e))?;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).map_err(|e| format!("Read error: {}", e))?;
            if n == 0 {
                return Err("MCP process closed stdout".to_string());
            }
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(t) {
                if v.get("id").and_then(|x| x.as_u64()) == Some(expected_id) {
                    return Ok(v);
                }
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.rpc(req, id)
    }
}

impl Drop for PersistentStdio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct McpPool {
    procs: Mutex<HashMap<String, PersistentStdio>>,
}

impl Default for McpPool {
    fn default() -> Self {
        Self {
            procs: Mutex::new(HashMap::new()),
        }
    }
}

impl McpPool {
    pub fn restart_all(&self) {
        if let Ok(mut map) = self.procs.lock() {
            map.clear();
        }
    }

    pub fn call_tool(&self, cfg: &McpConfigFile, full_name: &str, args: &Value) -> Result<String, String> {
        let rest = full_name
            .strip_prefix("mcp__")
            .ok_or_else(|| format!("Invalid MCP tool name: {}", full_name))?;
        let (server_name, tool_name) = rest
            .split_once("__")
            .ok_or_else(|| format!("Invalid MCP tool name: {}", full_name))?;
        let scfg = cfg
            .mcp_servers
            .get(server_name)
            .ok_or_else(|| format!("MCP server '{}' not found", server_name))?;
        if !scfg.enabled {
            return Err(format!("MCP server '{}' is disabled", server_name));
        }

        if !scfg.url.trim().is_empty() {
            return call_mcp_tool(cfg, full_name, args);
        }

        let mut map = self.procs.lock().map_err(|_| "MCP pool lock poisoned".to_string())?;
        if !map.contains_key(server_name) {
            let proc = PersistentStdio::spawn(scfg)?;
            map.insert(server_name.to_string(), proc);
        }

        let params = json!({ "name": tool_name, "arguments": args });
        let resp = match map.get_mut(server_name).unwrap().call("tools/call", params.clone()) {
            Ok(r) => r,
            Err(_) => {
                // Respawn once if persistent child exited
                let mut proc = PersistentStdio::spawn(scfg)?;
                let r = proc.call("tools/call", params)?;
                map.insert(server_name.to_string(), proc);
                r
            }
        };

        if let Some(err) = resp.get("error") {
            return Err(format!("MCP error: {}", err));
        }
        let content = resp
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| resp.to_string());
        Ok(content)
    }
}

pub fn call_mcp_tool(cfg: &McpConfigFile, full_name: &str, args: &Value) -> Result<String, String> {
    let rest = full_name
        .strip_prefix("mcp__")
        .ok_or_else(|| format!("Invalid MCP tool name: {}", full_name))?;
    let (server_name, tool_name) = rest
        .split_once("__")
        .ok_or_else(|| format!("Invalid MCP tool name: {}", full_name))?;

    let scfg = cfg
        .mcp_servers
        .get(server_name)
        .ok_or_else(|| format!("MCP server '{}' not found", server_name))?;
    if !scfg.enabled {
        return Err(format!("MCP server '{}' is disabled", server_name));
    }

    let reqs = vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "yokahu", "version": "0.2.0" }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": args
            }
        }),
    ];

    let resps = exchange_mcp(scfg, &reqs)?;
    let call_resp = resps
        .into_iter()
        .find(|r| r.get("id") == Some(&json!(2)))
        .ok_or_else(|| format!("No response from MCP server '{}'", server_name))?;

    if let Some(err) = call_resp.get("error") {
        return Err(format!("MCP error: {}", err));
    }

    let content = call_resp
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| call_resp.to_string());

    Ok(content)
}

pub fn scaffold_mcp_server(lang: &str, name: &str, dir: &Path) -> Result<McpServerConfig, String> {
    let target_dir = dir.join(name);
    fs::create_dir_all(&target_dir).map_err(|e| format!("Cannot create dir: {}", e))?;

    if lang == "python" {
        let script_path = target_dir.join("server.py");
        let code = format!(
            "\"\"\"Custom MCP server '{}' for Yokahu.\"\"\"\nimport json\nimport sys\n\nTOOLS = [{{\n    \"name\": \"hello\",\n    \"description\": \"Example tool from {} MCP server.\",\n    \"inputSchema\": {{\"type\": \"object\", \"properties\": {{\"name\": {{\"type\": \"string\"}}}}, \"required\": [\"name\"]}}\n}}]\n\ndef handle_tool(tool_name, args):\n    if tool_name == \"hello\":\n        return {{\"content\": [{{\"type\": \"text\", \"text\": f\"Hello {{args.get('name', 'world')}} from {}!\"}}]}}\n    return {{\"content\": [{{\"type\": \"text\", \"text\": f\"Unknown tool: {{tool_name}}\"}}], \"isError\": True}}\n\nfor line in sys.stdin:\n    line = line.strip()\n    if not line:\n        continue\n    req = json.loads(line)\n    method = req.get(\"method\", \"\")\n    if method == \"notifications/initialized\" or \"id\" not in req:\n        continue\n    req_id = req[\"id\"]\n    params = req.get(\"params\") or {{}}\n    if method == \"initialize\":\n        res = {{\"protocolVersion\": params.get(\"protocolVersion\", \"2024-11-05\"), \"capabilities\": {{\"tools\": {{}}}}, \"serverInfo\": {{\"name\": \"{}\", \"version\": \"0.1.0\"}}}}\n    elif method == \"ping\":\n        res = {{}}\n    elif method == \"tools/list\":\n        res = {{\"tools\": TOOLS}}\n    elif method == \"tools/call\":\n        res = handle_tool(params.get(\"name\", \"\"), params.get(\"arguments\") or {{}})\n    else:\n        sys.stdout.write(json.dumps({{\"jsonrpc\": \"2.0\", \"id\": req_id, \"error\": {{\"code\": -32601, \"message\": \"Method not found\"}}}}) + \"\\n\")\n        sys.stdout.flush()\n        continue\n    sys.stdout.write(json.dumps({{\"jsonrpc\": \"2.0\", \"id\": req_id, \"result\": res}}) + \"\\n\")\n    sys.stdout.flush()\n",
            name, name, name, name
        );
        fs::write(&script_path, code).map_err(|e| format!("Write failed: {}", e))?;
        Ok(McpServerConfig {
            command: "python".to_string(),
            args: vec![script_path.display().to_string()],
            env: BTreeMap::new(),
            url: String::new(),
            enabled: true,
        })
    } else {
        let src_dir = target_dir.join("src");
        fs::create_dir_all(&src_dir).map_err(|e| format!("Cannot create src dir: {}", e))?;
        let cargo_toml = format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            name
        );
        fs::write(target_dir.join("Cargo.toml"), cargo_toml).map_err(|e| format!("Write failed: {}", e))?;
        let main_rs = r##"use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines().flatten() {
        let t = line.trim();
        if t.is_empty() || t.contains("\"notifications/initialized\"") { continue; }
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
"##;
        fs::write(src_dir.join("main.rs"), main_rs).map_err(|e| format!("Write failed: {}", e))?;
        Ok(McpServerConfig {
            command: "cargo".to_string(),
            args: vec![
                "run".to_string(),
                "--quiet".to_string(),
                "--manifest-path".to_string(),
                target_dir.join("Cargo.toml").display().to_string(),
            ],
            env: BTreeMap::new(),
            url: String::new(),
            enabled: true,
        })
    }
}

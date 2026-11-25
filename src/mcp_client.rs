use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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

pub fn config_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("Yokahu").join("mcp_config.json")
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

        match spawn_and_exchange(scfg, &reqs) {
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

    let resps = spawn_and_exchange(scfg, &reqs)?;
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

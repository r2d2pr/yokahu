use crate::mcp_client::{self, DiscoveredMcpTool, McpConfigFile, McpPool};
use crate::skills;
use crate::tools::{self, BackgroundTasks, SafetyMode};
use crate::ui::{ChatMessage, PendingApproval, PendingQuestion, ProviderKind, TaskStep, WorkerEvent};
use eframe::egui;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

fn build_url(base_url: &str, subpath: &str) -> String {
    format!("{}{}", base_url.trim().trim_end_matches('/'), subpath)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into()
}

pub fn fetch_models(
    prov: ProviderKind,
    base_url: &str,
    api_key: &str,
    tx: &Sender<WorkerEvent>,
    ctx: &egui::Context,
) {
    let subpath = match prov {
        ProviderKind::Ollama => "/api/tags",
        ProviderKind::OpenAiCompat => "/models",
    };
    let url = build_url(base_url, subpath);
    let mut req = agent().get(&url);
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", api_key.trim()));
    }
    let Ok(mut resp) = req.call() else {
        let _ = tx.send(WorkerEvent::Models(Vec::new(), false));
        ctx.request_repaint();
        return;
    };
    let raw = resp.body_mut().read_to_string().unwrap_or_default();
    let list: Vec<String> = if let Ok(v) = serde_json::from_str::<Value>(&raw) {
        match prov {
            ProviderKind::Ollama => v
                .get("models")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            ProviderKind::OpenAiCompat => v
                .get("data")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.get("id").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        }
    } else {
        Vec::new()
    };
    let _ = tx.send(WorkerEvent::Models(list, true));
    ctx.request_repaint();
}

pub fn pull_ollama_model(base_url: &str, model: &str, tx: &Sender<WorkerEvent>, ctx: &egui::Context) {
    let url = build_url(base_url, "/api/pull");
    let payload = json!({ "model": model, "stream": true }).to_string();
    let Ok(resp) = agent()
        .post(&url)
        .header("Content-Type", "application/json")
        .send(payload.as_str())
    else {
        let _ = tx.send(WorkerEvent::PullStatus("Failed to connect to Ollama".to_string(), false));
        ctx.request_repaint();
        return;
    };
    let reader = BufReader::new(resp.into_body().into_reader());
    for line in reader.lines().flatten() {
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            if let Some(st) = v.get("status").and_then(|s| s.as_str()) {
                let done = st == "success";
                let _ = tx.send(WorkerEvent::PullStatus(format!("{}: {}", model, st), done));
                ctx.request_repaint();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_agent_loop(
    history: Vec<ChatMessage>,
    prov: ProviderKind,
    base_url: String,
    api_key: String,
    model: String,
    worker_model: String,
    system: String,
    think: bool,
    temp: f32,
    num_ctx: i64,
    keep_alive: String,
    workspace: Option<PathBuf>,
    safety: SafetyMode,
    web_tools_enabled: bool,
    include_antigravity_skills: bool,
    mcp_cfg: McpConfigFile,
    mcp_tools: Vec<DiscoveredMcpTool>,
    mcp_pool: Arc<McpPool>,
    bg_tasks: Arc<BackgroundTasks>,
    tx: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let mut api_msgs: Vec<Value> = Vec::new();
    let mut sys_full = system;
    sys_full.push_str(&skills::load_workspace_rules(workspace.as_deref()));
    let discovered_skills = skills::discover_skills(workspace.as_deref(), include_antigravity_skills);
    if !discovered_skills.is_empty() {
        sys_full.push_str("\n\nAvailable Skills (use `read_file` on SKILL.md if relevant):");
        for sk in discovered_skills.iter().take(20) {
            sys_full.push_str(&format!("\n- {} ({}): {}", sk.name, sk.path.display(), sk.description));
        }
    }
    if let Some(ws) = &workspace {
        sys_full.push_str(&format!(
            "\nActive workspace directory: {}\nWorkspace tools: list_dir, read_file, search_files, find_files, write_file, edit_file, run_command, spawn_background, manage_tasks.",
            ws.display()
        ));
    }
    if !mcp_tools.is_empty() {
        sys_full.push_str("\nYou also have user-configured MCP tools prefixed with mcp__<server>__<tool>.");
    }
    if !sys_full.trim().is_empty() {
        api_msgs.push(json!({ "role": "system", "content": sys_full }));
    }
    for m in history {
        if m.role == "tool" {
            api_msgs.push(json!({ "role": "tool", "content": m.content }));
        } else if m.images.is_empty() {
            api_msgs.push(json!({ "role": m.role, "content": m.content }));
        } else {
            api_msgs.push(json!({ "role": m.role, "content": m.content, "images": m.images }));
        }
    }

    let mut all_tools: Vec<Value> = Vec::new();
    if let Some(arr) = tools::agent_ui_tools_schema().as_array() {
        all_tools.extend(arr.iter().cloned());
    }
    if workspace.is_some() {
        if let Some(arr) = tools::ollama_tools_schema().as_array() {
            all_tools.extend(arr.iter().cloned());
        }
    }
    if web_tools_enabled {
        if let Some(arr) = tools::web_tools_schema().as_array() {
            all_tools.extend(arr.iter().cloned());
        }
    }
    all_tools.extend(mcp_client::to_tool_schemas(&mcp_tools));

    let http_client = agent();
    let mut use_tools = !all_tools.is_empty();

    for _step in 0..25 {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let build_payload = |with_tools: bool| -> (&'static str, String) {
            match prov {
                ProviderKind::Ollama => {
                    let mut body = json!({
                        "model": model,
                        "messages": api_msgs,
                        "stream": true,
                        "think": think,
                        "keep_alive": keep_alive,
                        "options": { "temperature": temp, "num_ctx": num_ctx }
                    });
                    if with_tools && !all_tools.is_empty() {
                        body["tools"] = Value::Array(all_tools.clone());
                    }
                    ("/api/chat", body.to_string())
                }
                ProviderKind::OpenAiCompat => {
                    let mut body = json!({
                        "model": model,
                        "messages": api_msgs,
                        "stream": true,
                        "temperature": temp
                    });
                    if with_tools && !all_tools.is_empty() {
                        body["tools"] = Value::Array(all_tools.clone());
                    }
                    ("/chat/completions", body.to_string())
                }
            }
        };

        let (subpath, payload) = build_payload(use_tools);
        let url = build_url(&base_url, subpath);
        let send_req = |u: &str, p: &str| {
            let mut req = http_client.post(u).header("Content-Type", "application/json");
            if !api_key.trim().is_empty() {
                req = req.header("Authorization", &format!("Bearer {}", api_key.trim()));
            }
            req.send(p)
        };

        let resp = match send_req(&url, &payload) {
            Ok(r) => r,
            Err(ureq::Error::StatusCode(400)) if use_tools => {
                // Fallback for models that do not support tool calling
                use_tools = false;
                let (_, fallback_payload) = build_payload(false);
                match send_req(&url, &fallback_payload) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(WorkerEvent::Error(format!("Request failed ({}): {}", url, e)));
                        break;
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(WorkerEvent::Error(format!("Request failed ({}): {}", url, e)));
                break;
            }
        };

        let mut assistant_text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut oai_tc_acc: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        let reader = BufReader::new(resp.into_body().into_reader());

        for line in reader.lines().flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut one = line.trim().to_string();
            if one.is_empty() {
                continue;
            }
            if prov == ProviderKind::OpenAiCompat {
                if let Some(stripped) = one.strip_prefix("data:") {
                    one = stripped.trim().to_string();
                } else {
                    continue;
                }
                if one == "[DONE]" {
                    break;
                }
            }
            if let Ok(v) = serde_json::from_str::<Value>(&one) {
                match prov {
                    ProviderKind::Ollama => {
                        if let Some(msg) = v.get("message") {
                            if let Some(t) = msg.get("thinking").and_then(|x| x.as_str()) {
                                if !t.is_empty() {
                                    let _ = tx.send(WorkerEvent::Thinking(t.to_string()));
                                    ctx.request_repaint();
                                }
                            }
                            if let Some(c) = msg.get("content").and_then(|x| x.as_str()) {
                                if !c.is_empty() {
                                    assistant_text.push_str(c);
                                    let _ = tx.send(WorkerEvent::Token(c.to_string()));
                                    ctx.request_repaint();
                                }
                            }
                            if let Some(tcs) = msg.get("tool_calls").and_then(|x| x.as_array()) {
                                tool_calls.extend(tcs.iter().cloned());
                            }
                        }
                    }
                    ProviderKind::OpenAiCompat => {
                        if let Some(delta) = v
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("delta"))
                        {
                            if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                                if !c.is_empty() {
                                    assistant_text.push_str(c);
                                    let _ = tx.send(WorkerEvent::Token(c.to_string()));
                                    ctx.request_repaint();
                                }
                            }
                            if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                                for tc in tcs {
                                    let idx = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                                    let entry = oai_tc_acc
                                        .entry(idx)
                                        .or_insert_with(|| (String::new(), String::new(), String::new()));
                                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                                        entry.0.push_str(id);
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                                            entry.1.push_str(n);
                                        }
                                        if let Some(a) = f.get("arguments").and_then(|x| x.as_str()) {
                                            entry.2.push_str(a);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if prov == ProviderKind::OpenAiCompat && !oai_tc_acc.is_empty() {
            for (_idx, (id, name, args)) in oai_tc_acc {
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                }));
            }
        }

        if tool_calls.is_empty() || cancel.load(Ordering::Relaxed) {
            break;
        }

        api_msgs.push(json!({
            "role": "assistant",
            "content": assistant_text,
            "tool_calls": tool_calls
        }));

        for tc in tool_calls {
            let tc_id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let func = tc.get("function").unwrap_or(&tc);
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({})),
                Some(v) => v.clone(),
                None => json!({}),
            };

            let _ = tx.send(WorkerEvent::ToolStart(name.to_string()));
            ctx.request_repaint();

            // Handle Antigravity-style UI & Subagent tools directly
            if name == "update_focus" {
                let focus = args.get("focus").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let _ = tx.send(WorkerEvent::Focus(focus));
                ctx.request_repaint();
                api_msgs.push(json!({ "role": "tool", "content": "OK" }));
                continue;
            }
            if name == "update_steps" {
                let steps: Vec<TaskStep> = args
                    .get("steps")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| {
                                Some(TaskStep {
                                    title: s.get("title")?.as_str()?.to_string(),
                                    status: s.get("status")?.as_str().unwrap_or("pending").to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = tx.send(WorkerEvent::Steps(steps));
                ctx.request_repaint();
                api_msgs.push(json!({ "role": "tool", "content": "Steps updated." }));
                continue;
            }
            if name == "ask_question" {
                let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let options: Vec<String> = args
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let (qtx, qrx) = mpsc::channel();
                let _ = tx.send(WorkerEvent::AskQuestion(PendingQuestion {
                    question,
                    options,
                    reply_tx: qtx,
                }));
                ctx.request_repaint();
                let answer = qrx.recv().unwrap_or_else(|_| "No answer".to_string());
                if prov == ProviderKind::OpenAiCompat && !tc_id.is_empty() {
                    api_msgs.push(json!({ "role": "tool", "tool_call_id": tc_id, "content": answer }));
                } else {
                    api_msgs.push(json!({ "role": "tool", "content": answer }));
                }
                continue;
            }
            if name == "invoke_subagent" {
                let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("Worker");
                let sub_prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                let w_model = if worker_model.trim().is_empty() { &model } else { &worker_model };
                let _ = tx.send(WorkerEvent::Focus(format!("Subagent ({}): {}", role, w_model)));
                ctx.request_repaint();
                let sub_out = run_subagent(
                    prov,
                    &base_url,
                    &api_key,
                    w_model,
                    role,
                    sub_prompt,
                    workspace.as_deref(),
                    num_ctx,
                    &keep_alive,
                );
                let _ = tx.send(WorkerEvent::ToolResult {
                    name: format!("subagent:{}", role),
                    output: sub_out.clone(),
                });
                ctx.request_repaint();
                if prov == ProviderKind::OpenAiCompat && !tc_id.is_empty() {
                    api_msgs.push(json!({ "role": "tool", "tool_call_id": tc_id, "content": sub_out }));
                } else {
                    api_msgs.push(json!({ "role": "tool", "content": sub_out }));
                }
                continue;
            }

            let is_mcp = name.starts_with("mcp__");
            let mut allowed = true;
            if is_mcp || tools::is_mutating_tool(name) {
                match safety {
                    SafetyMode::ReadOnly => allowed = false,
                    SafetyMode::Safe => {
                        let preview = if is_mcp {
                            format!("MCP Tool: {}\nArgs: {}", name, args)
                        } else {
                            tools::preview_tool(name, &args)
                        };
                        let diff_lines = if name == "write_file" {
                            tools::compute_write_diff(workspace.as_deref(), &args)
                        } else if name == "edit_file" {
                            tools::compute_edit_diff(workspace.as_deref(), &args)
                        } else {
                            Vec::new()
                        };
                        let (rtx, rrx) = mpsc::channel();
                        let _ = tx.send(WorkerEvent::NeedApproval(PendingApproval {
                            tool_name: name.to_string(),
                            preview,
                            diff_lines,
                            reply_tx: rtx,
                        }));
                        ctx.request_repaint();
                        allowed = rrx.recv().unwrap_or(false);
                    }
                    SafetyMode::AutoApprove => allowed = true,
                }
            }

            let output = if !allowed {
                format!("Tool '{}' denied by user or Read-Only mode.", name)
            } else if is_mcp {
                mcp_pool
                    .call_tool(&mcp_cfg, name, &args)
                    .unwrap_or_else(|e| format!("MCP error: {}", e))
            } else if tools::is_web_tool(name) {
                tools::execute_web_tool(name, &args)
            } else if name == "spawn_background" {
                let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                bg_tasks.spawn(workspace.as_deref(), cmd)
            } else if name == "manage_tasks" {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
                let id = args.get("id").and_then(|v| v.as_u64());
                bg_tasks.manage(action, id)
            } else {
                tools::execute_tool(workspace.as_deref(), name, &args)
            };

            let _ = tx.send(WorkerEvent::ToolResult {
                name: name.to_string(),
                output: output.clone(),
            });
            ctx.request_repaint();
            if prov == ProviderKind::OpenAiCompat && !tc_id.is_empty() {
                api_msgs.push(json!({ "role": "tool", "tool_call_id": tc_id, "content": output }));
            } else {
                api_msgs.push(json!({ "role": "tool", "content": output }));
            }
        }
    }

    let _ = tx.send(WorkerEvent::Focus(String::new()));
    let _ = tx.send(WorkerEvent::Done);
    ctx.request_repaint();
}

fn run_subagent(
    prov: ProviderKind,
    base_url: &str,
    api_key: &str,
    model: &str,
    role: &str,
    prompt: &str,
    workspace: Option<&Path>,
    num_ctx: i64,
    keep_alive: &str,
) -> String {
    let mut sys = format!("You are a focused subagent ({}). Return a concise, factual summary.", role);
    if let Some(ws) = workspace {
        sys.push_str(&format!(" Workspace: {}", ws.display()));
    }
    let mut msgs = vec![
        json!({ "role": "system", "content": sys }),
        json!({ "role": "user", "content": prompt }),
    ];
    let ro_tools = json!([
        { "type": "function", "function": { "name": "list_dir", "parameters": { "type": "object", "properties": { "path": { "type": "string" } } } } },
        { "type": "function", "function": { "name": "read_file", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "start_line": { "type": "integer" }, "end_line": { "type": "integer" } }, "required": ["path"] } } },
        { "type": "function", "function": { "name": "search_files", "parameters": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] } } },
        { "type": "function", "function": { "name": "find_files", "parameters": { "type": "object", "properties": { "pattern": { "type": "string" } }, "required": ["pattern"] } } }
    ]);
    let client = agent();
    let (subpath, is_oai) = match prov {
        ProviderKind::Ollama => ("/api/chat", false),
        ProviderKind::OpenAiCompat => ("/chat/completions", true),
    };
    let url = build_url(base_url, subpath);

    for _ in 0..6 {
        let mut body = if is_oai {
            json!({ "model": model, "messages": msgs, "stream": false, "temperature": 0.2 })
        } else {
            json!({ "model": model, "messages": msgs, "stream": false, "keep_alive": keep_alive, "options": { "temperature": 0.2, "num_ctx": num_ctx } })
        };
        if workspace.is_some() {
            body["tools"] = ro_tools.clone();
        }
        let mut req = client.post(&url).header("Content-Type", "application/json");
        if !api_key.trim().is_empty() {
            req = req.header("Authorization", &format!("Bearer {}", api_key.trim()));
        }
        let Ok(mut resp) = req.send(body.to_string().as_str()) else {
            return "Subagent request failed.".to_string();
        };
        let raw = resp.body_mut().read_to_string().unwrap_or_default();
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            return raw;
        };
        let msg = if is_oai {
            v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message")).cloned().unwrap_or(json!({}))
        } else {
            v.get("message").cloned().unwrap_or(json!({}))
        };
        let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
        let tcs = msg.get("tool_calls").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        if tcs.is_empty() {
            return content;
        }
        msgs.push(msg);
        for tc in tcs {
            let func = tc.get("function").unwrap_or(&tc);
            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or(json!({})),
                Some(v) => v.clone(),
                None => json!({}),
            };
            let out = tools::execute_tool(workspace, name, &args);
            msgs.push(json!({ "role": "tool", "content": out }));
        }
    }
    "Subagent reached step limit.".to_string()
}

pub fn run_compare_stream(
    history: Vec<ChatMessage>,
    prov: ProviderKind,
    base_url: String,
    api_key: String,
    model: String,
    system: String,
    temp: f32,
    num_ctx: i64,
    keep_alive: String,
    tx: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let mut api_msgs: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        api_msgs.push(json!({ "role": "system", "content": system }));
    }
    for m in history {
        if m.role == "user" || m.role == "assistant" {
            api_msgs.push(json!({ "role": m.role, "content": m.content }));
        }
    }
    let (subpath, payload) = match prov {
        ProviderKind::Ollama => (
            "/api/chat",
            json!({
                "model": model,
                "messages": api_msgs,
                "stream": true,
                "keep_alive": keep_alive,
                "options": { "temperature": temp, "num_ctx": num_ctx }
            })
            .to_string(),
        ),
        ProviderKind::OpenAiCompat => (
            "/chat/completions",
            json!({ "model": model, "messages": api_msgs, "stream": true, "temperature": temp }).to_string(),
        ),
    };
    let url = build_url(&base_url, subpath);
    let mut req = agent().post(&url).header("Content-Type", "application/json");
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", api_key.trim()));
    }
    if let Ok(resp) = req.send(payload.as_str()) {
        let reader = BufReader::new(resp.into_body().into_reader());
        for line in reader.lines().flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut one = line.trim().to_string();
            if one.is_empty() {
                continue;
            }
            if prov == ProviderKind::OpenAiCompat {
                if let Some(s) = one.strip_prefix("data:") {
                    one = s.trim().to_string();
                } else {
                    continue;
                }
                if one == "[DONE]" {
                    break;
                }
            }
            if let Ok(v) = serde_json::from_str::<Value>(&one) {
                let tok = match prov {
                    ProviderKind::Ollama => v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()),
                    ProviderKind::OpenAiCompat => v
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                        .and_then(|d| d.get("content"))
                        .and_then(|c| c.as_str()),
                };
                if let Some(t) = tok {
                    if !t.is_empty() {
                        let _ = tx.send(WorkerEvent::CompareToken(t.to_string()));
                        ctx.request_repaint();
                    }
                }
            }
        }
    }
}

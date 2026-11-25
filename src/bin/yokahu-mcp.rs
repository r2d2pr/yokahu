use yokahu::json::{self, obj, Json};
use yokahu::ollama;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

fn default_model() -> String {
    std::env::var("YOKAHU_MODEL")
        .or_else(|_| std::env::var("QWEN_SLAVE_MODEL"))
        .unwrap_or_else(|_| "qwen3.5:9b".to_string())
}

fn default_num_ctx() -> i64 {
    std::env::var("QWEN_NUM_CTX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8192)
}

fn default_keep_alive() -> String {
    std::env::var("QWEN_KEEP_ALIVE").unwrap_or_else(|_| "5m".to_string())
}

fn chat(
    prompt: &str,
    system: &str,
    model: &str,
    temperature: f64,
    think: bool,
    num_ctx: i64,
    keep_alive: &str,
    format: Option<&Json>,
) -> Result<String, String> {
    ollama::ensure_ollama_running()?;

    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(obj(vec![
            ("role", Json::Str("system".to_string())),
            ("content", Json::Str(system.to_string())),
        ]));
    }
    messages.push(obj(vec![
        ("role", Json::Str("user".to_string())),
        ("content", Json::Str(prompt.to_string())),
    ]));

    let mut fields = vec![
        ("model", Json::Str(model.to_string())),
        ("messages", Json::Arr(messages)),
        ("stream", Json::Bool(false)),
        ("think", Json::Bool(think)),
        ("keep_alive", Json::Str(keep_alive.to_string())),
        (
            "options",
            obj(vec![
                ("temperature", Json::Num(temperature)),
                ("num_ctx", Json::Num(num_ctx as f64)),
            ]),
        ),
    ];
    if let Some(fmt) = format {
        if !matches!(fmt, Json::Null) {
            fields.push(("format", fmt.clone()));
        }
    }

    let body = obj(fields);
    let res = ollama::http_json("POST", "/api/chat", Some(&body), 600)?;
    let msg = res
        .get("message")
        .ok_or_else(|| "Missing 'message' in Ollama response".to_string())?;
    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let thinking = msg.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
    if think && !thinking.is_empty() {
        Ok(format!("<thinking>\n{}\n</thinking>\n\n{}", thinking, content))
    } else {
        Ok(content)
    }
}

fn tool_text(text: String, is_error: bool) -> Json {
    let content = Json::Arr(vec![obj(vec![
        ("type", Json::Str("text".to_string())),
        ("text", Json::Str(text)),
    ])]);
    if is_error {
        obj(vec![("content", content), ("isError", Json::Bool(true))])
    } else {
        obj(vec![("content", content)])
    }
}

fn handle_tool_call(name: &str, args: &Json) -> Json {
    let def_model = default_model();
    let def_ctx = default_num_ctx();
    let def_ka = default_keep_alive();

    match name {
        "qwen_status" => {
            let state = match ollama::ensure_ollama_running() {
                Ok(s) => s,
                Err(e) => return tool_text(format!("Ollama unavailable: {}", e), true),
            };
            let tags = match ollama::http_json("GET", "/api/tags", None, 5) {
                Ok(t) => t,
                Err(e) => return tool_text(format!("Failed to list models: {}", e), true),
            };
            let models: Vec<Json> = tags
                .get("models")
                .and_then(|m| m.as_arr())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.get("name").and_then(|n| n.as_str()))
                        .map(|s| Json::Str(s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let (host, port) = ollama::ollama_host_port();
            let info = obj(vec![
                ("ollama", Json::Str(state.to_string())),
                ("url", Json::Str(format!("http://{}:{}", host, port))),
                ("default_model", Json::Str(def_model)),
                ("default_num_ctx", Json::Num(def_ctx as f64)),
                ("default_keep_alive", Json::Str(def_ka)),
                ("models", Json::Arr(models)),
            ]);
            tool_text(info.encode(), false)
        }
        "qwen_ask" => {
            let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            if prompt.is_empty() {
                return tool_text("Missing required 'prompt'.".to_string(), true);
            }
            let system = args.get("system").and_then(|v| v.as_str()).unwrap_or("");
            let model = args.get("model").and_then(|v| v.as_str()).unwrap_or(&def_model);
            let temp = args.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.3);
            let think = args.get("think").and_then(|v| v.as_bool()).unwrap_or(false);
            let num_ctx = args.get("num_ctx").and_then(|v| v.as_i64()).unwrap_or(def_ctx);
            let keep_alive = args.get("keep_alive").and_then(|v| v.as_str()).unwrap_or(&def_ka);
            let format = args.get("format");

            match chat(prompt, system, model, temp, think, num_ctx, keep_alive, format) {
                Ok(reply) => tool_text(reply, false),
                Err(e) => tool_text(format!("Error: {}", e), true),
            }
        }
        "qwen_analyze_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() || !Path::new(path).is_file() {
                return tool_text(format!("File not found: {}", path), true);
            }
            let question = args
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("Summarize this file concisely.");
            let start_line = args.get("start_line").and_then(|v| v.as_i64()).unwrap_or(1).max(1) as usize;
            let end_line = args.get("end_line").and_then(|v| v.as_i64()).unwrap_or(400).max(1) as usize;
            let model = args.get("model").and_then(|v| v.as_str()).unwrap_or(&def_model);
            let think = args.get("think").and_then(|v| v.as_bool()).unwrap_or(false);
            let num_ctx = args.get("num_ctx").and_then(|v| v.as_i64()).unwrap_or(def_ctx);
            let keep_alive = args.get("keep_alive").and_then(|v| v.as_str()).unwrap_or(&def_ka);
            let format = args.get("format");

            let raw = match fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => return tool_text(format!("Failed to read {}: {}", path, e), true),
            };
            let lines: Vec<&str> = raw.lines().collect();
            let total = lines.len();
            let s_idx = (start_line - 1).min(total);
            let e_idx = end_line.min(total);
            let snippet = lines[s_idx..e_idx].join("\n");
            let prompt = format!(
                "File: {} (lines {}-{} of {})\n\n```\n{}\n```\n\nTask: {}",
                path, start_line, e_idx, total, snippet, question
            );
            let system = "You are a precise local code and document analysis worker. Answer concisely and accurately based only on the provided file.";
            match chat(&prompt, system, model, 0.2, think, num_ctx, keep_alive, format) {
                Ok(reply) => tool_text(reply, false),
                Err(e) => tool_text(format!("Error: {}", e), true),
            }
        }
        "qwen_batch" => {
            let system = args.get("system").and_then(|v| v.as_str()).unwrap_or("");
            let model = args.get("model").and_then(|v| v.as_str()).unwrap_or(&def_model);
            let temp = args.get("temperature").and_then(|v| v.as_f64()).unwrap_or(0.2);
            let think = args.get("think").and_then(|v| v.as_bool()).unwrap_or(false);
            let num_ctx = args.get("num_ctx").and_then(|v| v.as_i64()).unwrap_or(def_ctx);
            let keep_alive = args.get("keep_alive").and_then(|v| v.as_str()).unwrap_or(&def_ka);
            let format = args.get("format");

            let mut results = Vec::new();
            if let Some(prompts) = args.get("prompts").and_then(|v| v.as_arr()) {
                for (idx, item) in prompts.iter().enumerate() {
                    let p = item.as_str().unwrap_or("");
                    if p.is_empty() {
                        continue;
                    }
                    let res = match chat(p, system, model, temp, think, num_ctx, keep_alive, format) {
                        Ok(out) => obj(vec![
                            ("index", Json::Num(idx as f64)),
                            ("ok", Json::Bool(true)),
                            ("output", Json::Str(out)),
                        ]),
                        Err(e) => obj(vec![
                            ("index", Json::Num(idx as f64)),
                            ("ok", Json::Bool(false)),
                            ("error", Json::Str(e)),
                        ]),
                    };
                    results.push(res);
                }
            } else if let Some(paths) = args.get("paths").and_then(|v| v.as_arr()) {
                let question = args
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Summarize this file concisely.");
                for item in paths {
                    let path = item.as_str().unwrap_or("");
                    let res = match fs::read_to_string(path) {
                        Ok(raw) => {
                            let snippet: String = raw.lines().take(400).collect::<Vec<_>>().join("\n");
                            let p = format!("File: {}\n\n```\n{}\n```\n\nTask: {}", path, snippet, question);
                            match chat(&p, system, model, temp, think, num_ctx, keep_alive, format) {
                                Ok(out) => obj(vec![
                                    ("path", Json::Str(path.to_string())),
                                    ("ok", Json::Bool(true)),
                                    ("output", Json::Str(out)),
                                ]),
                                Err(e) => obj(vec![
                                    ("path", Json::Str(path.to_string())),
                                    ("ok", Json::Bool(false)),
                                    ("error", Json::Str(e)),
                                ]),
                            }
                        }
                        Err(e) => obj(vec![
                            ("path", Json::Str(path.to_string())),
                            ("ok", Json::Bool(false)),
                            ("error", Json::Str(e.to_string())),
                        ]),
                    };
                    results.push(res);
                }
            } else {
                return tool_text("Provide either 'prompts' or 'paths' array.".to_string(), true);
            }

            tool_text(Json::Arr(results).encode(), false)
        }
        other => tool_text(format!("Unknown tool: {}", other), true),
    }
}

fn common_props() -> Vec<(&'static str, Json)> {
    vec![
        ("system", obj(vec![("type", Json::Str("string".to_string())), ("description", Json::Str("Optional system instruction.".to_string()))])),
        ("model", obj(vec![("type", Json::Str("string".to_string())), ("description", Json::Str("Ollama model name (default: qwen3.5:9b).".to_string()))])),
        ("temperature", obj(vec![("type", Json::Str("number".to_string())), ("description", Json::Str("Sampling temperature (default: 0.3).".to_string()))])),
        ("think", obj(vec![("type", Json::Str("boolean".to_string())), ("description", Json::Str("Enable Qwen3.5 reasoning/thinking mode (default: false).".to_string()))])),
        ("num_ctx", obj(vec![("type", Json::Str("integer".to_string())), ("description", Json::Str("Context window size (default: 8192).".to_string()))])),
        ("keep_alive", obj(vec![("type", Json::Str("string".to_string())), ("description", Json::Str("VRAM keep-alive duration e.g. '5m', or '0' to unload immediately.".to_string()))])),
        ("format", obj(vec![("description", Json::Str("Optional 'json' string or JSON Schema object for structured output.".to_string()))])),
    ]
}

fn tools_schema() -> Json {
    let mut ask_props = vec![(
        "prompt",
        obj(vec![
            ("type", Json::Str("string".to_string())),
            ("description", Json::Str("Task or prompt for Qwen3.5-9B.".to_string())),
        ]),
    )];
    ask_props.extend(common_props());

    let mut file_props = vec![
        ("path", obj(vec![("type", Json::Str("string".to_string())), ("description", Json::Str("Absolute path to the local file.".to_string()))])),
        ("question", obj(vec![("type", Json::Str("string".to_string())), ("description", Json::Str("What to extract, summarize, or analyze.".to_string()))])),
        ("start_line", obj(vec![("type", Json::Str("integer".to_string())), ("description", Json::Str("1-based start line (default: 1).".to_string()))])),
        ("end_line", obj(vec![("type", Json::Str("integer".to_string())), ("description", Json::Str("1-based end line (default: 400).".to_string()))])),
    ];
    file_props.extend(common_props());

    let mut batch_props = vec![
        ("prompts", obj(vec![("type", Json::Str("array".to_string())), ("items", obj(vec![("type", Json::Str("string".to_string()))])), ("description", Json::Str("List of prompts to run sequentially.".to_string()))])),
        ("paths", obj(vec![("type", Json::Str("array".to_string())), ("items", obj(vec![("type", Json::Str("string".to_string()))])), ("description", Json::Str("List of file paths to analyze with 'question'.".to_string()))])),
        ("question", obj(vec![("type", Json::Str("string".to_string())), ("description", Json::Str("Question applied to each file when 'paths' is provided.".to_string()))])),
    ];
    batch_props.extend(common_props());

    Json::Arr(vec![
        obj(vec![
            ("name", Json::Str("qwen_ask".to_string())),
            ("description", Json::Str("Delegate a prompt or subtask to the local Qwen3.5-9B model running via Ollama.".to_string())),
            ("inputSchema", obj(vec![
                ("type", Json::Str("object".to_string())),
                ("properties", obj(ask_props)),
                ("required", Json::Arr(vec![Json::Str("prompt".to_string())])),
            ])),
        ]),
        obj(vec![
            ("name", Json::Str("qwen_analyze_file".to_string())),
            ("description", Json::Str("Have local Qwen3.5-9B read and summarize/analyze a local file without loading it into the main agent context.".to_string())),
            ("inputSchema", obj(vec![
                ("type", Json::Str("object".to_string())),
                ("properties", obj(file_props)),
                ("required", Json::Arr(vec![Json::Str("path".to_string())])),
            ])),
        ]),
        obj(vec![
            ("name", Json::Str("qwen_batch".to_string())),
            ("description", Json::Str("Run a batch of prompts or analyze multiple files with local Qwen3.5-9B in one call.".to_string())),
            ("inputSchema", obj(vec![
                ("type", Json::Str("object".to_string())),
                ("properties", obj(batch_props)),
            ])),
        ]),
        obj(vec![
            ("name", Json::Str("qwen_status".to_string())),
            ("description", Json::Str("Check local Ollama server status and installed models.".to_string())),
            ("inputSchema", obj(vec![
                ("type", Json::Str("object".to_string())),
                ("properties", obj(vec![])),
            ])),
        ]),
    ])
}

fn send(out: &mut io::StdoutLock, msg: &Json) {
    let _ = writeln!(out, "{}", msg.encode());
    let _ = out.flush();
}

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req = match json::parse(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        if method == "notifications/initialized" {
            continue;
        }
        let req_id = match req.get("id") {
            Some(id) if !matches!(id, Json::Null) => id.clone(),
            _ => continue,
        };
        let params = req.get("params").cloned().unwrap_or_else(|| obj(vec![]));

        let resp = match method {
            "initialize" => {
                let pv = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("2024-11-05")
                    .to_string();
                let result = obj(vec![
                    ("protocolVersion", Json::Str(pv)),
                    ("capabilities", obj(vec![("tools", obj(vec![]))])),
                    (
                        "serverInfo",
                        obj(vec![
                            ("name", Json::Str("qwen-slave".to_string())),
                            ("version", Json::Str("1.1.0".to_string())),
                        ]),
                    ),
                ]);
                obj(vec![
                    ("jsonrpc", Json::Str("2.0".to_string())),
                    ("id", req_id),
                    ("result", result),
                ])
            }
            "ping" => obj(vec![
                ("jsonrpc", Json::Str("2.0".to_string())),
                ("id", req_id),
                ("result", obj(vec![])),
            ]),
            "tools/list" => obj(vec![
                ("jsonrpc", Json::Str("2.0".to_string())),
                ("id", req_id),
                ("result", obj(vec![("tools", tools_schema())])),
            ]),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| obj(vec![]));
                let res = handle_tool_call(name, &args);
                obj(vec![
                    ("jsonrpc", Json::Str("2.0".to_string())),
                    ("id", req_id),
                    ("result", res),
                ])
            }
            other => obj(vec![
                ("jsonrpc", Json::Str("2.0".to_string())),
                ("id", req_id),
                (
                    "error",
                    obj(vec![
                        ("code", Json::Num(-32601.0)),
                        ("message", Json::Str(format!("Method not found: {}", other))),
                    ]),
                ),
            ]),
        };

        send(&mut out, &resp);
    }
}

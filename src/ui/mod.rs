pub mod agent;
pub mod icons;
pub mod modals;

use crate::markdown;
use crate::mcp_client::{self, DiscoveredMcpTool, McpConfigFile, McpPool};
use crate::ollama;
use crate::theme::{self, ThemeConfig};
use crate::tools::{self, SafetyMode, WorkspaceEntry};
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, RichText, Stroke};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const GREEN: Color32 = Color32::from_rgb(63, 185, 80);
pub const RED: Color32 = Color32::from_rgb(248, 81, 73);
pub const AMBER: Color32 = Color32::from_rgb(210, 153, 34);

const EMBLEM_PNG: &[u8] = include_bytes!("../../assets/emblem.png");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    Ollama,
    OpenAiCompat,
}

impl Default for ProviderKind {
    fn default() -> Self {
        ProviderKind::Ollama
    }
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "Ollama",
            ProviderKind::OpenAiCompat => "OpenAI-Compatible",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub is_image: bool,
    pub data: String,
    #[serde(skip)]
    pub thumb: Option<egui::TextureHandle>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub builtin: bool,
}

fn default_personas() -> Vec<Persona> {
    vec![
        Persona {
            name: "General".to_string(),
            prompt: "You are a local AI assistant running in Yokahu. Be concise, accurate, and practical.".to_string(),
            builtin: true,
        },
        Persona {
            name: "Systems Coder".to_string(),
            prompt: "You are an expert Rust and systems engineer. Prefer zero-overhead, idiomatic, safe code and verify edge cases.".to_string(),
            builtin: true,
        },
        Persona {
            name: "Code Reviewer".to_string(),
            prompt: "Review code strictly for correctness, security, concurrency bugs, and unnecessary complexity. Be direct.".to_string(),
            builtin: true,
        },
        Persona {
            name: "Concise".to_string(),
            prompt: "Answer in terse, high-signal bullet points. No fluff.".to_string(),
            builtin: true,
        },
    ]
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub thinking: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub compare_model: String,
    #[serde(default)]
    pub compare_content: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
    pub messages: Vec<ChatMessage>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    pub chats: Vec<Conversation>,
    pub active_id: Option<String>,
    pub workspace: Option<String>,
    pub provider: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub think: bool,
    pub temperature: f32,
    pub num_ctx: i64,
    pub keep_alive: String,
    pub safety_mode: SafetyMode,
    pub system_prompt: String,
    pub theme: ThemeConfig,
    #[serde(default = "default_personas")]
    pub personas: Vec<Persona>,
    #[serde(default)]
    pub active_persona: String,
    #[serde(default)]
    pub compare_enabled: bool,
    #[serde(default)]
    pub compare_model: String,
    #[serde(default)]
    pub whisper_cli: String,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            chats: Vec::new(),
            active_id: None,
            workspace: None,
            provider: ProviderKind::Ollama,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_key: String::new(),
            model: "qwen3.5:9b".to_string(),
            think: false,
            temperature: 0.3,
            num_ctx: 8192,
            keep_alive: "5m".to_string(),
            safety_mode: SafetyMode::Safe,
            system_prompt: "You are a local AI assistant running in Yokahu. Be concise, accurate, and practical.".to_string(),
            theme: ThemeConfig::default(),
            personas: default_personas(),
            active_persona: "General".to_string(),
            compare_enabled: false,
            compare_model: "qwen2.5-coder:7b-instruct-q6_K".to_string(),
            whisper_cli: "whisper".to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Chats,
    Files,
    Settings,
}

pub struct PendingApproval {
    pub tool_name: String,
    pub preview: String,
    pub diff_lines: Vec<(char, String)>,
    pub reply_tx: Sender<bool>,
}

pub enum WorkerEvent {
    Models(Vec<String>, bool),
    McpTools(Vec<DiscoveredMcpTool>, Vec<String>),
    PullStatus(String, bool),
    Thinking(String),
    Token(String),
    CompareToken(String),
    Transcript(String),
    ToolStart(String),
    ToolResult { name: String, output: String },
    NeedApproval(PendingApproval),
    Error(String),
    Done,
}

pub struct YokahuApp {
    pub state: PersistedState,
    pub models: Vec<String>,
    pub endpoint_ok: bool,
    pub history_filter: String,
    pub prompt_input: String,
    pub attachments: Vec<Attachment>,
    pub busy: bool,
    pub cancel_flag: Arc<AtomicBool>,
    pub rx: Receiver<WorkerEvent>,
    pub tx: Sender<WorkerEvent>,
    pub pending_approval: Option<PendingApproval>,
    pub show_sys_modal: bool,
    pub sys_draft: String,
    pub show_provider_modal: bool,
    pub pull_model_input: String,
    pub pull_status: String,
    pub show_mcp_modal: bool,
    pub mcp_cfg: McpConfigFile,
    pub mcp_tools: Vec<DiscoveredMcpTool>,
    pub mcp_errors: Vec<String>,
    pub new_mcp_name: String,
    pub new_mcp_cmd: String,
    pub new_mcp_args: String,
    pub new_mcp_env: String,
    pub new_mcp_url: String,
    pub new_mcp_is_http: bool,
    pub scaffold_name: String,
    pub scaffold_lang: String,
    pub mcp_status_msg: String,
    pub show_onboarding: bool,
    pub show_theme_modal: bool,
    pub show_sidebar: bool,
    pub sidebar_tab: SidebarTab,
    pub workspace_tree: Vec<WorkspaceEntry>,
    pub mcp_pool: Arc<McpPool>,
    pub tok_count: usize,
    pub gen_start: Option<Instant>,
    pub last_tok_per_sec: f32,
    pub show_palette: bool,
    pub palette_query: String,
    pub preview_file: Option<(String, PathBuf, String)>,
    pub new_persona_name: String,
    pub renaming_chat: Option<(String, String)>,
    pub recording_mic: bool,
    pub logo_tex: Option<egui::TextureHandle>,
    pub status_note: String,
}

impl YokahuApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = load_state();
        theme::apply_theme(&cc.egui_ctx, &state.theme);
        let mcp_cfg = mcp_client::load_config();
        let (tx, rx) = mpsc::channel();

        let tx_boot = tx.clone();
        let ctx_boot = cc.egui_ctx.clone();
        let prov = state.provider;
        let url = state.base_url.clone();
        let key = state.api_key.clone();
        let mcp_boot = mcp_cfg.clone();
        thread::spawn(move || {
            if prov == ProviderKind::Ollama && url.contains("127.0.0.1:11434") {
                let _ = ollama::ensure_ollama_running();
            }
            agent::fetch_models(prov, &url, &key, &tx_boot, &ctx_boot);
            let (tools, errs) = mcp_client::discover_tools(&mcp_boot);
            let _ = tx_boot.send(WorkerEvent::McpTools(tools, errs));
            ctx_boot.request_repaint();
        });

        let logo_tex = image::load_from_memory(EMBLEM_PNG).ok().map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            cc.egui_ctx.load_texture("yokahu_emblem", color_img, egui::TextureOptions::LINEAR)
        });

        let workspace_tree = state
            .workspace
            .as_deref()
            .map(|w| tools::list_workspace_tree(Path::new(w)))
            .unwrap_or_default();

        let mut app = Self {
            sys_draft: state.system_prompt.clone(),
            state,
            models: Vec::new(),
            endpoint_ok: false,
            history_filter: String::new(),
            prompt_input: String::new(),
            attachments: Vec::new(),
            busy: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            rx,
            tx,
            pending_approval: None,
            show_sys_modal: false,
            show_provider_modal: false,
            pull_model_input: String::new(),
            pull_status: String::new(),
            show_mcp_modal: false,
            mcp_cfg,
            mcp_tools: Vec::new(),
            mcp_errors: Vec::new(),
            new_mcp_name: String::new(),
            new_mcp_cmd: String::new(),
            new_mcp_args: String::new(),
            new_mcp_env: String::new(),
            new_mcp_url: String::new(),
            new_mcp_is_http: false,
            scaffold_name: "my-mcp".to_string(),
            scaffold_lang: "python".to_string(),
            mcp_status_msg: String::new(),
            show_onboarding: false,
            show_theme_modal: false,
            show_sidebar: true,
            sidebar_tab: SidebarTab::Chats,
            workspace_tree,
            mcp_pool: Arc::new(McpPool::default()),
            tok_count: 0,
            gen_start: None,
            last_tok_per_sec: 0.0,
            show_palette: false,
            palette_query: String::new(),
            preview_file: None,
            new_persona_name: String::new(),
            renaming_chat: None,
            recording_mic: false,
            logo_tex,
            status_note: String::new(),
        };
        if app.state.personas.is_empty() {
            app.state.personas = default_personas();
        }
        if app.state.chats.is_empty() {
            app.new_chat();
        }
        app
    }

    pub fn new_chat(&mut self) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let id = format!("c_{}", ts);
        self.state.chats.insert(
            0,
            Conversation {
                id: id.clone(),
                title: "New Conversation".to_string(),
                pinned: false,
                messages: Vec::new(),
            },
        );
        self.state.active_id = Some(id);
        save_state(&self.state);
    }

    pub fn active_chat_idx(&self) -> usize {
        if let Some(id) = &self.state.active_id {
            if let Some(pos) = self.state.chats.iter().position(|c| &c.id == id) {
                return pos;
            }
        }
        0
    }

    pub fn refresh_models(&self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();
        let prov = self.state.provider;
        let url = self.state.base_url.clone();
        let key = self.state.api_key.clone();
        thread::spawn(move || agent::fetch_models(prov, &url, &key, &tx, &ctx_c));
    }

    pub fn refresh_mcp(&self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();
        let cfg = self.mcp_cfg.clone();
        thread::spawn(move || {
            let (tools, errs) = mcp_client::discover_tools(&cfg);
            let _ = tx.send(WorkerEvent::McpTools(tools, errs));
            ctx_c.request_repaint();
        });
    }

    fn send_message(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        let raw = self.prompt_input.trim().to_string();
        if raw.is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.state.chats.is_empty() {
            self.new_chat();
        }
        let idx = self.active_chat_idx();

        let mut full_content = raw.clone();
        let mut images = Vec::new();
        for att in &self.attachments {
            if att.is_image {
                images.push(att.data.clone());
            } else {
                full_content.push_str(&format!("\n\n--- File: {} ---\n```\n{}\n```", att.name, att.data));
            }
        }

        if self.state.chats[idx].messages.is_empty() {
            let title_src = if !raw.is_empty() {
                raw.clone()
            } else {
                self.attachments.first().map(|a| a.name.clone()).unwrap_or_else(|| "Chat".into())
            };
            self.state.chats[idx].title = title_src.chars().take(40).collect();
        }

        self.state.chats[idx].messages.push(ChatMessage {
            role: "user".to_string(),
            content: full_content,
            thinking: String::new(),
            model: String::new(),
            images,
            tool_name: String::new(),
            compare_model: String::new(),
            compare_content: String::new(),
        });
        let cmp_model = if self.state.compare_enabled && !self.state.compare_model.trim().is_empty() {
            self.state.compare_model.clone()
        } else {
            String::new()
        };
        self.state.chats[idx].messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: String::new(),
            thinking: String::new(),
            model: self.state.model.clone(),
            images: Vec::new(),
            tool_name: String::new(),
            compare_model: cmp_model,
            compare_content: String::new(),
        });

        self.prompt_input.clear();
        self.attachments.clear();
        self.busy = true;
        self.tok_count = 0;
        self.gen_start = Some(Instant::now());
        self.last_tok_per_sec = 0.0;
        self.cancel_flag.store(false, Ordering::Relaxed);
        save_state(&self.state);
        self.spawn_worker(ctx, idx);
    }

    fn spawn_worker(&self, ctx: &egui::Context, idx: usize) {
        let history = self.state.chats[idx].messages[..self.state.chats[idx].messages.len() - 1].to_vec();
        let prov = self.state.provider;
        let base_url = self.state.base_url.clone();
        let api_key = self.state.api_key.clone();
        let model = self.state.model.clone();
        let system = self.state.system_prompt.clone();
        let think = self.state.think;
        let temp = self.state.temperature;
        let num_ctx = self.state.num_ctx;
        let keep_alive = self.state.keep_alive.clone();
        let workspace = self.state.workspace.clone().map(PathBuf::from);
        let safety = self.state.safety_mode;
        let mcp_cfg = self.mcp_cfg.clone();
        let mcp_tools = self.mcp_tools.clone();
        let mcp_pool = self.mcp_pool.clone();
        let tx = self.tx.clone();
        let cancel = self.cancel_flag.clone();
        let ctx_clone = ctx.clone();

        let tx_cmp = self.tx.clone();
        let cancel_cmp = self.cancel_flag.clone();
        let ctx_cmp = ctx.clone();
        let cmp_enabled = self.state.compare_enabled && !self.state.compare_model.trim().is_empty();
        let cmp_model = self.state.compare_model.clone();
        let cmp_hist = history.clone();
        let cmp_url = base_url.clone();
        let cmp_key = api_key.clone();
        let cmp_sys = system.clone();
        let cmp_ka = keep_alive.clone();

        thread::spawn(move || {
            agent::run_agent_loop(
                history, prov, base_url, api_key, model, system, think, temp, num_ctx, keep_alive,
                workspace, safety, mcp_cfg, mcp_tools, mcp_pool, tx, cancel, ctx_clone,
            );
        });
        if cmp_enabled {
            thread::spawn(move || {
                agent::run_compare_stream(
                    cmp_hist, prov, cmp_url, cmp_key, cmp_model, cmp_sys, temp, num_ctx, cmp_ka,
                    tx_cmp, cancel_cmp, ctx_cmp,
                );
            });
        }
    }

    fn fork_at(&mut self, msg_idx: usize) {
        if self.state.chats.is_empty() {
            return;
        }
        let cur = &self.state.chats[self.active_chat_idx()];
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let id = format!("c_{}", ts);
        let forked = Conversation {
            id: id.clone(),
            title: format!("Fork: {}", truncate(&cur.title, 32)),
            pinned: false,
            messages: cur.messages[..=msg_idx].to_vec(),
        };
        self.state.chats.insert(0, forked);
        self.state.active_id = Some(id);
        save_state(&self.state);
    }

    fn regenerate_at(&mut self, ctx: &egui::Context, msg_idx: usize) {
        if self.busy || self.state.chats.is_empty() {
            return;
        }
        let idx = self.active_chat_idx();
        self.state.chats[idx].messages.truncate(msg_idx);
        self.state.chats[idx].messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: String::new(),
            thinking: String::new(),
            model: self.state.model.clone(),
            images: Vec::new(),
            tool_name: String::new(),
            compare_model: String::new(),
            compare_content: String::new(),
        });
        self.busy = true;
        self.tok_count = 0;
        self.gen_start = Some(Instant::now());
        self.last_tok_per_sec = 0.0;
        self.cancel_flag.store(false, Ordering::Relaxed);
        save_state(&self.state);
        self.spawn_worker(ctx, idx);
    }

    fn export_active_chat(&mut self, as_json: bool) {
        if self.state.chats.is_empty() {
            return;
        }
        let chat = &self.state.chats[self.active_chat_idx()];
        let ext = if as_json { "json" } else { "md" };
        let default_name = format!("{}.{}", sanitize_filename(&chat.title), ext);
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter(ext.to_uppercase(), &[ext])
            .save_file()
        {
            let content = if as_json {
                serde_json::to_string_pretty(chat).unwrap_or_default()
            } else {
                let mut md = format!("# {}\n\n", chat.title);
                for m in &chat.messages {
                    md.push_str(&format!("## {}\n\n{}\n\n", m.role.to_uppercase(), m.content));
                }
                md
            };
            match fs::write(&path, content) {
                Ok(_) => self.status_note = format!("Exported to {}", path.display()),
                Err(e) => self.status_note = format!("Export failed: {}", e),
            }
        }
    }
}

impl eframe::App for YokahuApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let th = self.state.theme.clone();

        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                WorkerEvent::Models(list, ok) => {
                    self.endpoint_ok = ok;
                    if !ok || list.is_empty() {
                        self.show_onboarding = true;
                    }
                    if !list.is_empty() {
                        self.models = list;
                        if self.state.model.trim().is_empty() {
                            self.state.model = self.models[0].clone();
                        }
                    }
                }
                WorkerEvent::McpTools(tools, errs) => {
                    self.mcp_tools = tools;
                    self.mcp_errors = errs;
                }
                WorkerEvent::PullStatus(msg, refresh) => {
                    self.pull_status = msg;
                    if refresh {
                        self.refresh_models(&ctx);
                    }
                }
                WorkerEvent::Thinking(chunk) => {
                    self.tok_count += 1;
                    if let Some(st) = self.gen_start {
                        self.last_tok_per_sec = self.tok_count as f32 / st.elapsed().as_secs_f32().max(0.05);
                    }
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            last.thinking.push_str(&chunk);
                        }
                    }
                }
                WorkerEvent::Token(chunk) => {
                    self.tok_count += 1;
                    if let Some(st) = self.gen_start {
                        self.last_tok_per_sec = self.tok_count as f32 / st.elapsed().as_secs_f32().max(0.05);
                    }
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            last.content.push_str(&chunk);
                        }
                    }
                }
                WorkerEvent::CompareToken(chunk) => {
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            last.compare_content.push_str(&chunk);
                        }
                    }
                }
                WorkerEvent::Transcript(text) => {
                    if !self.prompt_input.is_empty() {
                        self.prompt_input.push(' ');
                    }
                    self.prompt_input.push_str(&text);
                }
                WorkerEvent::ToolStart(desc) => {
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            if !last.content.is_empty() {
                                last.content.push_str("\n\n");
                            }
                            last.content.push_str(&format!("`[tool] {}`\n", desc));
                        }
                    }
                }
                WorkerEvent::ToolResult { name, output } => {
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        let pos = self.state.chats[idx].messages.len().saturating_sub(1);
                        self.state.chats[idx].messages.insert(
                            pos,
                            ChatMessage {
                                role: "tool".to_string(),
                                content: output,
                                thinking: String::new(),
                                model: String::new(),
                                images: Vec::new(),
                                tool_name: name,
                                compare_model: String::new(),
                                compare_content: String::new(),
                            },
                        );
                    }
                }
                WorkerEvent::NeedApproval(req) => self.pending_approval = Some(req),
                WorkerEvent::Error(err) => {
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            last.content.push_str(&format!("\n\n[Error: {}]", err));
                        }
                    }
                }
                WorkerEvent::Done => {
                    self.busy = false;
                    self.pending_approval = None;
                    save_state(&self.state);
                }
            }
        }

        // Global keyboard shortcuts + Ctrl+V clipboard image paste
        let (k_new, k_side, k_pal, k_esc, k_paste) = ctx.input(|i| {
            let cmd = i.modifiers.command || i.modifiers.ctrl;
            (
                cmd && i.key_pressed(egui::Key::N),
                cmd && i.key_pressed(egui::Key::B),
                cmd && i.key_pressed(egui::Key::K),
                i.key_pressed(egui::Key::Escape),
                cmd && i.key_pressed(egui::Key::V),
            )
        });
        if k_new && !self.busy {
            self.new_chat();
        }
        if k_side {
            self.show_sidebar = !self.show_sidebar;
        }
        if k_pal {
            self.show_palette = !self.show_palette;
            self.palette_query.clear();
        }
        if k_paste {
            if let Some(att) = paste_clipboard_image(&ctx) {
                self.attachments.push(att);
                self.status_note = "Pasted image from clipboard".to_string();
            }
        }
        if k_esc {
            if self.busy {
                self.cancel_flag.store(true, Ordering::Relaxed);
            } else {
                self.show_palette = false;
                self.preview_file = None;
                self.show_sys_modal = false;
                self.show_provider_modal = false;
                self.show_mcp_modal = false;
                self.show_theme_modal = false;
                self.show_onboarding = false;
                self.renaming_chat = None;
            }
        }

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(att) = read_attachment(&ctx, f.path()) {
                self.attachments.push(att);
            }
        }

        // Sidebar
        if self.show_sidebar {
            egui::Panel::left("sidebar")
                .exact_size(290.0)
                .frame(egui::Frame::NONE.fill(th.c_sidebar()).inner_margin(14.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if let Some(tex) = &self.logo_tex {
                            ui.image((tex.id(), egui::vec2(22.0, 22.0)));
                        }
                        ui.label(RichText::new("Yokahu").strong().size(16.0).color(th.c_text()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let badge = egui::Frame::NONE
                                .fill(Color32::from_rgba_unmultiplied(63, 185, 80, 28))
                                .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(63, 185, 80, 85)))
                                .corner_radius(CornerRadius::same(12))
                                .inner_margin(egui::Margin::symmetric(8, 2));
                            badge.show(ui, |ui| {
                                ui.label(RichText::new("LOCAL").size(10.0).color(GREEN));
                            });
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Chats, "Chats");
                        ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Files, "Files");
                        ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Settings, "Settings");
                    });
                    ui.separator();
                    ui.add_space(4.0);

                    match self.sidebar_tab {
                        SidebarTab::Chats => {
                            if ui
                                .add_sized([ui.available_width(), 30.0], egui::Button::new("+ New Conversation"))
                                .clicked()
                            {
                                self.new_chat();
                            }
                            ui.add_space(6.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.history_filter)
                                    .hint_text("Search history...")
                                    .desired_width(ui.available_width()),
                            );
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                let filter = self.history_filter.to_lowercase();
                                let active_id = self.state.active_id.clone();
                                let mut switch_to: Option<String> = None;
                                let mut delete_id: Option<String> = None;
                                let mut toggle_pin: Option<String> = None;
                                let mut start_rename: Option<(String, String)> = None;
                                let mut commit_rename: Option<(String, String)> = None;

                                // Pinned chats first, then unpinned
                                let mut order: Vec<usize> = (0..self.state.chats.len()).collect();
                                order.sort_by_key(|&i| !self.state.chats[i].pinned);

                                for i in order {
                                    let c = &self.state.chats[i];
                                    if !filter.is_empty() && !c.title.to_lowercase().contains(&filter) {
                                        continue;
                                    }
                                    if let Some((ref rid, ref mut rbuf)) = self.renaming_chat {
                                        if rid == &c.id {
                                            let resp = ui.add(egui::TextEdit::singleline(rbuf).desired_width(ui.available_width()));
                                            resp.request_focus();
                                            if resp.lost_focus() || ctx.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                                                commit_rename = Some((rid.clone(), rbuf.clone()));
                                            }
                                            continue;
                                        }
                                    }
                                    let is_active = active_id.as_deref() == Some(c.id.as_str());
                                    let prefix = if c.pinned { "* " } else { "" };
                                    ui.horizontal(|ui| {
                                        if ui
                                            .selectable_label(is_active, format!("{}{}", prefix, truncate(&c.title, 20)))
                                            .clicked()
                                        {
                                            switch_to = Some(c.id.clone());
                                        }
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if icons::delete_button(ui).clicked() {
                                                delete_id = Some(c.id.clone());
                                            }
                                            if icons::edit_button(ui).clicked() {
                                                start_rename = Some((c.id.clone(), c.title.clone()));
                                            }
                                            if icons::pin_button(ui, c.pinned, th.c_accent()).clicked() {
                                                toggle_pin = Some(c.id.clone());
                                            }
                                        });
                                    });
                                }

                                if let Some(id) = switch_to {
                                    self.state.active_id = Some(id);
                                    save_state(&self.state);
                                }
                                if let Some(id) = toggle_pin {
                                    if let Some(c) = self.state.chats.iter_mut().find(|x| x.id == id) {
                                        c.pinned = !c.pinned;
                                    }
                                    save_state(&self.state);
                                }
                                if let Some(pair) = start_rename {
                                    self.renaming_chat = Some(pair);
                                }
                                if let Some((id, new_title)) = commit_rename {
                                    if !new_title.trim().is_empty() {
                                        if let Some(c) = self.state.chats.iter_mut().find(|x| x.id == id) {
                                            c.title = new_title.trim().to_string();
                                        }
                                        save_state(&self.state);
                                    }
                                    self.renaming_chat = None;
                                }
                                if let Some(id) = delete_id {
                                    self.state.chats.retain(|c| c.id != id);
                                    if self.state.chats.is_empty() {
                                        self.new_chat();
                                    } else if self.state.active_id.as_deref() == Some(id.as_str()) {
                                        self.state.active_id = Some(self.state.chats[0].id.clone());
                                    }
                                    save_state(&self.state);
                                }
                            });
                        }
                        SidebarTab::Files => {
                            ui.horizontal(|ui| {
                                if ui.button("Select Folder...").clicked() {
                                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                        self.workspace_tree = tools::list_workspace_tree(&dir);
                                        self.state.workspace = Some(dir.display().to_string());
                                        save_state(&self.state);
                                    }
                                }
                                if self.state.workspace.is_some() {
                                    if ui.button("Refresh").clicked() {
                                        if let Some(w) = &self.state.workspace {
                                            self.workspace_tree = tools::list_workspace_tree(Path::new(w));
                                        }
                                    }
                                    if ui.button("Clear").clicked() {
                                        self.state.workspace = None;
                                        self.workspace_tree.clear();
                                        save_state(&self.state);
                                    }
                                }
                            });
                            let ws_label = self.state.workspace.as_deref().unwrap_or("No workspace selected");
                            ui.label(RichText::new(truncate(ws_label, 36)).size(11.0).color(th.c_muted()));
                            if let Some(ws) = &self.state.workspace {
                                if ui.small_button("Edit .yokahu.md").clicked() {
                                    let mem = Path::new(ws).join(".yokahu.md");
                                    if !mem.exists() {
                                        let _ = fs::write(&mem, "# Project Memory\n\nAdd workspace conventions and context here.\n");
                                    }
                                    let body = safe_read_preview(&mem).unwrap_or_default();
                                    self.preview_file = Some((".yokahu.md".to_string(), mem, body));
                                }
                            }
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for entry in &self.workspace_tree {
                                    let indent = "  ".repeat(entry.depth);
                                    let label = if entry.is_dir {
                                        format!("{}{}/", indent, entry.name)
                                    } else {
                                        format!("{}{}", indent, entry.name)
                                    };
                                    if entry.is_dir {
                                        ui.label(RichText::new(label).size(12.0).color(th.c_muted()));
                                    } else {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .selectable_label(false, RichText::new(label).size(12.0).color(th.c_text()))
                                                .clicked()
                                            {
                                                let body = match safe_read_preview(&entry.path) {
                                                    Ok(text) => text,
                                                    Err(msg) => msg,
                                                };
                                                self.preview_file = Some((entry.name.clone(), entry.path.clone(), body));
                                            }
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if icons::plus_button(ui, "Attach to chat").clicked() {
                                                    if let Some(att) = read_attachment(&ctx, &entry.path) {
                                                        self.attachments.push(att);
                                                    } else {
                                                        self.status_note = "File too large or binary".to_string();
                                                    }
                                                }
                                            });
                                        });
                                    }
                                }
                            });
                        }
                        SidebarTab::Settings => {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                ui.label(RichText::new("Model & Provider").size(12.0).strong().color(th.c_text()));
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(egui::TextEdit::singleline(&mut self.state.model).desired_width(155.0))
                                        .changed()
                                    {
                                        save_state(&self.state);
                                    }
                                    egui::ComboBox::from_id_salt("model_pick")
                                        .selected_text("Pick")
                                        .width(68.0)
                                        .show_ui(ui, |ui| {
                                            for m in &self.models {
                                                if ui.selectable_value(&mut self.state.model, m.clone(), m).changed() {
                                                    save_state(&self.state);
                                                }
                                            }
                                        });
                                });
                                if ui.button("Provider & Pull Model...").clicked() {
                                    self.show_provider_modal = true;
                                }
                                ui.add_space(8.0);
                                ui.separator();

                                ui.label(RichText::new("Agent Safety").size(12.0).strong().color(th.c_text()));
                                egui::ComboBox::from_id_salt("safety_combo")
                                    .selected_text(self.state.safety_mode.label())
                                    .show_ui(ui, |ui| {
                                        for mode in [SafetyMode::Safe, SafetyMode::ReadOnly, SafetyMode::AutoApprove] {
                                            if ui.selectable_value(&mut self.state.safety_mode, mode, mode.label()).changed() {
                                                save_state(&self.state);
                                            }
                                        }
                                    });
                                ui.add_space(8.0);
                                ui.separator();

                                ui.label(RichText::new("Generation").size(12.0).strong().color(th.c_text()));
                                if ui.checkbox(&mut self.state.think, "Thinking mode").changed() {
                                    save_state(&self.state);
                                }
                                ui.label(RichText::new(format!("Temperature: {:.1}", self.state.temperature)).size(11.5).color(th.c_muted()));
                                if ui
                                    .add(egui::Slider::new(&mut self.state.temperature, 0.0..=1.0).show_value(false))
                                    .changed()
                                {
                                    save_state(&self.state);
                                }
                                ui.label(RichText::new(format!("Context (num_ctx): {}", self.state.num_ctx)).size(11.5).color(th.c_muted()));
                                if ui
                                    .add(egui::Slider::new(&mut self.state.num_ctx, 2048..=32768).step_by(1024.0).show_value(false))
                                    .changed()
                                {
                                    save_state(&self.state);
                                }
                                ui.add_space(8.0);
                                ui.separator();

                                ui.label(RichText::new("Personas").size(12.0).strong().color(th.c_text()));
                                let mut picked_persona: Option<Persona> = None;
                                egui::ComboBox::from_id_salt("persona_combo")
                                    .selected_text(&self.state.active_persona)
                                    .show_ui(ui, |ui| {
                                        for p in &self.state.personas {
                                            if ui.selectable_label(self.state.active_persona == p.name, &p.name).clicked() {
                                                picked_persona = Some(p.clone());
                                            }
                                        }
                                    });
                                if let Some(p) = picked_persona {
                                    self.state.active_persona = p.name;
                                    self.state.system_prompt = p.prompt;
                                    save_state(&self.state);
                                }
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.new_persona_name).hint_text("custom name").desired_width(120.0));
                                    if ui.button("Save").clicked() && !self.new_persona_name.trim().is_empty() {
                                        let name = self.new_persona_name.trim().to_string();
                                        let prompt = self.state.system_prompt.clone();
                                        self.state.personas.retain(|p| p.name != name);
                                        self.state.personas.push(Persona { name: name.clone(), prompt, builtin: false });
                                        self.state.active_persona = name;
                                        self.new_persona_name.clear();
                                        save_state(&self.state);
                                    }
                                    let is_custom = self.state.personas.iter().any(|p| p.name == self.state.active_persona && !p.builtin);
                                    if is_custom && ui.button("Del").clicked() {
                                        let cur = self.state.active_persona.clone();
                                        self.state.personas.retain(|p| p.name != cur);
                                        self.state.active_persona = "General".to_string();
                                        save_state(&self.state);
                                    }
                                });
                                ui.add_space(8.0);
                                ui.separator();

                                ui.label(RichText::new("Integrations & UI").size(12.0).strong().color(th.c_text()));
                                if ui.checkbox(&mut self.state.compare_enabled, "Compare 2 models side-by-side").changed() {
                                    save_state(&self.state);
                                }
                                if self.state.compare_enabled {
                                    if ui
                                        .add(egui::TextEdit::singleline(&mut self.state.compare_model).hint_text("2nd model").desired_width(ui.available_width()))
                                        .changed()
                                    {
                                        save_state(&self.state);
                                    }
                                }
                                ui.label(RichText::new("Whisper CLI (optional STT):").size(11.0).color(th.c_muted()));
                                if ui
                                    .add(egui::TextEdit::singleline(&mut self.state.whisper_cli).desired_width(ui.available_width()))
                                    .changed()
                                {
                                    save_state(&self.state);
                                }
                                ui.add_space(4.0);
                                if ui.add_sized([ui.available_width(), 28.0], egui::Button::new(format!("MCP Servers ({} tools)...", self.mcp_tools.len()))).clicked() {
                                    self.show_mcp_modal = true;
                                }
                                if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Restart MCP Processes")).clicked() {
                                    self.mcp_pool.restart_all();
                                    self.status_note = "MCP processes restarted".to_string();
                                }
                                if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("System Prompt...")).clicked() {
                                    self.sys_draft = self.state.system_prompt.clone();
                                    self.show_sys_modal = true;
                                }
                                if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Appearance & Colors...")).clicked() {
                                    self.show_theme_modal = true;
                                }
                            });
                        }
                    }
                });
        }

        // Topbar
        egui::Panel::top("topbar")
            .exact_size(46.0)
            .frame(egui::Frame::NONE.fill(th.c_root()).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(if self.show_sidebar { "Hide" } else { "Sidebar" }).corner_radius(CornerRadius::same(16)))
                        .clicked()
                    {
                        self.show_sidebar = !self.show_sidebar;
                    }
                    ui.add_space(6.0);
                    let title = self
                        .state
                        .chats
                        .get(self.active_chat_idx())
                        .map(|c| c.title.as_str())
                        .unwrap_or("New Conversation");
                    ui.label(RichText::new(title).size(14.0).color(th.c_muted()));
                    if !self.status_note.is_empty() {
                        ui.label(RichText::new(format!("  |  {}", self.status_note)).size(11.5).color(GREEN));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (dot_col, status_txt) = if self.endpoint_ok {
                            (GREEN, format!("{} Ready", self.state.provider.label()))
                        } else {
                            (AMBER, format!("{} Offline", self.state.provider.label()))
                        };
                        ui.label(RichText::new(status_txt).size(12.0).color(th.c_muted()));
                        icons::status_dot(ui, dot_col);
                        if ui.small_button("Export .md").clicked() {
                            self.export_active_chat(false);
                        }
                        if ui.small_button("Export .json").clicked() {
                            self.export_active_chat(true);
                        }
                    });
                });
            });

        // Composer
        egui::Panel::bottom("composer")
            .frame(egui::Frame::NONE.fill(th.c_root()).inner_margin(egui::Margin::symmetric(20, 14)))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(760.0);

                    // @filename autocomplete popup when workspace is active
                    if !self.workspace_tree.is_empty() {
                        if let Some(at_pos) = self.prompt_input.rfind('@') {
                            let after_at = &self.prompt_input[at_pos + 1..];
                            if !after_at.contains(' ') && !after_at.contains('\n') {
                                let q = after_at.to_lowercase();
                                let matches: Vec<WorkspaceEntry> = self
                                    .workspace_tree
                                    .iter()
                                    .filter(|e| !e.is_dir && (q.is_empty() || e.name.to_lowercase().contains(&q)))
                                    .take(6)
                                    .cloned()
                                    .collect();
                                if !matches.is_empty() {
                                    let ac_frame = egui::Frame::NONE
                                        .fill(th.c_elevated())
                                        .stroke(Stroke::new(1.0, th.c_border()))
                                        .corner_radius(CornerRadius::same(10))
                                        .inner_margin(8.0);
                                    ac_frame.show(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(RichText::new("Attach @file:").size(11.0).color(th.c_muted()));
                                            for m in matches {
                                                if ui.small_button(&m.name).clicked() {
                                                    if let Some(att) = read_attachment(&ctx, &m.path) {
                                                        self.attachments.push(att);
                                                    }
                                                    self.prompt_input.truncate(at_pos);
                                                    self.prompt_input.push_str(&m.name);
                                                    self.prompt_input.push(' ');
                                                }
                                            }
                                        });
                                    });
                                    ui.add_space(4.0);
                                }
                            }
                        }
                    }

                    let frame = egui::Frame::NONE
                        .fill(th.c_surface())
                        .stroke(Stroke::new(1.0, th.c_border()))
                        .corner_radius(CornerRadius::same(16))
                        .inner_margin(12.0);
                    frame.show(ui, |ui| {
                        if !self.attachments.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                let mut rem: Option<usize> = None;
                                for (i, att) in self.attachments.iter().enumerate() {
                                    if let Some(tex) = &att.thumb {
                                        ui.image((tex.id(), egui::vec2(36.0, 36.0)));
                                    }
                                    ui.label(RichText::new(&att.name).size(11.5).color(th.c_accent()));
                                    if ui.small_button("x").clicked() {
                                        rem = Some(i);
                                    }
                                }
                                if let Some(i) = rem {
                                    self.attachments.remove(i);
                                }
                            });
                            ui.add_space(4.0);
                        }

                        let send_pressed = ctx.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                        let te = ui.add(
                            egui::TextEdit::multiline(&mut self.prompt_input)
                                .hint_text(format!(
                                    "Ask {} anything... (@file, Ctrl+V image, Ctrl+K palette)",
                                    self.state.model
                                ))
                                .desired_rows(2)
                                .desired_width(ui.available_width())
                                .frame(egui::Frame::NONE),
                        );
                        if te.has_focus() && send_pressed && !self.busy {
                            if self.prompt_input.ends_with('\n') {
                                self.prompt_input.pop();
                            }
                            self.send_message(&ctx);
                        }

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("Attach...").clicked() {
                                if let Some(files) = rfd::FileDialog::new().pick_files() {
                                    for p in files {
                                        if let Some(att) = read_attachment(&ctx, &p) {
                                            self.attachments.push(att);
                                        }
                                    }
                                }
                            }
                            if ui.button("Voice...").on_hover_text("Transcribe audio file via Whisper CLI").clicked() {
                                if let Some(audio) = rfd::FileDialog::new()
                                    .add_filter("Audio", &["wav", "mp3", "m4a", "ogg", "flac"])
                                    .pick_file()
                                {
                                    let cli = self.state.whisper_cli.clone();
                                    let tx = self.tx.clone();
                                    let ctx_c = ctx.clone();
                                    self.status_note = "Transcribing audio...".to_string();
                                    thread::spawn(move || transcribe_audio(&cli, &audio, &tx, &ctx_c));
                                }
                            }
                            #[cfg(windows)]
                            {
                                let mic_btn = if self.recording_mic {
                                    egui::Button::new(RichText::new("Stop Rec").color(Color32::WHITE)).fill(RED)
                                } else {
                                    egui::Button::new("Mic")
                                };
                                if ui.add(mic_btn).on_hover_text("Record live microphone & transcribe via Whisper").clicked() {
                                    if !self.recording_mic {
                                        if start_mic_recording() {
                                            self.recording_mic = true;
                                            self.status_note = "Recording mic...".to_string();
                                        } else {
                                            self.status_note = "Mic record failed".to_string();
                                        }
                                    } else {
                                        self.recording_mic = false;
                                        if let Some(wav) = stop_mic_recording() {
                                            let cli = self.state.whisper_cli.clone();
                                            let tx = self.tx.clone();
                                            let ctx_c = ctx.clone();
                                            self.status_note = "Transcribing recording...".to_string();
                                            thread::spawn(move || transcribe_audio(&cli, &wav, &tx, &ctx_c));
                                        }
                                    }
                                }
                            }
                            let est_chars: usize = self
                                .state
                                .chats
                                .get(self.active_chat_idx())
                                .map(|c| c.messages.iter().map(|m| m.content.len() + m.thinking.len()).sum())
                                .unwrap_or(0);
                            let est_toks = (est_chars / 4).max(1);
                            let speed_str = if self.last_tok_per_sec > 0.0 {
                                format!("{:.1} tok/s  |  ~{}/{} ctx", self.last_tok_per_sec, est_toks, self.state.num_ctx)
                            } else {
                                format!("~{}/{} ctx", est_toks, self.state.num_ctx)
                            };
                            ui.label(RichText::new(speed_str).size(11.0).color(th.c_muted()));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if self.busy {
                                    if ui
                                        .add(egui::Button::new(RichText::new("Stop").color(Color32::WHITE)).fill(RED))
                                        .clicked()
                                    {
                                        self.cancel_flag.store(true, Ordering::Relaxed);
                                    }
                                } else if ui
                                    .add(
                                        egui::Button::new(RichText::new("Send").color(Color32::WHITE).strong())
                                            .fill(th.c_accent())
                                            .corner_radius(CornerRadius::same(16)),
                                    )
                                    .clicked()
                                {
                                    self.send_message(&ctx);
                                }
                            });
                        });
                    });
                });
            });

        // Main Chat Area
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(th.c_root()).inner_margin(egui::Margin::symmetric(24, 12)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.set_max_width(760.0);
                            let chat = &self.state.chats[self.active_chat_idx()];
                            if chat.messages.is_empty() {
                                ui.add_space(40.0);
                                ui.vertical_centered(|ui| {
                                    if let Some(tex) = &self.logo_tex {
                                        ui.image((tex.id(), egui::vec2(140.0, 140.0)));
                                        ui.add_space(8.0);
                                    }
                                    ui.label(RichText::new("Yokahu Local AI Workspace").size(22.0).strong().color(th.c_text()));
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new(format!(
                                            "Model: {}  |  Provider: {}  |  MCP Tools: {}",
                                            self.state.model,
                                            self.state.provider.label(),
                                            self.mcp_tools.len()
                                        ))
                                        .size(13.0)
                                        .color(th.c_muted()),
                                    );
                                });
                            } else {
                                let msgs_snapshot = chat.messages.clone();
                                let mut edit_user_idx: Option<usize> = None;
                                let mut regen_idx: Option<usize> = None;
                                let mut fork_idx: Option<usize> = None;
                                let mut apply_code_req: Option<String> = None;

                                for (m_idx, msg) in msgs_snapshot.iter().enumerate() {
                                    if msg.role == "user" {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                            let bubble = egui::Frame::NONE
                                                .fill(th.c_elevated())
                                                .stroke(Stroke::new(1.0, th.c_border()))
                                                .corner_radius(CornerRadius::same(12))
                                                .inner_margin(12.0);
                                            bubble.show(ui, |ui| {
                                                ui.set_max_width(ui.available_width() * 0.78);
                                                ui.label(RichText::new(&msg.content).color(th.c_text()));
                                            });
                                        });
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                            if ui.small_button("Edit").clicked() && !self.busy {
                                                edit_user_idx = Some(m_idx);
                                            }
                                            if ui.small_button("Copy").clicked() {
                                                ctx.copy_text(msg.content.clone());
                                            }
                                        });
                                    } else if msg.role == "tool" {
                                        egui::CollapsingHeader::new(
                                            RichText::new(format!("Tool result: {}", msg.tool_name))
                                                .size(11.5)
                                                .color(th.c_muted()),
                                        )
                                        .id_salt(format!("tool_{}", m_idx))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(&msg.content)
                                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                                    .color(th.c_muted()),
                                            );
                                        });
                                    } else {
                                        let model_lbl = if msg.model.is_empty() { &self.state.model } else { &msg.model };
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(model_lbl).size(11.0).color(th.c_muted()));
                                            if icons::speaker_button(ui).clicked() {
                                                speak_text(&msg.content);
                                            }
                                            if !self.busy {
                                                if ui.small_button("Copy").clicked() {
                                                    ctx.copy_text(msg.content.clone());
                                                }
                                                if ui.small_button("Fork").clicked() {
                                                    fork_idx = Some(m_idx);
                                                }
                                                if ui.small_button("Regenerate").clicked() {
                                                    regen_idx = Some(m_idx);
                                                }
                                            }
                                        });
                                        if !msg.thinking.is_empty() {
                                            egui::CollapsingHeader::new(
                                                RichText::new("Thought process").size(12.0).color(th.c_muted()),
                                            )
                                            .id_salt(format!("think_{}", m_idx))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(&msg.thinking)
                                                        .font(FontId::new(12.0, FontFamily::Monospace))
                                                        .color(th.c_muted()),
                                                );
                                            });
                                        }
                                        if !msg.compare_model.is_empty() {
                                            ui.columns(2, |cols| {
                                                cols[0].label(RichText::new(model_lbl).size(11.5).strong().color(th.c_accent()));
                                                if let Some(code) = markdown::render_markdown(&mut cols[0], &ctx, &th, &msg.content) {
                                                    apply_code_req = Some(code);
                                                }
                                                cols[1].label(RichText::new(&msg.compare_model).size(11.5).strong().color(GREEN));
                                                if let Some(code) = markdown::render_markdown(&mut cols[1], &ctx, &th, &msg.compare_content) {
                                                    apply_code_req = Some(code);
                                                }
                                            });
                                        } else if let Some(code) = markdown::render_markdown(ui, &ctx, &th, &msg.content) {
                                            apply_code_req = Some(code);
                                        }
                                    }
                                    ui.add_space(12.0);
                                }

                                if let Some(u_idx) = edit_user_idx {
                                    let c_idx = self.active_chat_idx();
                                    self.prompt_input = self.state.chats[c_idx].messages[u_idx].content.clone();
                                    self.state.chats[c_idx].messages.truncate(u_idx);
                                    save_state(&self.state);
                                }
                                if let Some(r_idx) = regen_idx {
                                    self.regenerate_at(&ctx, r_idx);
                                }
                                if let Some(f_idx) = fork_idx {
                                    self.fork_at(f_idx);
                                }
                                if let Some(code) = apply_code_req {
                                    let mut dlg = rfd::FileDialog::new();
                                    if let Some(ws) = &self.state.workspace {
                                        dlg = dlg.set_directory(ws);
                                    }
                                    if let Some(path) = dlg.save_file() {
                                        match fs::write(&path, &code) {
                                            Ok(_) => self.status_note = format!("Applied to {}", path.display()),
                                            Err(e) => self.status_note = format!("Apply failed: {}", e),
                                        }
                                    }
                                }
                            }

                            if let Some(pending) = self.pending_approval.take() {
                                let card = egui::Frame::NONE
                                    .fill(th.c_surface())
                                    .stroke(Stroke::new(1.5, AMBER))
                                    .corner_radius(CornerRadius::same(12))
                                    .inner_margin(12.0);
                                let mut decided = false;
                                card.show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!("Approval required: {}", pending.tool_name))
                                            .strong()
                                            .color(AMBER),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(&pending.preview)
                                            .font(FontId::new(12.0, FontFamily::Monospace))
                                            .color(th.c_muted()),
                                    );
                                    if !pending.diff_lines.is_empty() {
                                        ui.add_space(6.0);
                                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                            for (kind, line) in &pending.diff_lines {
                                                let col = match kind {
                                                    '+' => GREEN,
                                                    '-' => RED,
                                                    _ => th.c_muted(),
                                                };
                                                ui.label(
                                                    RichText::new(format!("{} {}", kind, line))
                                                        .font(FontId::new(12.0, FontFamily::Monospace))
                                                        .color(col),
                                                );
                                            }
                                        });
                                    }
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(egui::Button::new(RichText::new("Approve").color(th.c_root()).strong()).fill(GREEN))
                                            .clicked()
                                        {
                                            let _ = pending.reply_tx.send(true);
                                            decided = true;
                                        }
                                        if ui
                                            .add(egui::Button::new(RichText::new("Deny").color(Color32::WHITE)).fill(RED))
                                            .clicked()
                                        {
                                            let _ = pending.reply_tx.send(false);
                                            decided = true;
                                        }
                                    });
                                });
                                if !decided {
                                    self.pending_approval = Some(pending);
                                }
                            }
                        });
                    });
            });

        modals::render_modals(self, &ctx, &th);
    }
}

pub fn state_path() -> PathBuf {
    mcp_client::app_data_dir().join("state.json")
}

pub fn load_state() -> PersistedState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &PersistedState) {
    let p = state_path();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = fs::write(p, json);
    }
}

pub fn safe_read_preview(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| format!("Cannot stat file: {}", e))?;
    if meta.len() > 256 * 1024 {
        return Err(format!(
            "File too large ({:.1} KB > 256 KB limit) to preview safely.",
            meta.len() as f64 / 1024.0
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("Cannot read file: {}", e))?;
    if bytes.iter().take(4096).any(|&b| b == 0) {
        return Err("Binary file - preview disabled.".to_string());
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().take(2000).collect();
    if text.lines().count() > 2000 {
        lines.push("\n... (truncated at 2000 lines)");
    }
    Ok(lines.join("\n"))
}

pub fn read_attachment(ctx: &egui::Context, path: &Path) -> Option<Attachment> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif");
    if is_image {
        let bytes = fs::read(path).ok()?;
        let thumb = image::load_from_memory(&bytes).ok().map(|img| {
            let rgba = img.thumbnail(96, 96).to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            ctx.load_texture(format!("att_{}", name), color_img, egui::TextureOptions::LINEAR)
        });
        Some(Attachment {
            name,
            is_image: true,
            data: encode_base64(&bytes),
            thumb,
        })
    } else {
        let meta = fs::metadata(path).ok()?;
        if meta.len() > 512 * 1024 {
            return None;
        }
        let text = fs::read_to_string(path).ok()?;
        Some(Attachment {
            name,
            is_image: false,
            data: text,
            thumb: None,
        })
    }
}

fn paste_clipboard_image(ctx: &egui::Context) -> Option<Attachment> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    let rgba = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())?;
    let mut png_bytes = Vec::new();
    let dyn_img = image::DynamicImage::ImageRgba8(rgba.clone());
    dyn_img
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .ok()?;
    let thumb_img = dyn_img.thumbnail(96, 96).to_rgba8();
    let size = [thumb_img.width() as usize, thumb_img.height() as usize];
    let color_img = egui::ColorImage::from_rgba_unmultiplied(size, thumb_img.as_raw());
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let name = format!("clipboard_{}.png", ts);
    let tex = ctx.load_texture(format!("att_{}", name), color_img, egui::TextureOptions::LINEAR);
    Some(Attachment {
        name,
        is_image: true,
        data: encode_base64(&png_bytes),
        thumb: Some(tex),
    })
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

fn sanitize_filename(s: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if clean.is_empty() { "yokahu_chat".to_string() } else { clean }
}

fn speak_text(text: &str) {
    let clean: String = text.chars().take(1500).collect();
    thread::spawn(move || {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let escaped = clean.replace('\'', "''");
            let script = format!(
                "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Speak('{}')",
                escaped
            );
            let mut cmd = std::process::Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", &script]);
            cmd.creation_flags(0x08000000);
            let _ = cmd.output();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("say").arg(&clean).output();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("spd-say").arg(&clean).output();
        }
    });
}

fn transcribe_audio(cli: &str, audio: &Path, tx: &Sender<WorkerEvent>, ctx: &egui::Context) {
    let out_dir = std::env::temp_dir().join("yokahu_whisper");
    let _ = fs::create_dir_all(&out_dir);
    let res = std::process::Command::new(cli.trim())
        .arg(audio)
        .args(["--output_format", "txt", "--output_dir"])
        .arg(&out_dir)
        .output();
    match res {
        Ok(o) if o.status.success() => {
            let stem = audio.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
            let txt_path = out_dir.join(format!("{}.txt", stem));
            let text = fs::read_to_string(&txt_path)
                .unwrap_or_else(|_| String::from_utf8_lossy(&o.stdout).to_string());
            let _ = tx.send(WorkerEvent::Transcript(text.trim().to_string()));
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let _ = tx.send(WorkerEvent::Error(format!("Whisper failed: {}", err.trim())));
        }
        Err(e) => {
            let _ = tx.send(WorkerEvent::Error(format!("Cannot run '{}': {}", cli, e)));
        }
    }
    ctx.request_repaint();
}

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn mciSendStringA(lpszCommand: *const u8, lpszReturnString: *mut u8, cchReturn: u32, hwndCallback: usize) -> u32;
}

#[cfg(windows)]
fn mci_cmd(cmd: &str) -> bool {
    let mut buf = cmd.as_bytes().to_vec();
    buf.push(0);
    unsafe { mciSendStringA(buf.as_ptr(), std::ptr::null_mut(), 0, 0) == 0 }
}

#[cfg(windows)]
fn start_mic_recording() -> bool {
    let _ = mci_cmd("close yokahu_mic");
    mci_cmd("open new Type waveaudio Alias yokahu_mic") && mci_cmd("record yokahu_mic")
}

#[cfg(windows)]
fn stop_mic_recording() -> Option<PathBuf> {
    let path = std::env::temp_dir().join("yokahu_mic.wav");
    let _ = fs::remove_file(&path);
    let _ = mci_cmd("stop yokahu_mic");
    let ok = mci_cmd(&format!("save yokahu_mic \"{}\"", path.display()));
    let _ = mci_cmd("close yokahu_mic");
    if ok && path.exists() { Some(path) } else { None }
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, RichText, Stroke};
use yokahu::mcp_client::{self, DiscoveredMcpTool, McpConfigFile, McpServerConfig};
use yokahu::ollama;
use yokahu::tools::{self, SafetyMode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BG_ROOT: Color32 = Color32::from_rgb(11, 14, 20);
const BG_SIDEBAR: Color32 = Color32::from_rgb(16, 20, 29);
const BG_SURFACE: Color32 = Color32::from_rgb(21, 26, 38);
const BG_ELEVATED: Color32 = Color32::from_rgb(28, 35, 51);
const BORDER: Color32 = Color32::from_rgb(37, 46, 66);
const TEXT: Color32 = Color32::from_rgb(230, 237, 243);
const TEXT_MUTED: Color32 = Color32::from_rgb(139, 150, 168);
const TEXT_DIM: Color32 = Color32::from_rgb(92, 102, 122);
const ACCENT: Color32 = Color32::from_rgb(88, 166, 255);
const GREEN: Color32 = Color32::from_rgb(63, 185, 80);
const RED: Color32 = Color32::from_rgb(248, 81, 73);
const AMBER: Color32 = Color32::from_rgb(210, 153, 34);

const ICON_PNG: &[u8] = include_bytes!("../../assets/icon.png");
const LOGO_JPG: &[u8] = include_bytes!("../../assets/logo.jpg");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ProviderKind {
    Ollama,
    OpenAiCompat,
}

impl Default for ProviderKind {
    fn default() -> Self {
        ProviderKind::Ollama
    }
}

impl ProviderKind {
    fn label(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "Ollama",
            ProviderKind::OpenAiCompat => "OpenAI-Compatible",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Attachment {
    name: String,
    is_image: bool,
    data: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
    #[serde(default)]
    thinking: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    images: Vec<String>,
    #[serde(default)]
    tool_name: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Conversation {
    id: String,
    title: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct PersistedState {
    chats: Vec<Conversation>,
    active_id: Option<String>,
    workspace: Option<String>,
    provider: ProviderKind,
    base_url: String,
    api_key: String,
    model: String,
    think: bool,
    temperature: f32,
    num_ctx: i64,
    keep_alive: String,
    safety_mode: SafetyMode,
    system_prompt: String,
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
        }
    }
}

struct PendingApproval {
    tool_name: String,
    preview: String,
    reply_tx: Sender<bool>,
}

enum WorkerEvent {
    Models(Vec<String>, bool),
    McpTools(Vec<DiscoveredMcpTool>, Vec<String>),
    PullStatus(String, bool),
    Thinking(String),
    Token(String),
    ToolStart(String),
    ToolResult { name: String, output: String },
    NeedApproval(PendingApproval),
    Error(String),
    Done,
}

struct YokahuApp {
    state: PersistedState,
    models: Vec<String>,
    endpoint_ok: bool,
    history_filter: String,
    prompt_input: String,
    attachments: Vec<Attachment>,
    busy: bool,
    cancel_flag: Arc<AtomicBool>,
    rx: Receiver<WorkerEvent>,
    tx: Sender<WorkerEvent>,
    pending_approval: Option<PendingApproval>,
    show_sys_modal: bool,
    sys_draft: String,
    show_provider_modal: bool,
    pull_model_input: String,
    pull_status: String,
    show_mcp_modal: bool,
    mcp_cfg: McpConfigFile,
    mcp_tools: Vec<DiscoveredMcpTool>,
    mcp_errors: Vec<String>,
    new_mcp_name: String,
    new_mcp_cmd: String,
    new_mcp_args: String,
    new_mcp_env: String,
    new_mcp_url: String,
    new_mcp_is_http: bool,
    scaffold_name: String,
    scaffold_lang: String,
    mcp_status_msg: String,
    show_onboarding: bool,
    logo_tex: Option<egui::TextureHandle>,
    status_note: String,
}

impl YokahuApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);
        let state = load_state();
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
            fetch_models(prov, &url, &key, &tx_boot, &ctx_boot);
            let (tools, errs) = mcp_client::discover_tools(&mcp_boot);
            let _ = tx_boot.send(WorkerEvent::McpTools(tools, errs));
            ctx_boot.request_repaint();
        });

        let logo_tex = image::load_from_memory(LOGO_JPG).ok().map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            cc.egui_ctx.load_texture("yokahu_logo", color_img, egui::TextureOptions::LINEAR)
        });

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
            logo_tex,
            status_note: String::new(),
        };
        if app.state.chats.is_empty() {
            app.new_chat();
        }
        app
    }

    fn new_chat(&mut self) {
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
                messages: Vec::new(),
            },
        );
        self.state.active_id = Some(id);
        save_state(&self.state);
    }

    fn active_chat_idx(&self) -> usize {
        if let Some(id) = &self.state.active_id {
            if let Some(pos) = self.state.chats.iter().position(|c| &c.id == id) {
                return pos;
            }
        }
        0
    }

    fn refresh_models(&self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx_c = ctx.clone();
        let prov = self.state.provider;
        let url = self.state.base_url.clone();
        let key = self.state.api_key.clone();
        thread::spawn(move || fetch_models(prov, &url, &key, &tx, &ctx_c));
    }

    fn refresh_mcp(&self, ctx: &egui::Context) {
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
        });
        self.state.chats[idx].messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: String::new(),
            thinking: String::new(),
            model: self.state.model.clone(),
            images: Vec::new(),
            tool_name: String::new(),
        });

        self.prompt_input.clear();
        self.attachments.clear();
        self.busy = true;
        self.cancel_flag.store(false, Ordering::Relaxed);
        save_state(&self.state);

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
        let tx = self.tx.clone();
        let cancel = self.cancel_flag.clone();
        let ctx_clone = ctx.clone();

        thread::spawn(move || {
            run_agent_loop(
                history, prov, base_url, api_key, model, system, think, temp, num_ctx, keep_alive,
                workspace, safety, mcp_cfg, mcp_tools, tx, cancel, ctx_clone,
            );
        });
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
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            last.thinking.push_str(&chunk);
                        }
                    }
                }
                WorkerEvent::Token(chunk) => {
                    if !self.state.chats.is_empty() {
                        let idx = self.active_chat_idx();
                        if let Some(last) = self.state.chats[idx].messages.last_mut() {
                            last.content.push_str(&chunk);
                        }
                    }
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

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(att) = read_attachment(f.path()) {
                self.attachments.push(att);
            }
        }

        // Sidebar
        egui::Panel::left("sidebar")
            .exact_size(285.0)
            .frame(egui::Frame::NONE.fill(BG_SIDEBAR).inner_margin(12.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("YOKAHU").strong().size(16.0).color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let badge = egui::Frame::NONE
                            .fill(Color32::from_rgba_unmultiplied(63, 185, 80, 30))
                            .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(63, 185, 80, 90)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::symmetric(8, 2));
                        badge.show(ui, |ui| {
                            ui.label(RichText::new("LOCAL").size(10.5).color(GREEN));
                        });
                    });
                });
                ui.add_space(8.0);

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

                let list_h = (ui.available_height() - 345.0).max(80.0);
                egui::ScrollArea::vertical().max_height(list_h).show(ui, |ui| {
                    let filter = self.history_filter.to_lowercase();
                    let active_id = self.state.active_id.clone();
                    let mut switch_to: Option<String> = None;
                    let mut delete_id: Option<String> = None;

                    for c in &self.state.chats {
                        if !filter.is_empty() && !c.title.to_lowercase().contains(&filter) {
                            continue;
                        }
                        let is_active = active_id.as_deref() == Some(c.id.as_str());
                        ui.horizontal(|ui| {
                            if ui.selectable_label(is_active, truncate(&c.title, 26)).clicked() {
                                switch_to = Some(c.id.clone());
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("x").clicked() {
                                    delete_id = Some(c.id.clone());
                                }
                            });
                        });
                    }
                    if let Some(id) = switch_to {
                        self.state.active_id = Some(id);
                        save_state(&self.state);
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

                ui.separator();
                ui.add_space(4.0);

                // Workspace
                ui.label(RichText::new("Workspace").size(11.5).color(TEXT_MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Select Folder...").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.state.workspace = Some(dir.display().to_string());
                            save_state(&self.state);
                        }
                    }
                    if self.state.workspace.is_some() && ui.button("Clear").clicked() {
                        self.state.workspace = None;
                        save_state(&self.state);
                    }
                });
                let ws_label = self.state.workspace.as_deref().unwrap_or("None (Chat only)");
                ui.label(RichText::new(truncate(ws_label, 36)).size(11.0).color(TEXT_DIM));
                ui.add_space(4.0);

                // Safety Mode
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Safety:").size(12.0).color(TEXT_MUTED));
                    egui::ComboBox::from_id_salt("safety_combo")
                        .selected_text(self.state.safety_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in [SafetyMode::Safe, SafetyMode::ReadOnly, SafetyMode::AutoApprove] {
                                if ui.selectable_value(&mut self.state.safety_mode, mode, mode.label()).changed() {
                                    save_state(&self.state);
                                }
                            }
                        });
                });

                // Model + Provider button
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Model:").size(12.0).color(TEXT_MUTED));
                    if ui.small_button("Provider / Pull...").clicked() {
                        self.show_provider_modal = true;
                    }
                });
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

                // Thinking + Temp
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.state.think, "Thinking").changed() {
                        save_state(&self.state);
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("Temp: {:.1}", self.state.temperature)).size(11.5).color(TEXT_MUTED));
                });
                if ui
                    .add(egui::Slider::new(&mut self.state.temperature, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    save_state(&self.state);
                }

                // num_ctx + keep_alive
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Ctx: {}", self.state.num_ctx)).size(11.5).color(TEXT_MUTED));
                    if ui
                        .add(egui::Slider::new(&mut self.state.num_ctx, 2048..=32768).step_by(1024.0).show_value(false))
                        .changed()
                    {
                        save_state(&self.state);
                    }
                });

                ui.horizontal(|ui| {
                    if ui
                        .button(format!("MCP Servers ({})", self.mcp_tools.len()))
                        .clicked()
                    {
                        self.show_mcp_modal = true;
                    }
                    if ui.button("System...").clicked() {
                        self.sys_draft = self.state.system_prompt.clone();
                        self.show_sys_modal = true;
                    }
                });
            });

        // Topbar
        egui::Panel::top("topbar")
            .exact_size(42.0)
            .frame(egui::Frame::NONE.fill(BG_ROOT).inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let title = self
                        .state
                        .chats
                        .get(self.active_chat_idx())
                        .map(|c| c.title.as_str())
                        .unwrap_or("New Conversation");
                    ui.label(RichText::new(title).size(14.0).color(TEXT_MUTED));
                    if !self.status_note.is_empty() {
                        ui.label(RichText::new(format!("  |  {}", self.status_note)).size(11.5).color(GREEN));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (dot_col, status_txt) = if self.endpoint_ok {
                            (GREEN, format!("{} Ready", self.state.provider.label()))
                        } else {
                            (AMBER, format!("{} Offline", self.state.provider.label()))
                        };
                        ui.label(RichText::new(status_txt).size(12.0).color(TEXT_MUTED));
                        ui.label(RichText::new("●").size(12.0).color(dot_col));
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
            .frame(egui::Frame::NONE.fill(BG_ROOT).inner_margin(egui::Margin::symmetric(20, 12)))
            .show(ui, |ui| {
                let frame = egui::Frame::NONE
                    .fill(BG_SURFACE)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(10.0);
                frame.show(ui, |ui| {
                    if !self.attachments.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            let mut rem: Option<usize> = None;
                            for (i, att) in self.attachments.iter().enumerate() {
                                ui.label(RichText::new(&att.name).size(11.5).color(ACCENT));
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
                                "Ask {} anything... (Enter to send, Shift+Enter for newline)",
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
                        if ui.button("Attach File...").clicked() {
                            if let Some(files) = rfd::FileDialog::new().pick_files() {
                                for p in files {
                                    if let Some(att) = read_attachment(&p) {
                                        self.attachments.push(att);
                                    }
                                }
                            }
                        }
                        ui.label(RichText::new("Drag & drop supported").size(11.0).color(TEXT_DIM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.busy {
                                if ui
                                    .add(egui::Button::new(RichText::new("Stop").color(Color32::WHITE)).fill(RED))
                                    .clicked()
                                {
                                    self.cancel_flag.store(true, Ordering::Relaxed);
                                }
                            } else if ui
                                .add(egui::Button::new(RichText::new("Send").color(BG_ROOT).strong()).fill(ACCENT))
                                .clicked()
                            {
                                self.send_message(&ctx);
                            }
                        });
                    });
                });
            });

        // Main Chat Area
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG_ROOT).inner_margin(egui::Margin::symmetric(24, 12)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let chat = &self.state.chats[self.active_chat_idx()];
                        if chat.messages.is_empty() {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| {
                                if let Some(tex) = &self.logo_tex {
                                    ui.image((tex.id(), egui::vec2(150.0, 150.0)));
                                    ui.add_space(8.0);
                                }
                                ui.label(RichText::new("Yokahu Local AI Workspace").size(22.0).strong().color(TEXT));
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(format!(
                                        "Model: {}  |  Provider: {}  |  MCP Tools: {}",
                                        self.state.model,
                                        self.state.provider.label(),
                                        self.mcp_tools.len()
                                    ))
                                    .size(13.0)
                                    .color(TEXT_MUTED),
                                );
                            });
                        } else {
                            for (m_idx, msg) in chat.messages.iter().enumerate() {
                                if msg.role == "user" {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                        let bubble = egui::Frame::NONE
                                            .fill(BG_ELEVATED)
                                            .stroke(Stroke::new(1.0, BORDER))
                                            .corner_radius(CornerRadius::same(10))
                                            .inner_margin(12.0);
                                        bubble.show(ui, |ui| {
                                            ui.set_max_width(ui.available_width() * 0.78);
                                            ui.label(RichText::new(&msg.content).color(TEXT));
                                        });
                                    });
                                } else if msg.role == "tool" {
                                    egui::CollapsingHeader::new(
                                        RichText::new(format!("Tool result: {}", msg.tool_name))
                                            .size(11.5)
                                            .color(TEXT_MUTED),
                                    )
                                    .id_salt(format!("tool_{}", m_idx))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(&msg.content)
                                                .font(FontId::new(12.0, FontFamily::Monospace))
                                                .color(TEXT_MUTED),
                                        );
                                    });
                                } else {
                                    let model_lbl = if msg.model.is_empty() { &self.state.model } else { &msg.model };
                                    ui.label(RichText::new(model_lbl).size(11.0).color(TEXT_DIM));
                                    if !msg.thinking.is_empty() {
                                        egui::CollapsingHeader::new(
                                            RichText::new("Thought process").size(12.0).color(TEXT_MUTED),
                                        )
                                        .id_salt(format!("think_{}", m_idx))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(&msg.thinking)
                                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                                    .color(TEXT_MUTED),
                                            );
                                        });
                                    }
                                    render_markdown(ui, &ctx, &msg.content);
                                }
                                ui.add_space(12.0);
                            }
                        }

                        if let Some(pending) = self.pending_approval.take() {
                            let card = egui::Frame::NONE
                                .fill(BG_SURFACE)
                                .stroke(Stroke::new(1.5, AMBER))
                                .corner_radius(CornerRadius::same(10))
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
                                        .color(TEXT_MUTED),
                                );
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(egui::Button::new(RichText::new("Approve").color(BG_ROOT).strong()).fill(GREEN))
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

        // System Prompt Modal
        if self.show_sys_modal {
            egui::Window::new("System Prompt")
                .collapsible(false)
                .resizable(true)
                .default_width(480.0)
                .show(&ctx, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.sys_draft)
                            .desired_rows(6)
                            .desired_width(ui.available_width()),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.state.system_prompt = self.sys_draft.clone();
                            save_state(&self.state);
                            self.show_sys_modal = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_sys_modal = false;
                        }
                    });
                });
        }

        // Provider & Model Pull Modal
        if self.show_provider_modal {
            egui::Window::new("Model Provider & Pull")
                .collapsible(false)
                .resizable(false)
                .default_width(440.0)
                .show(&ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Provider:");
                        for p in [ProviderKind::Ollama, ProviderKind::OpenAiCompat] {
                            if ui.selectable_value(&mut self.state.provider, p, p.label()).changed() {
                                self.state.base_url = match p {
                                    ProviderKind::Ollama => "http://127.0.0.1:11434".to_string(),
                                    ProviderKind::OpenAiCompat => "http://127.0.0.1:1234/v1".to_string(),
                                };
                                save_state(&self.state);
                                self.refresh_models(&ctx);
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.label("Base URL:");
                    if ui
                        .add(egui::TextEdit::singleline(&mut self.state.base_url).desired_width(ui.available_width()))
                        .changed()
                    {
                        save_state(&self.state);
                    }
                    if self.state.provider == ProviderKind::OpenAiCompat {
                        ui.add_space(4.0);
                        ui.label("API Key (optional for local servers):");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.state.api_key)
                                    .password(true)
                                    .desired_width(ui.available_width()),
                            )
                            .changed()
                        {
                            save_state(&self.state);
                        }
                    } else {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label("Ollama VRAM Keep-Alive:");
                            egui::ComboBox::from_id_salt("ka_modal")
                                .selected_text(&self.state.keep_alive)
                                .show_ui(ui, |ui| {
                                    for ka in ["5m", "15m", "0"] {
                                        if ui
                                            .selectable_value(&mut self.state.keep_alive, ka.to_string(), ka)
                                            .changed()
                                        {
                                            save_state(&self.state);
                                        }
                                    }
                                });
                        });
                        ui.separator();
                        ui.label("Pull any Ollama model (e.g. llama3.2, qwen3.5:9b, deepseek-r1:8b):");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.pull_model_input)
                                    .hint_text("model:tag")
                                    .desired_width(240.0),
                            );
                            if ui.button("Pull Model").clicked() && !self.pull_model_input.trim().is_empty() {
                                let m = self.pull_model_input.trim().to_string();
                                let url = self.state.base_url.clone();
                                let tx = self.tx.clone();
                                let ctx_c = ctx.clone();
                                self.pull_status = format!("Pulling {}...", m);
                                thread::spawn(move || pull_ollama_model(&url, &m, &tx, &ctx_c));
                            }
                        });
                        if !self.pull_status.is_empty() {
                            ui.label(RichText::new(&self.pull_status).size(11.5).color(ACCENT));
                        }
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Refresh Models").clicked() {
                            self.refresh_models(&ctx);
                        }
                        if ui.button("Close").clicked() {
                            self.show_provider_modal = false;
                        }
                    });
                });
        }

        // MCP Servers Modal
        if self.show_mcp_modal {
            egui::Window::new("MCP Servers (Custom Tools)")
                .collapsible(false)
                .resizable(true)
                .default_width(600.0)
                .show(&ctx, |ui| {
                    ui.label(
                        RichText::new(format!("Config: {}", mcp_client::config_path().display()))
                            .size(11.0)
                            .color(TEXT_DIM),
                    );
                    ui.add_space(6.0);

                    let mut remove_name: Option<String> = None;
                    let mut changed = false;
                    for (name, scfg) in self.mcp_cfg.mcp_servers.iter_mut() {
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut scfg.enabled, name).changed() {
                                changed = true;
                            }
                            let summary = if !scfg.url.is_empty() {
                                format!("HTTP: {}", scfg.url)
                            } else if scfg.env.is_empty() {
                                format!("{} {}", scfg.command, scfg.args.join(" "))
                            } else {
                                format!("{} {} [env: {}]", scfg.command, scfg.args.join(" "), scfg.env.len())
                            };
                            ui.label(RichText::new(summary).size(11.5).color(TEXT_MUTED));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Remove").clicked() {
                                    remove_name = Some(name.clone());
                                }
                            });
                        });
                    }
                    if let Some(n) = remove_name {
                        self.mcp_cfg.mcp_servers.remove(&n);
                        changed = true;
                    }
                    if changed {
                        mcp_client::save_config(&self.mcp_cfg);
                        self.refresh_mcp(&ctx);
                    }

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Add MCP Server:");
                        ui.selectable_value(&mut self.new_mcp_is_http, false, "Stdio");
                        ui.selectable_value(&mut self.new_mcp_is_http, true, "Streamable HTTP");
                    });

                    if self.new_mcp_is_http {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.new_mcp_name).hint_text("name").desired_width(100.0));
                            ui.add(egui::TextEdit::singleline(&mut self.new_mcp_url).hint_text("http://127.0.0.1:8000/mcp").desired_width(340.0));
                            if ui.button("Add").clicked()
                                && !self.new_mcp_name.trim().is_empty()
                                && !self.new_mcp_url.trim().is_empty()
                            {
                                self.mcp_cfg.mcp_servers.insert(
                                    self.new_mcp_name.trim().to_string(),
                                    McpServerConfig {
                                        command: String::new(),
                                        args: Vec::new(),
                                        env: BTreeMap::new(),
                                        url: self.new_mcp_url.trim().to_string(),
                                        enabled: true,
                                    },
                                );
                                self.new_mcp_name.clear();
                                self.new_mcp_url.clear();
                                mcp_client::save_config(&self.mcp_cfg);
                                self.refresh_mcp(&ctx);
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.new_mcp_name).hint_text("name").desired_width(90.0));
                            ui.add(egui::TextEdit::singleline(&mut self.new_mcp_cmd).hint_text("command (python, exe, npx)").desired_width(170.0));
                            ui.add(egui::TextEdit::singleline(&mut self.new_mcp_args).hint_text("args (space-separated)").desired_width(180.0));
                        });
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.new_mcp_env).hint_text("ENV (optional: KEY=VAL,KEY2=VAL2)").desired_width(380.0));
                            if ui.button("Add Stdio").clicked()
                                && !self.new_mcp_name.trim().is_empty()
                                && !self.new_mcp_cmd.trim().is_empty()
                            {
                                let args: Vec<String> = self.new_mcp_args.split_whitespace().map(|s| s.to_string()).collect();
                                let mut env = BTreeMap::new();
                                for pair in self.new_mcp_env.split(',') {
                                    if let Some((k, v)) = pair.split_once('=') {
                                        if !k.trim().is_empty() {
                                            env.insert(k.trim().to_string(), v.trim().to_string());
                                        }
                                    }
                                }
                                self.mcp_cfg.mcp_servers.insert(
                                    self.new_mcp_name.trim().to_string(),
                                    McpServerConfig {
                                        command: self.new_mcp_cmd.trim().to_string(),
                                        args,
                                        env,
                                        url: String::new(),
                                        enabled: true,
                                    },
                                );
                                self.new_mcp_name.clear();
                                self.new_mcp_cmd.clear();
                                self.new_mcp_args.clear();
                                self.new_mcp_env.clear();
                                mcp_client::save_config(&self.mcp_cfg);
                                self.refresh_mcp(&ctx);
                            }
                        });
                    }

                    ui.separator();
                    ui.label("Create New MCP Server (Scaffold + Auto-Register):");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.scaffold_name).hint_text("server-name").desired_width(140.0));
                        ui.selectable_value(&mut self.scaffold_lang, "python".to_string(), "Python");
                        ui.selectable_value(&mut self.scaffold_lang, "rust".to_string(), "Rust");
                        if ui.button("Pick Folder & Generate...").clicked() && !self.scaffold_name.trim().is_empty() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                let name = self.scaffold_name.trim().to_string();
                                match mcp_client::scaffold_mcp_server(&self.scaffold_lang, &name, &dir) {
                                    Ok(cfg) => {
                                        self.mcp_cfg.mcp_servers.insert(name.clone(), cfg);
                                        mcp_client::save_config(&self.mcp_cfg);
                                        self.mcp_status_msg = format!("Created & registered '{}' in {}", name, dir.display());
                                        self.refresh_mcp(&ctx);
                                    }
                                    Err(e) => self.mcp_status_msg = format!("Scaffold error: {}", e),
                                }
                            }
                        }
                    });
                    if !self.mcp_status_msg.is_empty() {
                        ui.label(RichText::new(&self.mcp_status_msg).size(11.5).color(GREEN));
                    }

                    if !self.mcp_errors.is_empty() {
                        ui.add_space(4.0);
                        for err in &self.mcp_errors {
                            ui.label(RichText::new(err).size(11.0).color(RED));
                        }
                    }

                    if !self.mcp_tools.is_empty() {
                        ui.separator();
                        ui.label(RichText::new(format!("Discovered Tools ({}):", self.mcp_tools.len())).size(12.0));
                        egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                            for t in &self.mcp_tools {
                                ui.label(
                                    RichText::new(format!("• {} — {}", t.full_name, t.description))
                                        .size(11.5)
                                        .color(TEXT_MUTED),
                                );
                            }
                        });
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Refresh Tools").clicked() {
                            self.refresh_mcp(&ctx);
                        }
                        if ui.button("Close").clicked() {
                            self.show_mcp_modal = false;
                        }
                    });
                });
        }

        // First-Run Onboarding Modal
        if self.show_onboarding {
            egui::Window::new("Welcome to Yokahu — Model Setup")
                .collapsible(false)
                .resizable(false)
                .default_width(440.0)
                .show(&ctx, |ui| {
                    ui.label("No active local model endpoint was detected yet. Choose how you want to run models:");
                    ui.add_space(6.0);
                    ui.label(RichText::new("1. Ollama (recommended): install Ollama, then pull any model (e.g. qwen3.5:9b, llama3.2).").size(12.0).color(TEXT_MUTED));
                    ui.label(RichText::new("2. OpenAI-Compatible: point to LM Studio, llama-server, or vLLM.").size(12.0).color(TEXT_MUTED));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open Provider / Pull Model...").clicked() {
                            self.show_onboarding = false;
                            self.show_provider_modal = true;
                        }
                        if ui.button("Retry Connection").clicked() {
                            self.show_onboarding = false;
                            self.refresh_models(&ctx);
                        }
                        if ui.button("Dismiss").clicked() {
                            self.show_onboarding = false;
                        }
                    });
                });
        }
    }
}

fn render_markdown(ui: &mut egui::Ui, ctx: &egui::Context, text: &str) {
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let before = &rest[..start];
        if !before.trim().is_empty() {
            ui.label(RichText::new(before.trim_end()).color(TEXT));
        }
        let after_ticks = &rest[start + 3..];
        let (lang, code_start) = if let Some(nl) = after_ticks.find('\n') {
            (after_ticks[..nl].trim(), &after_ticks[nl + 1..])
        } else {
            ("", after_ticks)
        };
        let (code, next_rest) = if let Some(end) = code_start.find("```") {
            (&code_start[..end], &code_start[end + 3..])
        } else {
            (code_start, "")
        };

        let box_frame = egui::Frame::NONE
            .fill(Color32::from_rgb(13, 17, 23))
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(10.0);
        box_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if lang.is_empty() { "code" } else { lang })
                        .size(11.0)
                        .color(TEXT_MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Copy").clicked() {
                        ctx.copy_text(code.to_string());
                    }
                });
            });
            ui.separator();
            ui.label(
                RichText::new(code.trim_end())
                    .font(FontId::new(12.5, FontFamily::Monospace))
                    .color(TEXT),
            );
        });
        rest = next_rest;
    }
    if !rest.trim().is_empty() {
        ui.label(RichText::new(rest.trim_start_matches('\n')).color(TEXT));
    }
}

fn parse_http_url(base_url: &str) -> Result<(String, u16, String), String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let (host_port, prefix) = match without_scheme.split_once('/') {
        Some((hp, p)) => (hp, format!("/{}", p)),
        None => (without_scheme, String::new()),
    };
    let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(80))
    } else {
        (host_port.to_string(), 80)
    };
    Ok((host, port, prefix))
}

fn connect_http(base_url: &str, timeout_secs: u64) -> Result<(TcpStream, String, u16, String), String> {
    let (host, port, prefix) = parse_http_url(base_url)?;
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e| format!("Invalid address {}:{}: {}", host, port, e))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4))
        .map_err(|e| format!("Connect failed: {}", e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(timeout_secs)));
    Ok((stream, host, port, prefix))
}

fn http_request(
    base_url: &str,
    method: &str,
    subpath: &str,
    api_key: &str,
    body: Option<&str>,
    timeout_secs: u64,
) -> Result<(BufReader<TcpStream>, bool), String> {
    let (mut stream, host, port, prefix) = connect_http(base_url, timeout_secs)?;
    let full_path = format!("{}{}", prefix, subpath);
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        method, full_path, host, port
    );
    if !api_key.trim().is_empty() {
        req.push_str(&format!("Authorization: Bearer {}\r\n", api_key.trim()));
    }
    if let Some(b) = body {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("Write error: {}", e))?;

    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    let _ = reader.read_line(&mut status);
    let mut chunked = false;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if h.to_ascii_lowercase().contains("transfer-encoding")
            && h.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    Ok((reader, chunked))
}

fn read_chunk_or_line(reader: &mut BufReader<TcpStream>, chunked: bool) -> Option<String> {
    if chunked {
        let mut sz_line = String::new();
        if reader.read_line(&mut sz_line).unwrap_or(0) == 0 {
            return None;
        }
        let hex = sz_line.trim().split(';').next().unwrap_or("");
        let sz = usize::from_str_radix(hex, 16).ok()?;
        if sz == 0 {
            return None;
        }
        let mut buf = vec![0u8; sz];
        if reader.read_exact(&mut buf).is_err() {
            return None;
        }
        let mut crlf = [0u8; 2];
        let _ = reader.read_exact(&mut crlf);
        Some(String::from_utf8_lossy(&buf).to_string())
    } else {
        let mut l = String::new();
        if reader.read_line(&mut l).unwrap_or(0) == 0 {
            None
        } else {
            Some(l)
        }
    }
}

fn fetch_models(
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
    let Ok((mut reader, chunked)) = http_request(base_url, "GET", subpath, api_key, None, 5) else {
        let _ = tx.send(WorkerEvent::Models(Vec::new(), false));
        ctx.request_repaint();
        return;
    };
    let mut raw = String::new();
    while let Some(c) = read_chunk_or_line(&mut reader, chunked) {
        raw.push_str(&c);
    }
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

fn pull_ollama_model(base_url: &str, model: &str, tx: &Sender<WorkerEvent>, ctx: &egui::Context) {
    let payload = json!({ "model": model, "stream": true }).to_string();
    let Ok((mut reader, chunked)) = http_request(base_url, "POST", "/api/pull", "", Some(&payload), 1800) else {
        let _ = tx.send(WorkerEvent::PullStatus("Failed to connect to Ollama".to_string(), false));
        ctx.request_repaint();
        return;
    };
    let mut buf = String::new();
    while let Some(chunk) = read_chunk_or_line(&mut reader, chunked) {
        buf.push_str(&chunk);
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf.drain(..=nl);
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if let Some(st) = v.get("status").and_then(|s| s.as_str()) {
                    let done = st == "success";
                    let _ = tx.send(WorkerEvent::PullStatus(format!("{}: {}", model, st), done));
                    ctx.request_repaint();
                }
            }
        }
    }
}

fn run_agent_loop(
    history: Vec<ChatMessage>,
    prov: ProviderKind,
    base_url: String,
    api_key: String,
    model: String,
    system: String,
    think: bool,
    temp: f32,
    num_ctx: i64,
    keep_alive: String,
    workspace: Option<PathBuf>,
    safety: SafetyMode,
    mcp_cfg: McpConfigFile,
    mcp_tools: Vec<DiscoveredMcpTool>,
    tx: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let mut api_msgs: Vec<Value> = Vec::new();
    let mut sys_full = system;
    if let Some(ws) = &workspace {
        sys_full.push_str(&format!(
            "\nActive workspace directory: {}\nYou have workspace tools (list_dir, read_file, search_files, write_file, run_command).",
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
    if workspace.is_some() {
        if let Some(arr) = tools::ollama_tools_schema().as_array() {
            all_tools.extend(arr.iter().cloned());
        }
    }
    all_tools.extend(mcp_client::to_tool_schemas(&mcp_tools));

    for _step in 0..8 {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let (subpath, payload) = match prov {
            ProviderKind::Ollama => {
                let mut body = json!({
                    "model": model,
                    "messages": api_msgs,
                    "stream": true,
                    "think": think,
                    "keep_alive": keep_alive,
                    "options": { "temperature": temp, "num_ctx": num_ctx }
                });
                if !all_tools.is_empty() {
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
                if !all_tools.is_empty() {
                    body["tools"] = Value::Array(all_tools.clone());
                }
                ("/chat/completions", body.to_string())
            }
        };

        let Ok((mut reader, chunked)) = http_request(&base_url, "POST", subpath, &api_key, Some(&payload), 600) else {
            let _ = tx.send(WorkerEvent::Error(format!("Cannot connect to {}", base_url)));
            break;
        };

        let mut assistant_text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut line_buf = String::new();

        while let Some(chunk) = read_chunk_or_line(&mut reader, chunked) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            line_buf.push_str(&chunk);
            while let Some(nl) = line_buf.find('\n') {
                let mut one = line_buf[..nl].trim().to_string();
                line_buf.drain(..=nl);
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
                        continue;
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
                                    tool_calls.extend(tcs.iter().cloned());
                                }
                            }
                        }
                    }
                }
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
            let func = tc.get("function").unwrap_or(&tc);
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = match func.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({})),
                Some(v) => v.clone(),
                None => json!({}),
            };

            let _ = tx.send(WorkerEvent::ToolStart(name.to_string()));
            ctx.request_repaint();

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
                        let (rtx, rrx) = mpsc::channel();
                        let _ = tx.send(WorkerEvent::NeedApproval(PendingApproval {
                            tool_name: name.to_string(),
                            preview,
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
                mcp_client::call_mcp_tool(&mcp_cfg, name, &args)
                    .unwrap_or_else(|e| format!("MCP error: {}", e))
            } else {
                tools::execute_tool(workspace.as_deref(), name, &args)
            };

            let _ = tx.send(WorkerEvent::ToolResult {
                name: name.to_string(),
                output: output.clone(),
            });
            ctx.request_repaint();
            api_msgs.push(json!({ "role": "tool", "content": output }));
        }
    }

    let _ = tx.send(WorkerEvent::Done);
    ctx.request_repaint();
}

fn state_path() -> PathBuf {
    mcp_client::app_data_dir().join("state.json")
}

fn load_state() -> PersistedState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(state: &PersistedState) {
    let p = state_path();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = fs::write(p, json);
    }
}

fn read_attachment(path: &Path) -> Option<Attachment> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif");
    if is_image {
        let bytes = fs::read(path).ok()?;
        Some(Attachment {
            name,
            is_image: true,
            data: encode_base64(&bytes),
        })
    } else {
        let text = fs::read_to_string(path).ok()?;
        Some(Attachment {
            name,
            is_image: false,
            data: text,
        })
    }
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
        out.push('…');
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

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_ROOT;
    visuals.window_fill = BG_SURFACE;
    visuals.extreme_bg_color = BG_ROOT;
    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(35, 43, 62);
    visuals.widgets.active.bg_fill = Color32::from_rgb(44, 55, 80);
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(88, 166, 255, 60);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    ctx.set_visuals(visuals);
}

fn main() -> eframe::Result<()> {
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 780.0])
        .with_min_inner_size([860.0, 560.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        vp = vp.with_icon(Arc::new(icon));
    }
    let opts = eframe::NativeOptions {
        viewport: vp,
        ..Default::default()
    };
    eframe::run_native("Yokahu", opts, Box::new(|cc| Ok(Box::new(YokahuApp::new(cc)))))
}

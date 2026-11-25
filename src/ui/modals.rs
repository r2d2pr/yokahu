use crate::mcp_client::{self, McpServerConfig};
use crate::theme::{self, ThemeConfig};
use crate::ui::{agent, read_attachment, save_state, ProviderKind, YokahuApp, GREEN, RED};
use eframe::egui::{self, FontFamily, FontId, RichText};
use std::collections::BTreeMap;
use std::thread;

pub fn render_modals(app: &mut YokahuApp, ctx: &egui::Context, th: &ThemeConfig) {
    if app.show_sys_modal {
        egui::Window::new("System Prompt")
            .collapsible(false)
            .resizable(true)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut app.sys_draft)
                        .desired_rows(6)
                        .desired_width(ui.available_width()),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        app.state.system_prompt = app.sys_draft.clone();
                        save_state(&app.state);
                        app.show_sys_modal = false;
                    }
                    if ui.button("Cancel").clicked() {
                        app.show_sys_modal = false;
                    }
                });
            });
    }

    if app.show_provider_modal {
        egui::Window::new("Model Provider & Pull")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Provider:");
                    for p in [ProviderKind::Ollama, ProviderKind::OpenAiCompat] {
                        if ui.selectable_value(&mut app.state.provider, p, p.label()).changed() {
                            app.state.base_url = match p {
                                ProviderKind::Ollama => "http://127.0.0.1:11434".to_string(),
                                ProviderKind::OpenAiCompat => "http://127.0.0.1:1234/v1".to_string(),
                            };
                            save_state(&app.state);
                            app.refresh_models(ctx);
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label("Base URL (supports http:// and https://):");
                if ui
                    .add(egui::TextEdit::singleline(&mut app.state.base_url).desired_width(ui.available_width()))
                    .changed()
                {
                    save_state(&app.state);
                }
                if app.state.provider == ProviderKind::OpenAiCompat {
                    ui.add_space(4.0);
                    ui.label("API Key (optional for local servers):");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut app.state.api_key)
                                .password(true)
                                .desired_width(ui.available_width()),
                        )
                        .changed()
                    {
                        save_state(&app.state);
                    }
                } else {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Ollama VRAM Keep-Alive:");
                        egui::ComboBox::from_id_salt("ka_modal")
                            .selected_text(&app.state.keep_alive)
                            .show_ui(ui, |ui| {
                                for ka in ["5m", "15m", "0"] {
                                    if ui.selectable_value(&mut app.state.keep_alive, ka.to_string(), ka).changed() {
                                        save_state(&app.state);
                                    }
                                }
                            });
                    });
                    ui.separator();
                    ui.label("Pull any Ollama model (e.g. llama3.2, qwen3.5:9b, deepseek-r1:8b):");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut app.pull_model_input)
                                .hint_text("model:tag")
                                .desired_width(240.0),
                        );
                        if ui.button("Pull Model").clicked() && !app.pull_model_input.trim().is_empty() {
                            let m = app.pull_model_input.trim().to_string();
                            let url = app.state.base_url.clone();
                            let tx = app.tx.clone();
                            let ctx_c = ctx.clone();
                            app.pull_status = format!("Pulling {}...", m);
                            thread::spawn(move || agent::pull_ollama_model(&url, &m, &tx, &ctx_c));
                        }
                    });
                    if !app.pull_status.is_empty() {
                        ui.label(RichText::new(&app.pull_status).size(11.5).color(th.c_accent()));
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Refresh Models").clicked() {
                        app.refresh_models(ctx);
                    }
                    if ui.button("Close").clicked() {
                        app.show_provider_modal = false;
                    }
                });
            });
    }

    if app.show_mcp_modal {
        egui::Window::new("MCP Servers (Custom Tools)")
            .collapsible(false)
            .resizable(true)
            .default_width(600.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("Config: {}", mcp_client::config_path().display()))
                        .size(11.0)
                        .color(th.c_muted()),
                );
                ui.add_space(6.0);

                let mut remove_name: Option<String> = None;
                let mut changed = false;
                for (name, scfg) in app.mcp_cfg.mcp_servers.iter_mut() {
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
                        ui.label(RichText::new(summary).size(11.5).color(th.c_muted()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                remove_name = Some(name.clone());
                            }
                        });
                    });
                }
                if let Some(n) = remove_name {
                    app.mcp_cfg.mcp_servers.remove(&n);
                    changed = true;
                }
                if changed {
                    mcp_client::save_config(&app.mcp_cfg);
                    app.refresh_mcp(ctx);
                }

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Add MCP Server:");
                    ui.selectable_value(&mut app.new_mcp_is_http, false, "Stdio");
                    ui.selectable_value(&mut app.new_mcp_is_http, true, "Streamable HTTP(S)");
                });

                if app.new_mcp_is_http {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut app.new_mcp_name).hint_text("name").desired_width(100.0));
                        ui.add(egui::TextEdit::singleline(&mut app.new_mcp_url).hint_text("https://.../mcp").desired_width(340.0));
                        if ui.button("Add").clicked()
                            && !app.new_mcp_name.trim().is_empty()
                            && !app.new_mcp_url.trim().is_empty()
                        {
                            app.mcp_cfg.mcp_servers.insert(
                                app.new_mcp_name.trim().to_string(),
                                McpServerConfig {
                                    command: String::new(),
                                    args: Vec::new(),
                                    env: BTreeMap::new(),
                                    url: app.new_mcp_url.trim().to_string(),
                                    enabled: true,
                                },
                            );
                            app.new_mcp_name.clear();
                            app.new_mcp_url.clear();
                            mcp_client::save_config(&app.mcp_cfg);
                            app.refresh_mcp(ctx);
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut app.new_mcp_name).hint_text("name").desired_width(90.0));
                        ui.add(egui::TextEdit::singleline(&mut app.new_mcp_cmd).hint_text("command (python, exe, npx)").desired_width(170.0));
                        ui.add(egui::TextEdit::singleline(&mut app.new_mcp_args).hint_text("args (space-separated)").desired_width(180.0));
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut app.new_mcp_env).hint_text("ENV (KEY=VAL,KEY2=VAL2)").desired_width(380.0));
                        if ui.button("Add Stdio").clicked()
                            && !app.new_mcp_name.trim().is_empty()
                            && !app.new_mcp_cmd.trim().is_empty()
                        {
                            let args: Vec<String> = app.new_mcp_args.split_whitespace().map(|s| s.to_string()).collect();
                            let mut env = BTreeMap::new();
                            for pair in app.new_mcp_env.split(',') {
                                if let Some((k, v)) = pair.split_once('=') {
                                    if !k.trim().is_empty() {
                                        env.insert(k.trim().to_string(), v.trim().to_string());
                                    }
                                }
                            }
                            app.mcp_cfg.mcp_servers.insert(
                                app.new_mcp_name.trim().to_string(),
                                McpServerConfig {
                                    command: app.new_mcp_cmd.trim().to_string(),
                                    args,
                                    env,
                                    url: String::new(),
                                    enabled: true,
                                },
                            );
                            app.new_mcp_name.clear();
                            app.new_mcp_cmd.clear();
                            app.new_mcp_args.clear();
                            app.new_mcp_env.clear();
                            mcp_client::save_config(&app.mcp_cfg);
                            app.refresh_mcp(ctx);
                        }
                    });
                }

                ui.separator();
                ui.label("Create New MCP Server (Scaffold + Auto-Register):");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut app.scaffold_name).hint_text("server-name").desired_width(140.0));
                    ui.selectable_value(&mut app.scaffold_lang, "python".to_string(), "Python");
                    ui.selectable_value(&mut app.scaffold_lang, "rust".to_string(), "Rust");
                    if ui.button("Pick Folder & Generate...").clicked() && !app.scaffold_name.trim().is_empty() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            let name = app.scaffold_name.trim().to_string();
                            match mcp_client::scaffold_mcp_server(&app.scaffold_lang, &name, &dir) {
                                Ok(cfg) => {
                                    app.mcp_cfg.mcp_servers.insert(name.clone(), cfg);
                                    mcp_client::save_config(&app.mcp_cfg);
                                    app.mcp_status_msg = format!("Created & registered '{}' in {}", name, dir.display());
                                    app.refresh_mcp(ctx);
                                }
                                Err(e) => app.mcp_status_msg = format!("Scaffold error: {}", e),
                            }
                        }
                    }
                });
                if !app.mcp_status_msg.is_empty() {
                    ui.label(RichText::new(&app.mcp_status_msg).size(11.5).color(GREEN));
                }
                for err in &app.mcp_errors {
                    ui.label(RichText::new(err).size(11.0).color(RED));
                }
                if !app.mcp_tools.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(format!("Discovered Tools ({}):", app.mcp_tools.len())).size(12.0));
                    egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                        for t in &app.mcp_tools {
                            ui.label(RichText::new(format!("\u{2022} {} - {}", t.full_name, t.description)).size(11.5).color(th.c_muted()));
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Refresh Tools").clicked() {
                        app.refresh_mcp(ctx);
                    }
                    if ui.button("Restart MCP").clicked() {
                        app.mcp_pool.restart_all();
                        app.refresh_mcp(ctx);
                    }
                    if ui.button("Close").clicked() {
                        app.show_mcp_modal = false;
                    }
                });
            });
    }

    if app.show_onboarding {
        egui::Window::new("Welcome to Yokahu - Model Setup")
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.label("No active local model endpoint was detected yet. Choose how you want to run models:");
                ui.add_space(6.0);
                ui.label(RichText::new("1. Ollama (recommended): install Ollama, then pull any model (e.g. qwen3.5:9b, llama3.2).").size(12.0).color(th.c_muted()));
                ui.label(RichText::new("2. OpenAI-Compatible: point to LM Studio, llama-server, or remote HTTPS APIs.").size(12.0).color(th.c_muted()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Open Provider / Pull Model...").clicked() {
                        app.show_onboarding = false;
                        app.show_provider_modal = true;
                    }
                    if ui.button("Retry Connection").clicked() {
                        app.show_onboarding = false;
                        app.refresh_models(ctx);
                    }
                    if ui.button("Dismiss").clicked() {
                        app.show_onboarding = false;
                    }
                });
            });
    }

    if app.show_theme_modal {
        egui::Window::new("Appearance & UI Colors")
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Theme Presets").strong());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for name in ThemeConfig::presets() {
                        let selected = app.state.theme.preset_name == *name;
                        if ui.selectable_label(selected, *name).clicked() {
                            app.state.theme = ThemeConfig::preset(name);
                            theme::apply_theme(ctx, &app.state.theme);
                            save_state(&app.state);
                        }
                    }
                });
                ui.separator();
                ui.label(RichText::new("Custom Colors").strong());
                ui.add_space(4.0);
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.label("Accent:");
                    changed |= ui.color_edit_button_srgb(&mut app.state.theme.accent).changed();
                    ui.add_space(10.0);
                    ui.label("Background:");
                    changed |= ui.color_edit_button_srgb(&mut app.state.theme.bg_root).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Sidebar:");
                    changed |= ui.color_edit_button_srgb(&mut app.state.theme.bg_sidebar).changed();
                    ui.add_space(10.0);
                    ui.label("Surface:");
                    changed |= ui.color_edit_button_srgb(&mut app.state.theme.bg_surface).changed();
                });
                if ui.checkbox(&mut app.state.theme.is_light, "Light base mode").changed() {
                    changed = true;
                }
                if changed {
                    app.state.theme.preset_name = "Custom".to_string();
                    theme::apply_theme(ctx, &app.state.theme);
                    save_state(&app.state);
                }
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    app.show_theme_modal = false;
                }
            });
    }

    if let Some((fname, fpath, mut fbody)) = app.preview_file.clone() {
        let mut open = true;
        let mut close_now = false;
        egui::Window::new(format!("Editor / Preview: {}", fname))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([660.0, 500.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(fpath.display().to_string()).size(11.0).color(th.c_muted()));
                ui.separator();
                let mut edited = false;
                egui::ScrollArea::both().max_height(380.0).show(ui, |ui| {
                    let te = egui::TextEdit::multiline(&mut fbody)
                        .font(FontId::new(12.5, FontFamily::Monospace))
                        .desired_width(ui.available_width())
                        .desired_rows(20);
                    if ui.add(te).changed() {
                        edited = true;
                    }
                });
                if edited {
                    app.preview_file = Some((fname.clone(), fpath.clone(), fbody.clone()));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        match std::fs::write(&fpath, &fbody) {
                            Ok(_) => app.status_note = format!("Saved {}", fname),
                            Err(e) => app.status_note = format!("Save failed: {}", e),
                        }
                    }
                    if ui.button("Attach to Chat").clicked() {
                        if let Some(att) = read_attachment(ctx, &fpath) {
                            app.attachments.push(att);
                        }
                        close_now = true;
                    }
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(fbody);
                    }
                    if ui.button("Close").clicked() {
                        close_now = true;
                    }
                });
            });
        if !open || close_now {
            app.preview_file = None;
        }
    }

    if app.show_palette {
        egui::Window::new("Command Palette (Ctrl+K)")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
            .default_width(480.0)
            .show(ctx, |ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.palette_query)
                        .hint_text("Jump to chat, model, theme, or persona...")
                        .desired_width(ui.available_width()),
                );
                resp.request_focus();
                let q = app.palette_query.to_lowercase();
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for c in &app.state.chats {
                        if q.is_empty() || c.title.to_lowercase().contains(&q) {
                            if ui.selectable_label(false, format!("[Chat] {}", c.title)).clicked() {
                                app.state.active_id = Some(c.id.clone());
                                app.show_palette = false;
                                save_state(&app.state);
                            }
                        }
                    }
                    for m in &app.models {
                        if q.is_empty() || m.to_lowercase().contains(&q) {
                            if ui.selectable_label(false, format!("[Model] {}", m)).clicked() {
                                app.state.model = m.clone();
                                app.show_palette = false;
                                save_state(&app.state);
                            }
                        }
                    }
                    for p in &app.state.personas {
                        if q.is_empty() || p.name.to_lowercase().contains(&q) {
                            if ui.selectable_label(false, format!("[Persona] {}", p.name)).clicked() {
                                app.state.active_persona = p.name.clone();
                                app.state.system_prompt = p.prompt.clone();
                                app.show_palette = false;
                                save_state(&app.state);
                            }
                        }
                    }
                    for tname in ThemeConfig::presets() {
                        if q.is_empty() || tname.to_lowercase().contains(&q) {
                            if ui.selectable_label(false, format!("[Theme] {}", tname)).clicked() {
                                app.state.theme = ThemeConfig::preset(tname);
                                theme::apply_theme(ctx, &app.state.theme);
                                app.show_palette = false;
                                save_state(&app.state);
                            }
                        }
                    }
                });
                ui.add_space(4.0);
                if ui.button("Close (Esc)").clicked() {
                    app.show_palette = false;
                }
            });
    }
}

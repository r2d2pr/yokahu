use crate::theme::ThemeConfig;
use crate::tools;
use crate::ui::{read_attachment, save_state, PersistedState, WorkerEvent, YokahuApp, GREEN};
use eframe::egui::{self, CornerRadius, FontFamily, FontId, RichText, Stroke};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

pub fn render_editor_panel(app: &mut YokahuApp, ctx: &egui::Context, ui: &mut egui::Ui, th: &ThemeConfig) {
    if app.open_tabs.is_empty() {
        return;
    }
    if app.active_tab_idx >= app.open_tabs.len() {
        app.active_tab_idx = app.open_tabs.len() - 1;
    }

    egui::Panel::right("editor_panel")
        .resizable(true)
        .default_size(420.0)
        .size_range(280.0..=760.0)
        .frame(egui::Frame::NONE.fill(th.c_sidebar()).inner_margin(10.0))
        .show(ui, |ui| {
            let mut close_idx: Option<usize> = None;
            ui.horizontal_wrapped(|ui| {
                for (i, tab) in app.open_tabs.iter().enumerate() {
                    let selected = i == app.active_tab_idx;
                    let lbl = if tab.dirty { format!("{}*", tab.name) } else { tab.name.clone() };
                    if ui.selectable_label(selected, lbl).clicked() {
                        app.active_tab_idx = i;
                    }
                    if ui.small_button("x").clicked() {
                        close_idx = Some(i);
                    }
                }
            });
            if let Some(i) = close_idx {
                app.open_tabs.remove(i);
                if app.open_tabs.is_empty() {
                    return;
                }
                if app.active_tab_idx >= app.open_tabs.len() {
                    app.active_tab_idx = app.open_tabs.len() - 1;
                }
            }

            ui.separator();
            let tab = &mut app.open_tabs[app.active_tab_idx];
            ui.label(RichText::new(tab.path.display().to_string()).size(10.5).color(th.c_muted()));
            ui.add_space(4.0);

            let h = (ui.available_height() - 42.0).max(120.0);
            egui::ScrollArea::both().max_height(h).show(ui, |ui| {
                let te = egui::TextEdit::multiline(&mut tab.content)
                    .font(FontId::new(12.5, FontFamily::Monospace))
                    .desired_width(ui.available_width())
                    .desired_rows(24);
                if ui.add(te).changed() {
                    tab.dirty = true;
                }
            });

            ui.add_space(4.0);
            let mut close_all = false;
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    match fs::write(&tab.path, &tab.content) {
                        Ok(_) => {
                            tab.dirty = false;
                            app.status_note = format!("Saved {}", tab.name);
                        }
                        Err(e) => app.status_note = format!("Save failed: {}", e),
                    }
                }
                if ui.button("Attach").clicked() {
                    if let Some(att) = read_attachment(ctx, &tab.path) {
                        app.attachments.push(att);
                    }
                }
                if ui.button("Close All").clicked() {
                    close_all = true;
                }
            });
            if close_all {
                app.open_tabs.clear();
            }
        });
}

pub fn render_terminal_drawer(app: &mut YokahuApp, ctx: &egui::Context, ui: &mut egui::Ui, th: &ThemeConfig) {
    if !app.show_terminal {
        return;
    }
    egui::Panel::bottom("terminal_drawer")
        .resizable(true)
        .default_size(190.0)
        .size_range(120.0..=420.0)
        .frame(egui::Frame::NONE.fill(th.c_surface()).stroke(Stroke::new(1.0, th.c_border())).inner_margin(10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Terminal & Background Tasks").size(12.0).strong().color(th.c_text()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Close").clicked() {
                        app.show_terminal = false;
                    }
                    if ui.small_button("Clear").clicked() {
                        app.term_output.clear();
                    }
                    if ui.small_button("List Bg Tasks").clicked() {
                        app.term_output = app.bg_tasks.manage("list", None);
                    }
                });
            });
            ui.horizontal(|ui| {
                let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                let te = ui.add(
                    egui::TextEdit::singleline(&mut app.term_input)
                        .hint_text("Run shell command in workspace...")
                        .desired_width((ui.available_width() - 120.0).max(180.0)),
                );
                let run_clicked = ui.button("Run").clicked() || (te.has_focus() && enter);
                if run_clicked && !app.term_input.trim().is_empty() {
                    let cmd_str = app.term_input.trim().to_string();
                    app.term_input.clear();
                    let ws = app.state.workspace.clone().map(PathBuf::from);
                    let tx = app.tx.clone();
                    let ctx_c = ctx.clone();
                    app.term_output = format!("$ {}\nRunning...", cmd_str);
                    thread::spawn(move || {
                        let out = tools::execute_tool(
                            ws.as_deref(),
                            "run_command",
                            &serde_json::json!({ "command": cmd_str }),
                        );
                        let _ = tx.send(WorkerEvent::TerminalOut(out));
                        ctx_c.request_repaint();
                    });
                }
                if ui.button("Run Bg").clicked() && !app.term_input.trim().is_empty() {
                    let cmd_str = app.term_input.trim().to_string();
                    app.term_input.clear();
                    let ws = app.state.workspace.clone().map(PathBuf::from);
                    app.term_output = app.bg_tasks.spawn(ws.as_deref(), &cmd_str);
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                ui.label(
                    RichText::new(&app.term_output)
                        .font(FontId::new(12.0, FontFamily::Monospace))
                        .color(th.c_muted()),
                );
            });
        });
}

pub fn render_extra_modals(app: &mut YokahuApp, ctx: &egui::Context, th: &ThemeConfig) {
    if app.show_workflows_modal {
        egui::Window::new("Workflows & Scheduled Macros")
            .collapsible(false)
            .resizable(true)
            .default_width(500.0)
            .show(ctx, |ui| {
                let mut run_prompt: Option<String> = None;
                let mut del_idx: Option<usize> = None;
                for (i, wf) in app.state.workflows.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&wf.name).strong());
                        if wf.interval_secs > 0 {
                            ui.label(RichText::new(format!("(every {}s)", wf.interval_secs)).size(11.0).color(th.c_accent()));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Delete").clicked() {
                                del_idx = Some(i);
                            }
                            if ui.small_button("Run Now").clicked() {
                                run_prompt = Some(wf.prompt.clone());
                            }
                        });
                    });
                }
                if let Some(i) = del_idx {
                    app.state.workflows.remove(i);
                    save_state(&app.state);
                }
                if let Some(p) = run_prompt {
                    app.prompt_input = p;
                    app.show_workflows_modal = false;
                    app.send_message(ctx);
                }

                ui.separator();
                ui.label("New Workflow Macro:");
                ui.add(egui::TextEdit::singleline(&mut app.new_wf_name).hint_text("Macro name").desired_width(ui.available_width()));
                ui.add(egui::TextEdit::multiline(&mut app.new_wf_prompt).hint_text("Prompt to execute...").desired_rows(3).desired_width(ui.available_width()));
                ui.horizontal(|ui| {
                    ui.label("Auto-run interval (sec, 0 = manual):");
                    ui.add(egui::DragValue::new(&mut app.new_wf_interval).range(0..=86400));
                    if ui.button("Save Macro").clicked()
                        && !app.new_wf_name.trim().is_empty()
                        && !app.new_wf_prompt.trim().is_empty()
                    {
                        app.state.workflows.push(crate::ui::WorkflowMacro {
                            name: app.new_wf_name.trim().to_string(),
                            prompt: app.new_wf_prompt.trim().to_string(),
                            interval_secs: app.new_wf_interval,
                            last_run_epoch: 0,
                        });
                        app.new_wf_name.clear();
                        app.new_wf_prompt.clear();
                        app.new_wf_interval = 0;
                        save_state(&app.state);
                    }
                });
                ui.add_space(6.0);
                if ui.button("Close").clicked() {
                    app.show_workflows_modal = false;
                }
            });
    }

    if app.show_stats_modal {
        egui::Window::new("Usage & Performance Dashboard")
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                let st = &app.state.stats;
                let avg_tps = if st.total_secs > 0.0 { st.total_tokens as f32 / st.total_secs } else { 0.0 };
                ui.horizontal(|ui| {
                    stat_card(ui, th, "Total Requests", &st.total_requests.to_string());
                    stat_card(ui, th, "Total Tokens", &st.total_tokens.to_string());
                    stat_card(ui, th, "Avg Speed", &format!("{:.1} tok/s", avg_tps));
                });
                ui.separator();
                ui.label(RichText::new("Per-Model Breakdown").strong());
                for (m, ms) in &st.per_model {
                    let mtps = if ms.secs > 0.0 { ms.tokens as f32 / ms.secs } else { 0.0 };
                    ui.label(
                        RichText::new(format!("- {} : {} runs, {} tokens, {:.1} tok/s", m, ms.requests, ms.tokens, mtps))
                            .size(12.0)
                            .color(th.c_muted()),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Export Backup (.json)").clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_file_name("yokahu_backup.json").save_file() {
                            if let Ok(json) = serde_json::to_string_pretty(&app.state) {
                                let _ = fs::write(&p, json);
                                app.status_note = "Backup exported".to_string();
                            }
                        }
                    }
                    if ui.button("Import Backup (.json)").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                            if let Ok(raw) = fs::read_to_string(&p) {
                                if let Ok(loaded) = serde_json::from_str::<PersistedState>(&raw) {
                                    app.state = loaded;
                                    save_state(&app.state);
                                    app.status_note = "Backup restored".to_string();
                                }
                            }
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Check for Updates").clicked() {
                        let tx = app.tx.clone();
                        let ctx_c = ctx.clone();
                        app.update_status = "Checking GitHub...".to_string();
                        thread::spawn(move || check_updates(&tx, &ctx_c));
                    }
                    if !app.update_status.is_empty() {
                        let col = if app.update_status.contains("Up to date") { GREEN } else { th.c_accent() };
                        ui.label(RichText::new(&app.update_status).size(11.5).color(col));
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Reset Stats").clicked() {
                        app.state.stats = Default::default();
                        save_state(&app.state);
                    }
                    if ui.button("Close").clicked() {
                        app.show_stats_modal = false;
                    }
                });
            });
    }
}

fn stat_card(ui: &mut egui::Ui, th: &ThemeConfig, label: &str, val: &str) {
    let f = egui::Frame::NONE
        .fill(th.c_elevated())
        .stroke(Stroke::new(1.0, th.c_border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(10.0);
    f.show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(11.0).color(th.c_muted()));
            ui.label(RichText::new(val).size(16.0).strong().color(th.c_text()));
        });
    });
}

fn check_updates(tx: &Sender<WorkerEvent>, ctx: &egui::Context) {
    let cur = env!("CARGO_PKG_VERSION");
    let res = ureq::get("https://api.github.com/repos/r2d2pr/yokahu/releases/latest")
        .header("User-Agent", "Yokahu")
        .call();
    let msg = match res {
        Ok(mut r) => {
            let raw = r.body_mut().read_to_string().unwrap_or_default();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
                let clean = tag.trim_start_matches('v');
                if clean == cur || clean.is_empty() {
                    format!("Up to date (v{})", cur)
                } else {
                    format!("New release available: {} (current v{})", tag, cur)
                }
            } else {
                "Could not parse release info".to_string()
            }
        }
        Err(e) => format!("Update check failed: {}", e),
    };
    let _ = tx.send(WorkerEvent::UpdateStatus(msg));
    ctx.request_repaint();
}

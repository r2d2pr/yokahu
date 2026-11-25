use crate::theme::ThemeConfig;
use eframe::egui::{self, text::LayoutJob, Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, TextFormat};

pub fn render_markdown(ui: &mut egui::Ui, ctx: &egui::Context, th: &ThemeConfig, text: &str) {
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let before = &rest[..start];
        if !before.trim().is_empty() {
            render_blocks(ui, th, before);
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
            .fill(if th.is_light { Color32::from_rgb(242, 244, 248) } else { Color32::from_rgb(13, 17, 23) })
            .stroke(Stroke::new(1.0, th.c_border()))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(10.0);
        box_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if lang.is_empty() { "code" } else { lang })
                        .size(11.0)
                        .color(th.c_muted()),
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
                    .color(th.c_text()),
            );
        });
        ui.add_space(4.0);
        rest = next_rest;
    }
    if !rest.trim().is_empty() {
        render_blocks(ui, th, rest);
    }
}

fn render_blocks(ui: &mut egui::Ui, th: &ThemeConfig, block: &str) {
    for line in block.lines() {
        let trimmed = line.trim_start();
        if let Some(h) = trimmed.strip_prefix("### ") {
            ui.add_space(4.0);
            ui.label(RichText::new(h).size(15.0).strong().color(th.c_text()));
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            ui.add_space(6.0);
            ui.label(RichText::new(h).size(16.5).strong().color(th.c_text()));
        } else if let Some(h) = trimmed.strip_prefix("# ") {
            ui.add_space(6.0);
            ui.label(RichText::new(h).size(18.5).strong().color(th.c_text()));
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new("\u{2022}").color(th.c_accent()));
                ui.label(build_inline_job(th, item));
            });
        } else if let Some((num, rest)) = parse_numbered(trimmed) {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(format!("{}.", num)).color(th.c_accent()));
                ui.label(build_inline_job(th, rest));
            });
        } else if trimmed.is_empty() {
            ui.add_space(4.0);
        } else {
            ui.label(build_inline_job(th, line));
        }
    }
}

fn parse_numbered(s: &str) -> Option<(&str, &str)> {
    let dot = s.find(". ")?;
    let prefix = &s[..dot];
    if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
        Some((prefix, &s[dot + 2..]))
    } else {
        None
    }
}

fn build_inline_job(th: &ThemeConfig, line: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base_fmt = TextFormat {
        font_id: FontId::new(14.0, FontFamily::Proportional),
        color: th.c_text(),
        ..Default::default()
    };
    let bold_fmt = TextFormat {
        font_id: FontId::new(14.0, FontFamily::Proportional),
        color: th.c_text(),
        ..Default::default()
    };
    let code_fmt = TextFormat {
        font_id: FontId::new(12.5, FontFamily::Monospace),
        color: th.c_accent(),
        background: th.c_elevated(),
        ..Default::default()
    };

    let bytes = line.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(end) = line[i + 1..].find('`') {
                if i > plain_start {
                    job.append(&line[plain_start..i], 0.0, base_fmt.clone());
                }
                job.append(&line[i + 1..i + 1 + end], 0.0, code_fmt.clone());
                i = i + 1 + end + 1;
                plain_start = i;
                continue;
            }
        } else if line[i..].starts_with("**") {
            if let Some(end) = line[i + 2..].find("**") {
                if i > plain_start {
                    job.append(&line[plain_start..i], 0.0, base_fmt.clone());
                }
                // Highlight bold with accent-tinted or crisp bright weight via underline/color
                let mut bf = bold_fmt.clone();
                bf.italics = false;
                bf.color = if th.is_light { Color32::BLACK } else { Color32::WHITE };
                job.append(&line[i + 2..i + 2 + end], 0.0, bf);
                i = i + 2 + end + 2;
                plain_start = i;
                continue;
            }
        } else if bytes[i] == b'*' && !line[i..].starts_with("**") {
            if let Some(end) = line[i + 1..].find('*') {
                if i > plain_start {
                    job.append(&line[plain_start..i], 0.0, base_fmt.clone());
                }
                let mut it = base_fmt.clone();
                it.italics = true;
                job.append(&line[i + 1..i + 1 + end], 0.0, it);
                i = i + 1 + end + 1;
                plain_start = i;
                continue;
            }
        }
        // Advance by one UTF-8 char
        let ch_len = line[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        i += ch_len;
    }

    if plain_start < line.len() {
        job.append(&line[plain_start..], 0.0, base_fmt);
    }
    job
}

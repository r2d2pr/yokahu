use crate::theme::ThemeConfig;
use eframe::egui::{self, text::LayoutJob, Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, TextFormat};

pub fn render_markdown(ui: &mut egui::Ui, ctx: &egui::Context, th: &ThemeConfig, text: &str) -> Option<String> {
    let mut apply_code: Option<String> = None;
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
                    if ui.small_button("Apply to File...").clicked() {
                        apply_code = Some(code.trim_end().to_string());
                    }
                });
            });
            ui.separator();
            ui.label(highlight_code_job(th, lang, code.trim_end()));
        });
        ui.add_space(4.0);
        rest = next_rest;
    }
    if !rest.trim().is_empty() {
        render_blocks(ui, th, rest);
    }
    apply_code
}

fn highlight_code_job(th: &ThemeConfig, lang: &str, code: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font = FontId::new(12.5, FontFamily::Monospace);
    let plain = TextFormat { font_id: font.clone(), color: th.c_text(), ..Default::default() };
    let kw_col = if th.is_light { Color32::from_rgb(175, 0, 120) } else { Color32::from_rgb(255, 123, 114) };
    let str_col = if th.is_light { Color32::from_rgb(10, 120, 60) } else { Color32::from_rgb(165, 214, 255) };
    let cmt_col = th.c_muted();
    let num_col = if th.is_light { Color32::from_rgb(0, 92, 197) } else { Color32::from_rgb(121, 192, 255) };

    let kw_fmt = TextFormat { font_id: font.clone(), color: kw_col, ..Default::default() };
    let str_fmt = TextFormat { font_id: font.clone(), color: str_col, ..Default::default() };
    let cmt_fmt = TextFormat { font_id: font.clone(), color: cmt_col, italics: true, ..Default::default() };
    let num_fmt = TextFormat { font_id: font, color: num_col, ..Default::default() };

    let is_py_or_sh = matches!(lang.to_ascii_lowercase().as_str(), "py" | "python" | "sh" | "bash" | "powershell" | "ps1" | "yaml" | "toml");
    let keywords: &[&str] = &[
        "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "match", "if", "else",
        "for", "while", "loop", "return", "async", "await", "def", "class", "import", "from",
        "const", "function", "var", "true", "false", "null", "None", "Some", "Ok", "Err", "self",
    ];

    for (line_idx, line) in code.lines().enumerate() {
        if line_idx > 0 {
            job.append("\n", 0.0, plain.clone());
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if line[i..].starts_with("//") || (is_py_or_sh && bytes[i] == b'#') {
                job.append(&line[i..], 0.0, cmt_fmt.clone());
                break;
            } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                let q = bytes[i];
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == q {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                let end = j.min(bytes.len());
                job.append(&line[i..end], 0.0, str_fmt.clone());
                i = end;
            } else if bytes[i].is_ascii_digit() {
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_') {
                    j += 1;
                }
                job.append(&line[i..j], 0.0, num_fmt.clone());
                i = j;
            } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let word = &line[i..j];
                let fmt = if keywords.contains(&word) { kw_fmt.clone() } else { plain.clone() };
                job.append(word, 0.0, fmt);
                i = j;
            } else {
                let ch_len = line[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                job.append(&line[i..i + ch_len], 0.0, plain.clone());
                i += ch_len;
            }
        }
    }
    job
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
                ui.label(RichText::new("-").strong().color(th.c_accent()));
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
                let mut bf = base_fmt.clone();
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
        let ch_len = line[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        i += ch_len;
    }

    if plain_start < line.len() {
        job.append(&line[plain_start..], 0.0, base_fmt);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numbered_lists() {
        assert_eq!(parse_numbered("1. hello"), Some(("1", "hello")));
        assert_eq!(parse_numbered("12. item"), Some(("12", "item")));
        assert_eq!(parse_numbered("abc. nope"), None);
    }

    #[test]
    fn inline_job_handles_code_and_bold() {
        let th = ThemeConfig::default();
        let job = build_inline_job(&th, "Use `cargo test` for **fast** checks");
        assert!(job.sections.len() >= 4);
    }

    #[test]
    fn syntax_highlight_splits_keywords_and_strings() {
        let th = ThemeConfig::default();
        let job = highlight_code_job(&th, "rust", "let x = \"hi\"; // note\n");
        assert!(job.sections.len() >= 4);
    }
}

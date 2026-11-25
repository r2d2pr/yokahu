use crate::mcp_client;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

pub fn discover_skills(workspace: Option<&Path>, include_antigravity: bool) -> Vec<SkillEntry> {
    let mut roots = Vec::new();
    roots.push(mcp_client::app_data_dir().join("skills"));
    if let Some(ws) = workspace {
        roots.push(ws.join(".yokahu").join("skills"));
    }
    if include_antigravity {
        if let Some(home) = home_dir() {
            roots.push(home.join(".gemini").join("config").join("skills"));
        }
    }

    let mut skills = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let skill_md = entry.path().join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&skill_md) {
                let (name, desc) = parse_frontmatter(
                    &text,
                    &entry.file_name().to_string_lossy(),
                );
                if !skills.iter().any(|s: &SkillEntry| s.name == name) {
                    skills.push(SkillEntry {
                        name,
                        description: desc,
                        path: skill_md,
                    });
                }
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
}

fn parse_frontmatter(text: &str, fallback_name: &str) -> (String, String) {
    let mut name = fallback_name.to_string();
    let mut desc = String::new();
    let mut lines = text.lines();
    if lines.next().map(|l| l.trim()) != Some("---") {
        return (name, desc);
    }
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(v) = t.strip_prefix("name:") {
            let clean = v.trim().trim_matches('"').trim_matches('\'');
            if !clean.is_empty() {
                name = clean.to_string();
            }
        } else if let Some(v) = t.strip_prefix("description:") {
            desc = v.trim().trim_matches('"').trim_matches('\'').to_string();
        }
    }
    (name, desc)
}

pub fn load_workspace_rules(workspace: Option<&Path>) -> String {
    let Some(ws) = workspace else {
        return String::new();
    };
    let mut out = String::new();
    for fname in [".yokahu.md", "AGENTS.md"] {
        let p = ws.join(fname);
        if let Ok(content) = fs::read_to_string(&p) {
            let trimmed: String = content.chars().take(10000).collect();
            if !trimmed.trim().is_empty() {
                out.push_str(&format!("\n\n--- Workspace Rules ({}) ---\n{}", fname, trimmed));
            }
        }
    }
    out
}

pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/plan", "Create a step-by-step plan and track it with update_steps"),
    ("/grill-me", "Interview me one question at a time via ask_question"),
    ("/review", "Review workspace code for bugs, security, and complexity"),
    ("/compact", "Summarize older conversation turns to free context window"),
];

pub fn expand_slash_command(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("/plan") {
        return Some(format!(
            "[SLASH /plan] Break this task into clear steps using `update_steps` first, then execute and verify each step. Request: {}",
            rest.trim()
        ));
    }
    if let Some(rest) = trimmed.strip_prefix("/grill-me") {
        return Some(format!(
            "[SLASH /grill-me] Interview the user about every design decision one question at a time using the `ask_question` tool until you reach a shared understanding. Recommend an option for each question. Topic: {}",
            rest.trim()
        ));
    }
    if let Some(rest) = trimmed.strip_prefix("/review") {
        return Some(format!(
            "[SLASH /review] Inspect the relevant files using `find_files`, `search_files`, and `read_file`, then review strictly for correctness, security, edge cases, and unnecessary complexity. Target: {}",
            rest.trim()
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_parses_name_and_description() {
        let md = "---\nname: my-skill\ndescription: Does cool things\n---\n# Body";
        let (n, d) = parse_frontmatter(md, "fallback");
        assert_eq!(n, "my-skill");
        assert_eq!(d, "Does cool things");
    }

    #[test]
    fn slash_command_expands() {
        let out = expand_slash_command("/plan build auth").unwrap();
        assert!(out.contains("update_steps"));
        assert!(out.contains("build auth"));
    }
}

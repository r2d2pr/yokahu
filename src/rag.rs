use crate::mcp_client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct RagChunk {
    pub path: String,
    pub start_line: usize,
    pub text: String,
    pub embedding: Vec<f32>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct RagIndex {
    pub model: String,
    pub chunks: Vec<RagChunk>,
}

pub fn index_path() -> PathBuf {
    mcp_client::app_data_dir().join("rag_index.json")
}

pub fn load_index() -> RagIndex {
    fs::read_to_string(index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_index(idx: &RagIndex) {
    let p = index_path();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(idx) {
        let _ = fs::write(p, json);
    }
}

pub fn chunk_text(rel_path: &str, content: &str, lines_per_chunk: usize) -> Vec<(String, usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let step = (lines_per_chunk / 2).max(1);
    let mut i = 0;
    while i < lines.len() {
        let end = (i + lines_per_chunk).min(lines.len());
        let snippet = lines[i..end].join("\n");
        if !snippet.trim().is_empty() {
            out.push((rel_path.to_string(), i + 1, snippet));
        }
        if end == lines.len() {
            break;
        }
        i += step;
    }
    out
}

fn embed_batch(base_url: &str, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let url = format!("{}/api/embed", base_url.trim().trim_end_matches('/'));
    let body = json!({ "model": model, "input": inputs }).to_string();
    let mut resp = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send(body.as_str())
        .map_err(|e| format!("Embed error ({}): {}", model, e))?;
    let raw = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let arr = v
        .get("embeddings")
        .and_then(|e| e.as_array())
        .ok_or_else(|| "Missing 'embeddings' in Ollama response".to_string())?;
    let mut vecs = Vec::with_capacity(arr.len());
    for item in arr {
        let emb: Vec<f32> = item
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_f64().map(|n| n as f32)).collect())
            .unwrap_or_default();
        vecs.push(emb);
    }
    Ok(vecs)
}

pub fn index_workspace(base_url: &str, model: &str, workspace: &Path) -> Result<usize, String> {
    let mut raw_chunks = Vec::new();
    collect_chunks(workspace, workspace, &mut raw_chunks);
    if raw_chunks.is_empty() {
        return Ok(0);
    }

    let mut indexed = Vec::with_capacity(raw_chunks.len());
    for batch in raw_chunks.chunks(16) {
        let texts: Vec<String> = batch.iter().map(|(_, _, t)| t.clone()).collect();
        let vecs = embed_batch(base_url, model, &texts)?;
        for ((path, start_line, text), embedding) in batch.iter().zip(vecs.into_iter()) {
            indexed.push(RagChunk {
                path: path.clone(),
                start_line: *start_line,
                text: text.clone(),
                embedding,
            });
        }
    }
    let count = indexed.len();
    save_index(&RagIndex {
        model: model.to_string(),
        chunks: indexed,
    });
    Ok(count)
}

pub fn search(base_url: &str, query: &str, top_k: usize) -> Result<Vec<(f32, RagChunk)>, String> {
    let idx = load_index();
    if idx.chunks.is_empty() {
        return Err("RAG index is empty. Click 'Index Workspace' in Files tab first.".to_string());
    }
    let q_vecs = embed_batch(base_url, &idx.model, &[query.to_string()])?;
    let q = q_vecs.first().ok_or("Empty query embedding")?;
    let mut scored: Vec<(f32, RagChunk)> = idx
        .chunks
        .into_iter()
        .map(|c| (cosine_similarity(q, &c.embedding), c))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    Ok(scored)
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

fn collect_chunks(root: &Path, dir: &Path, out: &mut Vec<(String, usize, String)>) {
    if out.len() >= 400 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= 400 {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        if path.is_dir() {
            collect_chunks(root, &path, out);
        } else if let Ok(meta) = fs::metadata(&path) {
            if meta.len() <= 128 * 1024 {
                if let Ok(text) = fs::read_to_string(&path) {
                    let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
                    out.extend(chunk_text(&rel, &text, 40));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_works() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-5);
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) - 0.0).abs() < 1e-5);
    }

    #[test]
    fn chunk_text_splits_with_overlap() {
        let content = (1..=60).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let chunks = chunk_text("a.rs", &content, 40);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].1, 1);
        assert_eq!(chunks[1].1, 21);
    }
}

//! `base graph extract` — LLM semantic extraction over a doc corpus into the
//! workspace graph. Walks markdown, content-hash caches (re-extract only changed
//! docs — each LLM call is ~20-30s), prompts the model for concepts + edges, and
//! upserts them via `crud::semantic` into per-document named graphs.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::NamespaceConfig;
use crate::crud::semantic::{self, Extraction};

const MAX_DOC_CHARS: usize = 8000;

pub struct Report {
    pub extracted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub concepts: usize,
    pub edges: usize,
}

pub fn run(cwd: &Path, ns: &NamespaceConfig, target: &Path, model: Option<&str>) -> Result<Report> {
    let cache_dir = target.join(".base-semantic-cache");
    let docs = collect_docs(target);
    println!("Semantic extraction: {} markdown docs under {}", docs.len(), target.display());

    let mut r = Report { extracted: 0, skipped: 0, failed: 0, concepts: 0, edges: 0 };

    for doc in &docs {
        let text = std::fs::read_to_string(doc).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let hash = content_hash(&text);
        let marker = cache_dir.join(&hash);
        if marker.exists() {
            r.skipped += 1;
            continue;
        }

        let rel = doc.strip_prefix(target).unwrap_or(doc).display().to_string();
        let prompt = build_prompt(&rel, &text);
        let resp = match crate::llm::complete(&prompt, model) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  ! {rel}: {e}");
                r.failed += 1;
                continue;
            }
        };
        let ex = match parse_extraction(&resp) {
            Some(e) => e,
            None => {
                eprintln!("  ! {rel}: could not parse LLM response as extraction JSON");
                r.failed += 1;
                continue;
            }
        };

        if let Err(e) = semantic::upsert(cwd, ns, &rel, &ex) {
            eprintln!("  ! {rel}: graph write failed: {e}");
            r.failed += 1;
            continue;
        }
        r.extracted += 1;
        r.concepts += ex.concepts.len();
        r.edges += ex.edges.len();
        let _ = std::fs::create_dir_all(&cache_dir);
        let _ = std::fs::write(&marker, b"");
        println!("  ✓ {rel} ({} concepts, {} edges)", ex.concepts.len(), ex.edges.len());
    }

    Ok(r)
}

fn content_hash(text: &str) -> String {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Recursively collect markdown docs, skipping dependency/build/generated dirs.
fn collect_docs(root: &Path) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        "node_modules", ".git", "target", "dist", "build", "vendor",
        ".base", ".base-ast", ".base-ast-cache", ".base-semantic-cache", ".paul",
    ];
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !SKIP.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn build_prompt(doc_path: &str, text: &str) -> String {
    let body: String = text.chars().take(MAX_DOC_CHARS).collect();
    format!(
        "You are a knowledge-graph extractor. Read the document and extract its key concepts and the relationships between them.\n\n\
         Return ONLY a JSON object — no prose, no markdown fences — in exactly this shape:\n\
         {{\"concepts\":[{{\"name\":\"<short canonical name>\",\"type\":\"<concept|component|decision|process|entity|tool>\",\"summary\":\"<one sentence>\"}}],\
         \"edges\":[{{\"from\":\"<concept name>\",\"to\":\"<concept name>\",\"relation\":\"<rationale_for|depends_on|part_of|relates_to|contrasts_with|produces>\",\"confidence\":<0.0-1.0>,\"provenance\":\"<EXTRACTED|INFERRED|AMBIGUOUS>\"}}]}}\n\n\
         Rules:\n\
         - Concepts are the document's substantive ideas, not section headers.\n\
         - Every edge \"from\"/\"to\" must exactly match a concept \"name\".\n\
         - provenance: EXTRACTED = stated in text; INFERRED = implied; AMBIGUOUS = uncertain.\n\
         - Keep it tight: the 5-15 most important concepts.\n\n\
         DOCUMENT ({doc_path}):\n{body}"
    )
}

/// Tolerant parse: strip markdown fences, take the outermost JSON object.
fn parse_extraction(resp: &str) -> Option<Extraction> {
    let trimmed = resp.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&trimmed[start..=end]).ok()
}

pub fn format_report(r: &Report) -> String {
    format!(
        "\nSemantic extraction complete: {} docs extracted, {} skipped (cached), {} failed → {} concepts, {} edges\n",
        r.extracted, r.skipped, r.failed, r.concepts, r.edges
    )
}

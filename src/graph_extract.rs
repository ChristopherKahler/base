//! `base graph extract` — LLM semantic extraction over a doc corpus into the
//! workspace graph. Walks markdown, content-hash caches (re-extract only changed
//! docs — each LLM call is ~20-30s), prompts the model for concepts + edges, and
//! upserts them via `crud::semantic` into per-document named graphs.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::NamespaceConfig;
use crate::crud::semantic::{self, Extraction};
use crate::multimodal::{self, Modality};

const MAX_DOC_CHARS: usize = 8000;

pub struct Report {
    pub extracted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub concepts: usize,
    pub edges: usize,
    /// Non-markdown files found but NOT processed because multimodal is off.
    pub multimodal_skipped: usize,
}

/// One ingestible source plus the modality that decides how it's turned into text.
struct Source {
    path: PathBuf,
    modality: Modality,
}

pub fn run(
    cwd: &Path,
    ns: &NamespaceConfig,
    target: &Path,
    model: Option<&str>,
    multimodal_enabled: bool,
) -> Result<Report> {
    let cache_dir = target.join(".base-semantic-cache");
    let all = collect_sources(target);

    let mut r = Report { extracted: 0, skipped: 0, failed: 0, concepts: 0, edges: 0, multimodal_skipped: 0 };

    // Split markdown (always processed, zero deps) from multimodal sources.
    let (sources, deferred): (Vec<Source>, Vec<Source>) = all
        .into_iter()
        .partition(|s| s.modality == Modality::Markdown || multimodal_enabled);
    r.multimodal_skipped = deferred.len();

    let md_count = sources.iter().filter(|s| s.modality == Modality::Markdown).count();
    let mm_count = sources.len() - md_count;
    println!(
        "Semantic extraction: {} markdown + {} multimodal source(s) under {}",
        md_count, mm_count, target.display()
    );

    // Lazy, one-time dependency bootstrap — only for the modalities actually present.
    if mm_count > 0 {
        let mods: BTreeSet<Modality> = sources.iter().map(|s| s.modality).collect();
        multimodal::ensure_deps(&mods)?;
    }

    for src in &sources {
        let doc = &src.path;
        // Hash the raw source bytes so the expensive step (transcription / vision /
        // pdf parse) is cached too — re-running skips unchanged binaries.
        let Ok(bytes) = std::fs::read(doc) else { continue };
        let hash = content_hash_bytes(&bytes);
        let marker = cache_dir.join(&hash);
        if marker.exists() {
            r.skipped += 1;
            continue;
        }

        let rel = doc.strip_prefix(target).unwrap_or(doc).display().to_string();
        let text = if src.modality == Modality::Markdown {
            String::from_utf8_lossy(&bytes).to_string()
        } else {
            match multimodal::extract_text(doc, src.modality, model) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("  ! {rel} ({}): {e}", src.modality.label());
                    r.failed += 1;
                    continue;
                }
            }
        };
        if text.trim().is_empty() {
            // Nothing usable extracted; mark done so we don't retry a blank source.
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::write(&marker, b"");
            continue;
        }

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

fn content_hash_bytes(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Recursively collect ingestible sources (any classifiable modality), skipping
/// dependency/build/generated dirs. The caller decides which modalities to act on.
fn collect_sources(root: &Path) -> Vec<Source> {
    const SKIP: &[&str] = &[
        "node_modules", ".git", "target", "dist", "build", "vendor",
        ".base", ".base-ast", ".base-ast-cache", ".base-semantic-cache", ".paul",
    ];
    let mut out: Vec<Source> = Vec::new();
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
            } else if let Some(modality) = multimodal::classify(&path) {
                out.push(Source { path, modality });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
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
    let mut s = format!(
        "\nSemantic extraction complete: {} docs extracted, {} skipped (cached), {} failed → {} concepts, {} edges\n",
        r.extracted, r.skipped, r.failed, r.concepts, r.edges
    );
    if r.multimodal_skipped > 0 {
        s.push_str(&format!(
            "Note: {} non-markdown file(s) skipped — multimodal ingest is OFF. \
             Enable with `base config set multimodal.enabled true` (or pass --multimodal). \
             No sudo needed: PDF/image are in-process; audio/video install whisper+ffmpeg via pip --user once.\n",
            r.multimodal_skipped
        ));
    }
    s
}

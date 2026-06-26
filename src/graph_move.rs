//! `base graph move` — transfer a subgraph between workspace named graphs.
//!
//! A workspace graph is an N-Quads file where every quad's 4th term is the
//! workspace stamp `<{ns.uri}graph/ws/{slug}>`. Moving a subgraph from workspace A
//! to B means three things, in this order of importance:
//!
//! 1. **Named-graph rewrite (THE correctness property).** Each moved line's 4th
//!    term MUST be rewritten `graph/ws/A → graph/ws/B`. Skip it and the lines land
//!    present-but-invisible: `base recall` returns nothing, no error — a silent
//!    failure. [`rewrite_graph`] is the pure, idempotent primitive that does this.
//! 2. **Atomicity.** snapshot-both → write-dest → write-source → health-gate. On any
//!    failure both graphs roll back from the snapshot. Never a duplicate (forgot the
//!    source removal) or an orphan (forgot the rewrite).
//! 3. **AST is regenerated, not moved.** `--no-ast` excludes `code#`-namespace
//!    entities and `codemap/` pointers; the AST map rebuilds at the destination.
//!
//! The engine is line-oriented (matching the proven hand-run `grep | sed | cat`
//! golden path) but every write is re-parsed for validity before the atomic rename,
//! so a move can never leave a corrupt graph behind. Selection is subject-scoped:
//! entities whose IRI *is* the thing move; entities that merely *reference* it become
//! dangling incoming edges (left + counted, never silently dropped).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::{NamespaceConfig, WorkspaceEntry};
use crate::store::{self, GraphHealth};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

// ─── Selection ──────────────────────────────────────────────────────────────

/// How to pick the subgraph to move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// A single node by full IRI: its own triples + outgoing edges.
    Node(String),
    /// A domain by name: the domain node + entities attached to it (slug-convention
    /// IRIs like `task/<slug>.*` plus anything that links to `domain/<slug>`).
    Domain(String),
    /// Every subject whose IRI contains this substring.
    Prefix(String),
}

impl Selector {
    /// Parse a `--select` value: `domain:X` | `prefix:X` | `node:IRI` | a bare full IRI (→ Node).
    pub fn parse(s: &str) -> Result<Self> {
        if let Some(rest) = s.strip_prefix("domain:") {
            return Ok(Selector::Domain(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix("prefix:") {
            return Ok(Selector::Prefix(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix("node:") {
            return Ok(Selector::Node(rest.to_string()));
        }
        if s.contains("://") {
            return Ok(Selector::Node(s.to_string()));
        }
        bail!(
            "--select must be one of: node:<iri>, domain:<name>, prefix:<str>, or a full node IRI (got '{s}')"
        );
    }
}

// ─── Pure N-Quads line primitives (the unit-tested core) ────────────────────

/// Subject IRI of an N-Quads line. `None` for a blank-node subject or a malformed
/// line. Subjects in a base graph are always IRIs.
pub fn parse_subject(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('<')?;
    let end = rest.find('>')?;
    Some(&rest[..end])
}

/// The named-graph IRI (4th term) of an N-Quads quad line. `None` if the line is a
/// triple (no graph) or malformed. NOTE: for a bare triple this returns the OBJECT
/// token, so callers must compare the result against a specific expected graph IRI
/// rather than trusting it as "the graph" unconditionally.
pub fn parse_graph(line: &str) -> Option<&str> {
    let body = line.trim_end().strip_suffix('.')?.trim_end();
    let tok = body.rsplit([' ', '\t']).next()?;
    let inner = tok.strip_prefix('<')?.strip_suffix('>')?;
    Some(inner)
}

/// Rewrite ONLY the 4th (named-graph) term of a quad line to `to_graph`, preserving
/// subject/predicate/object byte-for-byte. Idempotent (re-running with the same
/// target is a no-op). `None` if the line has no `<...>` graph term / `.` terminator
/// to rewrite. The graph term is the final `<...>` token; an object/predicate `<` is
/// always earlier, and a literal `<` can only appear before the graph, so the LAST
/// `<` is the graph's opening bracket.
pub fn rewrite_graph(line: &str, to_graph: &str) -> Option<String> {
    let trimmed = line.trim_end();
    if !trimmed.ends_with('.') {
        return None;
    }
    let lt = trimmed.rfind('<')?;
    let prefix = &trimmed[..lt]; // "<s> <p> <o> " incl. trailing space
    Some(format!("{prefix}<{to_graph}> ."))
}

/// Every `<...>` inner IRI in a line, left to right. Used for edge analysis. (A `<...>`
/// inside a string literal would be captured too — acceptable for the informational
/// dangling-edge count.)
fn iri_tokens(line: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let mut i = 0;
    let bytes = line.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(off) = line[i + 1..].find('>') {
                v.push(&line[i + 1..i + 1 + off]);
                i = i + 1 + off + 1;
                continue;
            }
            break;
        }
        i += 1;
    }
    v
}

/// rdf:type object (local IRI) for a type line, else `None`.
fn type_local(line: &str, ns: &NamespaceConfig) -> Option<String> {
    if !line.contains(RDF_TYPE) {
        return None;
    }
    let after = line.split(RDF_TYPE).nth(1)?.trim_start();
    let inner = after.strip_prefix('<')?;
    let t = inner.split('>').next()?;
    Some(local_part(t, &ns.uri).to_string())
}

fn local_part<'a>(iri: &'a str, ns_uri: &str) -> &'a str {
    iri.strip_prefix(ns_uri).unwrap_or(iri)
}

/// AST-derived subject: a `code#`-namespace entity, or a `codemap/` map pointer.
/// These rebuild at the destination from the code, so a move excludes them.
pub fn is_ast_subject(iri: &str, ns: &NamespaceConfig) -> bool {
    if local_part(iri, &ns.uri).starts_with("codemap/") {
        return true;
    }
    // Default `ns.uri` is `…/ontology#`; AST lives under the sibling `…/code#`.
    if let Some(prefix) = ns.uri.strip_suffix("ontology#") {
        let code_ns = format!("{prefix}code#");
        if iri.starts_with(&code_ns) {
            return true;
        }
    }
    false
}

// ─── Spec / report ──────────────────────────────────────────────────────────

/// Resolved endpoints of a move: the two graph files + their named-graph IRIs.
#[derive(Debug, Clone)]
pub struct MoveSpec {
    pub source_path: PathBuf,
    pub dest_path: PathBuf,
    pub source_graph: String,
    pub dest_graph: String,
    pub source_ws: String,
    pub dest_ws: String,
    pub no_ast: bool,
}

/// Outcome of a planned (`dry_run`) or applied move.
#[derive(Debug)]
pub struct MoveReport {
    pub applied: bool,
    pub source_ws: String,
    pub dest_ws: String,
    pub source_path: String,
    pub dest_path: String,
    pub source_graph: String,
    pub dest_graph: String,
    pub subjects: usize,
    pub moved_lines: usize,
    pub by_type: BTreeMap<String, usize>,
    pub sample: Vec<String>,
    pub ast_excluded: usize,
    pub dangling_incoming: usize,
    pub source_backup: Option<String>,
    pub dest_backup: Option<String>,
}

// ─── Selector resolution ─────────────────────────────────────────────────────

/// Resolve a [`Selector`] to the set of subject IRIs it selects, by scanning the
/// source graph file. Subject-scoped: a `relatedTo`/`hasDomain` edge pointing AT the
/// domain attaches its subject (catches notes whose IRI doesn't carry the slug).
pub fn resolve_selector(
    source_path: &Path,
    selector: &Selector,
    source_graph: &str,
    ns: &NamespaceConfig,
) -> Result<HashSet<String>> {
    let content = std::fs::read_to_string(source_path)
        .with_context(|| format!("reading source graph {}", source_path.display()))?;
    let mut matched: HashSet<String> = HashSet::new();

    match selector {
        Selector::Node(iri) => {
            matched.insert(iri.clone());
        }
        Selector::Prefix(sub) => {
            for line in content.lines() {
                if parse_graph(line) != Some(source_graph) {
                    continue;
                }
                if let Some(s) = parse_subject(line)
                    && s.contains(sub.as_str())
                {
                    matched.insert(s.to_string());
                }
            }
        }
        Selector::Domain(name) => {
            let slug = crate::crud::slugify(name);
            let domain_iri = crate::crud::build_iri(ns, "domain", &slug);
            matched.insert(domain_iri.clone());

            // Slug-convention IRIs that belong to this domain/project.
            let conv = [
                format!("{}decision/{}.", ns.uri, slug),
                format!("{}rule/{}/", ns.uri, slug),
                format!("{}rule/{}.", ns.uri, slug),
                format!("{}task/{}.", ns.uri, slug),
                format!("{}milestone/{}.", ns.uri, slug),
                format!("{}project/{}", ns.uri, slug),
            ];
            let domain_obj = format!("<{domain_iri}>");

            for line in content.lines() {
                if parse_graph(line) != Some(source_graph) {
                    continue;
                }
                let Some(s) = parse_subject(line) else { continue };
                if conv.iter().any(|c| s.starts_with(c.as_str())) {
                    matched.insert(s.to_string());
                }
                // A subject that points AT the domain (note relatedTo, project hasDomain).
                if s != domain_iri && line.contains(&domain_obj) {
                    matched.insert(s.to_string());
                }
            }
        }
    }
    Ok(matched)
}

// ─── Move ────────────────────────────────────────────────────────────────────

/// Resolve the selector then move. The one-call entry point for `base graph move`.
pub fn graph_move(
    spec: &MoveSpec,
    selector: &Selector,
    ns: &NamespaceConfig,
    dry_run: bool,
) -> Result<MoveReport> {
    let subjects = resolve_selector(&spec.source_path, selector, &spec.source_graph, ns)?;
    graph_move_subjects(spec, &subjects, ns, dry_run)
}

/// Move an explicit set of subject IRIs (the variant `base project move` composes on).
pub fn graph_move_subjects(
    spec: &MoveSpec,
    subjects: &HashSet<String>,
    ns: &NamespaceConfig,
    dry_run: bool,
) -> Result<MoveReport> {
    // Health gates — never read/rewrite a broken graph.
    match store::graph_health(&spec.source_path) {
        GraphHealth::Healthy => {}
        GraphHealth::Missing => bail!("source graph not found: {}", spec.source_path.display()),
        GraphHealth::Unhealthy { reason, .. } => {
            bail!("source graph unhealthy ({reason}) — run `base doctor --repair` first")
        }
    }
    match store::graph_health(&spec.dest_path) {
        GraphHealth::Healthy | GraphHealth::Missing => {}
        GraphHealth::Unhealthy { reason, .. } => {
            bail!("destination graph unhealthy ({reason}) — run `base doctor --repair` first")
        }
    }

    let content = std::fs::read_to_string(&spec.source_path)
        .with_context(|| format!("reading source graph {}", spec.source_path.display()))?;

    // Apply --no-ast to the subject set (report how many were excluded).
    let effective: HashSet<String> = subjects
        .iter()
        .filter(|s| !(spec.no_ast && is_ast_subject(s, ns)))
        .cloned()
        .collect();
    let ast_excluded = subjects.len().saturating_sub(effective.len());

    // Partition the source file: selected (moves) vs kept (stays). Lines from any
    // other named graph are always kept untouched (defensive).
    let mut selected: Vec<&str> = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut sample: Vec<String> = Vec::new();
    let mut seen_sample: HashSet<&str> = HashSet::new();
    let mut dangling_incoming = 0usize;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let in_src = parse_graph(line) == Some(spec.source_graph.as_str());
        let subj = parse_subject(line);
        let is_selected = in_src && subj.map(|s| effective.contains(s)).unwrap_or(false);

        if is_selected {
            selected.push(line);
            if let Some(t) = type_local(line, ns) {
                *by_type.entry(t).or_insert(0) += 1;
            }
            if let Some(s) = subj
                && seen_sample.insert(s)
                && sample.len() < 8
            {
                sample.push(local_part(s, &ns.uri).to_string());
            }
        } else {
            kept.push(line);
            // Dangling incoming: an edge whose object is a moved subject but whose
            // own subject stays behind. Standard quad edge has exactly [s, p, o, g].
            if in_src {
                let toks = iri_tokens(line);
                if toks.len() == 4
                    && !effective.contains(toks[0])
                    && effective.contains(toks[2])
                {
                    dangling_incoming += 1;
                }
            }
        }
    }

    let mut report = MoveReport {
        applied: false,
        source_ws: spec.source_ws.clone(),
        dest_ws: spec.dest_ws.clone(),
        source_path: spec.source_path.display().to_string(),
        dest_path: spec.dest_path.display().to_string(),
        source_graph: spec.source_graph.clone(),
        dest_graph: spec.dest_graph.clone(),
        subjects: effective.len(),
        moved_lines: selected.len(),
        by_type,
        sample,
        ast_excluded,
        dangling_incoming,
        source_backup: None,
        dest_backup: None,
    };

    // Dry-run, or nothing to move → no writes. (An empty move is a valid no-op, the
    // basis of idempotency: a second run finds nothing left.)
    if dry_run || selected.is_empty() {
        report.applied = !dry_run; // a no-op move is "applied" (it succeeded, moved 0)
        return Ok(report);
    }

    // ── Atomic apply ──
    let src_backup = store::snapshot(&spec.source_path, "move-src")
        .context("failed to snapshot source before move")?;
    let dst_existed = spec.dest_path.exists();
    let dst_backup = if dst_existed {
        Some(
            store::snapshot(&spec.dest_path, "move-dst")
                .context("failed to snapshot destination before move")?,
        )
    } else {
        None
    };

    // Rewrite the 4th quad on every selected line: graph/ws/A → graph/ws/B.
    let rewritten: Vec<String> = selected
        .iter()
        .filter_map(|l| rewrite_graph(l, &spec.dest_graph))
        .collect();

    let dst_old = if dst_existed {
        std::fs::read_to_string(&spec.dest_path)
            .with_context(|| format!("reading destination graph {}", spec.dest_path.display()))?
    } else {
        String::new()
    };
    let dst_new = build_content(dst_old.lines().chain(rewritten.iter().map(|s| s.as_str())));
    let src_new = build_content(kept.iter().copied());

    commit(spec, &src_new, &dst_new, &src_backup, dst_backup.as_deref(), dst_existed, FailPoint::None)?;

    report.applied = true;
    report.source_backup = Some(src_backup.display().to_string());
    report.dest_backup = dst_backup.map(|p| p.display().to_string());
    Ok(report)
}

/// Join non-empty lines into a trailing-newline-terminated N-Quads document.
fn build_content<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for l in lines {
        if l.trim().is_empty() {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Test seam for the atomic-rollback property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailPoint {
    None,
    AfterDest,
}

/// Write dest, then source, validating each; on ANY failure restore both graphs from
/// their snapshots (or delete a freshly-created dest). This is the atomicity boundary.
fn commit(
    spec: &MoveSpec,
    src_new: &str,
    dst_new: &str,
    src_backup: &Path,
    dst_backup: Option<&Path>,
    dst_existed: bool,
    fail: FailPoint,
) -> Result<()> {
    let result = (|| -> Result<()> {
        write_validated(&spec.dest_path, dst_new)?;
        if fail == FailPoint::AfterDest {
            bail!("injected failure after destination write");
        }
        write_validated(&spec.source_path, src_new)?;
        ensure_healthy(&spec.dest_path)?;
        ensure_healthy(&spec.source_path)?;
        Ok(())
    })();

    if let Err(e) = result {
        // Roll back both tiers from the pre-move snapshot.
        let _ = std::fs::copy(src_backup, &spec.source_path);
        match dst_backup {
            Some(b) => {
                let _ = std::fs::copy(b, &spec.dest_path);
            }
            None if !dst_existed => {
                let _ = std::fs::remove_file(&spec.dest_path);
            }
            None => {}
        }
        return Err(e).context("graph move failed — both graphs rolled back from snapshot");
    }
    Ok(())
}

/// Write `content` to `path` atomically (temp + rename), re-parsing the temp file to
/// guarantee it is valid N-Quads before the rename commits.
fn write_validated(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().context("graph path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating {}", parent.display()))?;
    let tmp = path.with_extension(format!("nq.movetmp.{}", std::process::id()));
    std::fs::write(&tmp, content)
        .with_context(|| format!("writing temp {}", tmp.display()))?;

    match store::graph_health(&tmp) {
        GraphHealth::Healthy | GraphHealth::Missing => {}
        GraphHealth::Unhealthy { reason, .. } => {
            let _ = std::fs::remove_file(&tmp);
            bail!("move would corrupt {} ({reason}) — aborted", path.display());
        }
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("committing {}", path.display()))?;
    Ok(())
}

fn ensure_healthy(path: &Path) -> Result<()> {
    match store::graph_health(path) {
        GraphHealth::Healthy | GraphHealth::Missing => Ok(()),
        GraphHealth::Unhealthy { reason, .. } => {
            bail!("post-move health check failed for {} ({reason})", path.display())
        }
    }
}

// ─── Workspace-name → graph-file resolution (CLI side) ──────────────────────

/// Resolve a workspace NAME to its `graph.nq` path + canonical slug, via the
/// `[[workspace]]` registry. Matches by slugified final path component; prefers a
/// candidate whose graph.nq exists, and errors on a genuine ambiguity.
pub fn resolve_workspace(
    name: &str,
    registry: &[WorkspaceEntry],
) -> Result<(PathBuf, String)> {
    let want = crate::crud::slugify(name);
    let mut candidates: Vec<PathBuf> = registry
        .iter()
        .filter(|e| {
            Path::new(&e.path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(crate::crud::slugify)
                .as_deref()
                == Some(want.as_str())
        })
        .map(|e| Path::new(&e.path).join(".base").join("graph.nq"))
        .collect();
    candidates.sort();
    candidates.dedup();

    let existing: Vec<PathBuf> = candidates.iter().filter(|p| p.exists()).cloned().collect();
    let chosen = match (existing.as_slice(), candidates.as_slice()) {
        ([one], _) => one.clone(),
        ([], [one]) => one.clone(),
        ([], []) => bail!(
            "no registered workspace named '{name}' (slug '{want}') — check `base project list --all` / base.toml [[workspace]]"
        ),
        _ => bail!(
            "workspace name '{name}' is ambiguous — multiple registered paths slugify to '{want}'"
        ),
    };
    Ok((chosen, want))
}

/// Build a [`MoveSpec`] from workspace names, resolving both endpoints' graph files
/// and named-graph IRIs.
pub fn spec_from_names(
    from: &str,
    to: &str,
    registry: &[WorkspaceEntry],
    ns: &NamespaceConfig,
    no_ast: bool,
) -> Result<MoveSpec> {
    let (source_path, source_ws) = resolve_workspace(from, registry)?;
    let (dest_path, dest_ws) = resolve_workspace(to, registry)?;
    if source_ws == dest_ws {
        bail!("source and destination are the same workspace ('{source_ws}') — nothing to move");
    }
    Ok(MoveSpec {
        source_graph: crate::crud::workspace_graph_iri(ns, &source_ws),
        dest_graph: crate::crud::workspace_graph_iri(ns, &dest_ws),
        source_path,
        dest_path,
        source_ws,
        dest_ws,
        no_ast,
    })
}

// ─── Human report ────────────────────────────────────────────────────────────

pub fn format_report(r: &MoveReport) -> String {
    let mut out = String::new();
    out.push_str("═══════════════════════════════════════\n");
    out.push_str(if r.applied { "base graph move\n" } else { "base graph move — DRY RUN\n" });
    out.push_str("═══════════════════════════════════════\n");
    out.push_str(&format!("   {} → {}\n", r.source_ws, r.dest_ws));
    out.push_str(&format!("   {}  →  {}\n", r.source_graph, r.dest_graph));
    out.push_str(&format!("   {} subject(s), {} line(s)\n", r.subjects, r.moved_lines));
    if !r.by_type.is_empty() {
        let kinds: Vec<String> = r.by_type.iter().map(|(k, v)| format!("{v} {k}")).collect();
        out.push_str(&format!("   by kind: {}\n", kinds.join(", ")));
    }
    for s in &r.sample {
        out.push_str(&format!("     · {s}\n"));
    }
    if r.ast_excluded > 0 {
        out.push_str(&format!("   {} AST entit(ies) excluded (--no-ast; regenerate at destination)\n", r.ast_excluded));
    }
    if r.dangling_incoming > 0 {
        out.push_str(&format!("   {} incoming edge(s) left dangling (referencing nodes stay behind)\n", r.dangling_incoming));
    }
    if r.applied {
        if r.moved_lines == 0 {
            out.push_str("   nothing to move (already clean)\n");
        } else {
            out.push_str(&format!(
                "   ✓ moved · src backup: {} · dst backup: {}\n",
                r.source_backup.as_deref().unwrap_or("-"),
                r.dest_backup.as_deref().unwrap_or("(new file)")
            ));
        }
    } else {
        out.push_str("   DRY RUN — nothing written. Re-run with --yes to apply.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }
    fn gws(slug: &str) -> String {
        crate::crud::workspace_graph_iri(&ns(), slug)
    }

    // ── pure primitives ──────────────────────────────────────────────────────

    #[test]
    fn parse_subject_and_graph() {
        let g = gws("a");
        let line = format!("<http://x#project/p> <http://x#name> \"P\" <{g}> .");
        assert_eq!(parse_subject(&line), Some("http://x#project/p"));
        assert_eq!(parse_graph(&line), Some(g.as_str()));
    }

    #[test]
    fn rewrite_graph_swaps_only_fourth_term_and_is_idempotent() {
        let from = gws("a");
        let to = gws("b");
        // IRI object
        let line = format!("<http://x#s> <http://x#p> <http://x#o> <{from}> .");
        let once = rewrite_graph(&line, &to).unwrap();
        assert_eq!(once, format!("<http://x#s> <http://x#p> <http://x#o> <{to}> ."));
        // S/P/O preserved exactly.
        assert!(once.starts_with("<http://x#s> <http://x#p> <http://x#o> "));
        // Idempotent.
        assert_eq!(rewrite_graph(&once, &to).unwrap(), once);

        // Literal object (with spaces and a '<' inside) — graph still the LAST <...>.
        let lit = format!("<http://x#s> <http://x#p> \"a < b c\" <{from}> .");
        let r = rewrite_graph(&lit, &to).unwrap();
        assert_eq!(r, format!("<http://x#s> <http://x#p> \"a < b c\" <{to}> ."));
    }

    #[test]
    fn is_ast_subject_detects_code_ns_and_codemap() {
        let n = ns();
        assert!(is_ast_subject("http://ops-sys.local/code#foo_bar", &n));
        assert!(is_ast_subject("http://ops-sys.local/ontology#codemap/base-v2", &n));
        assert!(!is_ast_subject("http://ops-sys.local/ontology#project/base-v2", &n));
    }

    // ── fixtures ─────────────────────────────────────────────────────────────

    /// A small two-line-per-entity source graph stamped for workspace `a`.
    fn fixture(dir: &Path, slug_graph: &str) -> PathBuf {
        let g = gws(slug_graph);
        let u = ns().uri;
        let mut body = String::new();
        // project node (2 triples)
        body += &format!("<{u}project/demo> <{u}name> \"Demo\" <{g}> .\n");
        body += &format!("<{u}project/demo> <{RDF_TYPE}> <{u}Project> <{g}> .\n");
        body += &format!("<{u}project/demo> <{u}hasDomain> <{u}domain/demo> <{g}> .\n");
        // domain node
        body += &format!("<{u}domain/demo> <{RDF_TYPE}> <{u}Domain> <{g}> .\n");
        // a task (slug-convention)
        body += &format!("<{u}task/demo.ship> <{RDF_TYPE}> <{u}Task> <{g}> .\n");
        body += &format!("<{u}task/demo.ship> <{u}name> \"Ship it\" <{g}> .\n");
        // a decision (slug-convention)
        body += &format!("<{u}decision/demo.use-rust> <{RDF_TYPE}> <{u}Decision> <{g}> .\n");
        // a note attached by relatedTo edge (IRI does NOT carry the slug)
        body += &format!("<{u}note/abc123> <{RDF_TYPE}> <{u}Note> <{g}> .\n");
        body += &format!("<{u}note/abc123> <{u}relatedTo> <{u}domain/demo> <{g}> .\n");
        // an UNRELATED entity that merely references the project (dangling incoming)
        body += &format!("<{u}decision/other.mentions-demo> <{u}affects> <{u}project/demo> <{g}> .\n");
        body += &format!("<{u}decision/other.mentions-demo> <{RDF_TYPE}> <{u}Decision> <{g}> .\n");
        // an AST residual
        body += &format!("<http://ops-sys.local/code#demo_fn> <{RDF_TYPE}> <{u}Function> <{g}> .\n");
        body += &format!("<{u}codemap/demo> <{RDF_TYPE}> <{u}CodeMap> <{g}> .\n");

        let p = dir.join("graph.nq");
        fs::write(&p, body).unwrap();
        p
    }

    fn spec(src: &Path, dst: &Path, no_ast: bool) -> MoveSpec {
        MoveSpec {
            source_path: src.to_path_buf(),
            dest_path: dst.to_path_buf(),
            source_graph: gws("a"),
            dest_graph: gws("b"),
            source_ws: "a".into(),
            dest_ws: "b".into(),
            no_ast,
        }
    }

    #[test]
    fn domain_selector_grabs_node_slug_convention_and_edge_attached() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(dir.path(), "a");
        let subs = resolve_selector(&src, &Selector::Domain("Demo".into()), &gws("a"), &ns()).unwrap();
        let u = ns().uri;
        assert!(subs.contains(&format!("{u}domain/demo")));
        assert!(subs.contains(&format!("{u}project/demo")), "project hasDomain → attached");
        assert!(subs.contains(&format!("{u}task/demo.ship")), "slug-convention task");
        assert!(subs.contains(&format!("{u}decision/demo.use-rust")));
        assert!(subs.contains(&format!("{u}note/abc123")), "note relatedTo domain → attached");
        assert!(!subs.contains(&format!("{u}decision/other.mentions-demo")), "unrelated stays out");
    }

    #[test]
    fn move_rewrites_graph_dest_visible_source_clean() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(dir.path(), "a");
        let dst = dir.path().join("dest.nq");
        fs::write(&dst, "").unwrap();

        let report =
            graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), false).unwrap();
        assert!(report.applied);
        assert!(report.moved_lines >= 8);

        let dst_txt = fs::read_to_string(&dst).unwrap();
        // Every moved line is now stamped graph/ws/b, none left as graph/ws/a.
        assert!(dst_txt.contains(&format!("<{}> .", gws("b"))));
        assert!(!dst_txt.contains(&format!("<{}> .", gws("a"))), "no source-stamp leaked into dest");
        assert!(dst_txt.contains("project/demo"));

        let src_txt = fs::read_to_string(&src).unwrap();
        let u = ns().uri;
        // Subject-precise: the project's OWN triples are gone. (A kept dangling-incoming
        // line still mentions the IRI as an OBJECT, so a substring check would mislead.)
        let has_subject = |txt: &str, iri: &str| txt.lines().any(|l| parse_subject(l) == Some(iri));
        assert!(!has_subject(&src_txt, &format!("{u}project/demo")), "project node removed from source");
        assert!(!has_subject(&src_txt, &format!("{u}task/demo.ship")), "task removed from source");
        // The unrelated decision stays behind (dangling incoming logged, not moved).
        assert!(has_subject(&src_txt, &format!("{u}decision/other.mentions-demo")));
        assert!(report.dangling_incoming >= 1);

        // Both graphs healthy.
        assert_eq!(store::graph_health(&src), GraphHealth::Healthy);
        assert_eq!(store::graph_health(&dst), GraphHealth::Healthy);
    }

    #[test]
    fn move_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(dir.path(), "a");
        let dst = dir.path().join("dest.nq");
        fs::write(&dst, "").unwrap();

        graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), false).unwrap();
        let second =
            graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), false).unwrap();
        assert_eq!(second.moved_lines, 0, "second move finds nothing left");
        assert!(second.applied);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(dir.path(), "a");
        let dst = dir.path().join("dest.nq");
        fs::write(&dst, "").unwrap();
        let before_src = fs::read(&src).unwrap();
        let before_dst = fs::read(&dst).unwrap();

        let report =
            graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), true).unwrap();
        assert!(!report.applied);
        assert!(report.moved_lines >= 8, "preview counts the real move");
        assert_eq!(fs::read(&src).unwrap(), before_src, "dry-run leaves source untouched");
        assert_eq!(fs::read(&dst).unwrap(), before_dst, "dry-run leaves dest untouched");
    }

    #[test]
    fn no_ast_excludes_code_and_codemap() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(dir.path(), "a");
        let dst = dir.path().join("dest.nq");
        fs::write(&dst, "").unwrap();

        // Move everything base-named via prefix; --no-ast must drop code# + codemap/.
        let mut subjects = resolve_selector(&src, &Selector::Prefix("demo".into()), &gws("a"), &ns()).unwrap();
        // Prefix("demo") on subject IRI also matches code#demo_fn? No — code ns subject is
        // "…/code#demo_fn", which contains "demo". Include it explicitly to prove exclusion.
        subjects.insert("http://ops-sys.local/code#demo_fn".to_string());
        subjects.insert(format!("{}codemap/demo", ns().uri));

        let report = graph_move_subjects(&spec(&src, &dst, true), &subjects, &ns(), false).unwrap();
        assert!(report.ast_excluded >= 2, "code# + codemap/ excluded");
        let dst_txt = fs::read_to_string(&dst).unwrap();
        assert!(!dst_txt.contains("code#demo_fn"), "AST entity not moved");
        assert!(!dst_txt.contains("codemap/demo"), "codemap pointer not moved");
        // They remain in source.
        let src_txt = fs::read_to_string(&src).unwrap();
        assert!(src_txt.contains("code#demo_fn"));
    }

    #[test]
    fn atomic_rollback_restores_both_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(dir.path(), "a");
        let dst = dir.path().join("dest.nq");
        fs::write(&dst, format!("<{}existing/x> <{}name> \"X\" <{}> .\n", ns().uri, ns().uri, gws("b"))).unwrap();
        let src_before = fs::read_to_string(&src).unwrap();
        let dst_before = fs::read_to_string(&dst).unwrap();

        let sp = spec(&src, &dst, false);
        let src_backup = store::snapshot(&src, "test").unwrap();
        let dst_backup = store::snapshot(&dst, "test").unwrap();
        // VALID candidate contents so the dest write succeeds — the injected failure then
        // fires *after* the dest write, so rollback must restore BOTH from the snapshots.
        let valid_dst = format!("<{0}x> <{0}p> \"v\" <{1}> .\n", ns().uri, gws("b"));
        let res = commit(&sp, "", &valid_dst, &src_backup, Some(&dst_backup), true, FailPoint::AfterDest);
        assert!(res.is_err(), "injected failure surfaces");
        assert_eq!(fs::read_to_string(&src).unwrap(), src_before, "source restored");
        assert_eq!(fs::read_to_string(&dst).unwrap(), dst_before, "destination restored");
    }

    #[test]
    fn unhealthy_source_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("graph.nq");
        fs::write(&src, "<http://x#s> <http://x#p> \"truncated").unwrap();
        let dst = dir.path().join("dest.nq");
        fs::write(&dst, "").unwrap();
        let subs = HashSet::from(["http://x#s".to_string()]);
        assert!(graph_move_subjects(&spec(&src, &dst, false), &subs, &ns(), false).is_err());
    }
}

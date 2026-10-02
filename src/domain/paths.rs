//! `base domain paths` (P6): every path trigger an exact path (D1).
//!
//! `--suggest` writes nothing. It reads both tiers' `domains.toml` and proposes, per domain, the path list it should
//! have, against the registered projects of the merged store:
//!
//! - The domain of a registered project with a folder (its name has the project's slug): each broad or relative
//!   trigger becomes the project's folder, written once. A trigger that is fine stays. When the domain's
//!   `auto_inject` is false and a broad trigger is replaced, `auto_inject` goes back to true: `false` was F29's advice
//!   for a broad trigger, and D1 rules it out. A domain that set it for any other reason keeps it.
//! - A trigger naming base's own tier folder (`~/.base-gbl`, a workspace's `.base`) or one of base's config files by
//!   bare name (`commands.toml`) becomes the config files that exist: `~/.base-gbl/base.toml`, `commands.toml` and
//!   `domains.toml`, and the workspace's `.base/domains.toml` and `.base/base.toml`. A config file that is a link is
//!   named at its target too, which is where edits go (the WSL `commands.toml` behind the Windows link).
//! - Any other relative trigger is written out as the full path it already resolves to, which changes nothing it
//!   matches.
//! - A broad trigger nothing above can replace stays as it is and is listed for review: never guessed.
//!
//! `--out` writes the proposals as a list to review. `--apply` checks every entry of a reviewed list against one read
//! of both files and the store, refuses the whole file on any fault, then sets each listed domain's paths (and
//! `auto_inject`) in its tier's `domains.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::domain::matcher;
use crate::domain::tier::Tier;

/// One domain whose path list changes, in one tier.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub tier: Tier,
    pub domain: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub auto_inject_before: bool,
    pub auto_inject_after: bool,
    /// One line per trigger that changed, saying why.
    pub reasons: Vec<String>,
}

/// A trigger that stays as it is and needs a person.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    pub tier: Tier,
    pub domain: String,
    pub trigger: String,
    pub why: String,
}

/// Everything `--suggest` found.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub changes: Vec<Change>,
    pub review: Vec<Review>,
    /// Domains with path triggers that need nothing.
    pub unchanged: usize,
}

/// Each tier's `domains.toml` with the root its relative triggers resolve against: home for the global tier, the
/// workspace root for the workspace tier. The workspace tier is left out when it is the global tier's own store
/// (a cwd inside `~/.base-gbl`).
pub fn tier_files(cwd: &Path) -> Vec<(Tier, PathBuf, Option<PathBuf>)> {
    let mut tiers: Vec<(Tier, PathBuf, Option<PathBuf>)> = Vec::new();
    let home = crate::home::home_root();
    if let Some(home) = &home {
        tiers.push((Tier::Global, home.join(".base-gbl").join("domains.toml"), Some(home.clone())));
    }
    if let Some(base_dir) = crate::config::find_workspace_base(cwd) {
        let global_store = home.as_ref().map(|h| h.join(".base-gbl").join(".base"));
        let canon = |p: &Path| crate::scope::canonical_str(&p.display().to_string());
        if global_store.as_deref().is_none_or(|g| canon(g) != canon(&base_dir)) {
            let root = base_dir.parent().map(Path::to_path_buf);
            tiers.push((Tier::Workspace, base_dir.join("domains.toml"), root));
        }
    }
    tiers
}

/// A path in the one spelling base stores (F25b): absolute, `/`, drive letter upper-cased.
fn spelled(p: &str) -> String {
    crate::crud::project::absolute_path(p, None, None).unwrap_or_else(|| p.to_string())
}

/// A link target as a plain path: `\\?\UNC\host\x` is `\\host\x`, `\\?\C:\x` is `C:\x`.
fn plain_target(t: &Path) -> String {
    let s = t.display().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

/// base's own config files that exist, spelled, each link also at its target. Never follows a link to find out
/// whether it exists: the target of the Windows `commands.toml` is a WSL path, and opening one starts WSL.
pub fn base_config_files(home: Option<&Path>, ws_base: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: PathBuf| {
        let Ok(meta) = std::fs::symlink_metadata(&p) else { return };
        let s = spelled(&p.display().to_string());
        if !out.contains(&s) {
            out.push(s);
        }
        if meta.file_type().is_symlink()
            && let Ok(target) = std::fs::read_link(&p)
        {
            let t = if target.is_absolute() || matcher::is_absolute(&target.display().to_string()) {
                spelled(&plain_target(&target))
            } else {
                spelled(&p.parent().map(|d| d.join(&target)).unwrap_or(target).display().to_string())
            };
            if !out.contains(&t) {
                out.push(t);
            }
        }
    };
    if let Some(h) = home {
        for f in ["base.toml", "commands.toml", "domains.toml"] {
            add(h.join(".base-gbl").join(f));
        }
    }
    if let Some(b) = ws_base {
        add(b.join("domains.toml"));
        add(b.join("base.toml"));
    }
    out
}

const BASE_CONFIG_NAMES: [&str; 3] = ["base.toml", "commands.toml", "domains.toml"];

/// Does trigger `t` (resolved to `resolved`) name base's own config: a tier folder, or a config file by bare name?
fn names_base_config(t: &str, resolved: &str, home: Option<&Path>, ws_base: Option<&Path>) -> bool {
    let same = |p: &Path| {
        let p = p.display().to_string();
        matcher::path_under(resolved, &p) && matcher::path_under(&p, resolved)
    };
    let bare = t.trim();
    (!bare.contains(['/', '\\']) && BASE_CONFIG_NAMES.contains(&bare))
        || home.is_some_and(|h| same(&h.join(".base-gbl")))
        || ws_base.is_some_and(same)
}

/// Push `p` unless `list` already names the same place.
fn push_place(list: &mut Vec<String>, p: String, home: Option<&str>) {
    let place = |s: &str| matcher::resolve_trigger(s, None, home);
    let at = place(&p);
    if !list.iter().any(|x| *x == p || (at.is_some() && place(x) == at)) {
        list.push(p);
    }
}

/// Does `p` exist on this machine? `None` for a path this machine does not open (a WSL path from Windows).
fn exists_here(p: &str) -> Option<bool> {
    let low = p.to_ascii_lowercase();
    if low.starts_with("//wsl.localhost/") || low.starts_with("//wsl$/") || (cfg!(windows) && low.starts_with("/home/")) {
        return None;
    }
    Some(std::fs::symlink_metadata(p).is_ok())
}

/// What every domain's path list should be (writes nothing). See the module docs for the rules.
pub fn suggest(cwd: &Path) -> Report {
    let ctx = crate::domain::trigger_context(cwd);
    let home = crate::home::home_root();
    let home_str = home.as_ref().map(|h| h.display().to_string());
    let ws_base = crate::config::find_workspace_base(cwd);
    let config_files = base_config_files(home.as_deref(), ws_base.as_deref());
    let mut report = Report::default();
    for (tier, file, root) in tier_files(cwd) {
        for d in crate::domain::load_domains_file(&file, root.as_deref()) {
            if d.paths.is_empty() {
                continue;
            }
            let slug = crate::crud::slugify(&d.name);
            let project = ctx.registered.iter().find(|r| r.slug == slug && !r.path.is_empty());
            let folderless = project.is_none() && ctx.registered.iter().any(|r| r.slug == slug);
            let folder_broad = project.map(|p| matcher::trigger_breadth(&p.path, &d.name, &ctx)).unwrap_or_default();
            let mut after: Vec<String> = Vec::new();
            let mut reasons: Vec<String> = Vec::new();
            let mut replaced_broad = false;
            let mut review = |trigger: &str, why: String| {
                report.review.push(Review { tier, domain: d.name.clone(), trigger: trigger.to_string(), why })
            };
            for t in &d.paths {
                let Some(resolved) = matcher::resolve_trigger(t, d.root.as_deref(), home_str.as_deref()) else {
                    push_place(&mut after, t.clone(), home_str.as_deref());
                    review(t, "is not a rooted path and cannot fire; write it as a full path, or remove it".into());
                    continue;
                };
                let relative = !matcher::is_absolute(t) && !t.trim().starts_with('~');
                let broad = matcher::trigger_breadth(&resolved, &d.name, &ctx);
                if project.is_none() && names_base_config(t, &resolved, home.as_deref(), ws_base.as_deref()) {
                    for f in &config_files {
                        push_place(&mut after, f.clone(), home_str.as_deref());
                    }
                    reasons.push(format!("`{t}` names base's own config: the config files that exist"));
                    continue;
                }
                if broad.is_empty() && !relative {
                    push_place(&mut after, spelled(t), home_str.as_deref());
                    continue;
                }
                let full = spelled(&resolved);
                match project {
                    Some(p) if folder_broad.is_empty() => {
                        let folder = spelled(&p.path);
                        reasons.push(if broad.is_empty() {
                            format!("`{t}` is relative: {}'s folder, {folder}", d.name)
                        } else {
                            replaced_broad = true;
                            format!("`{t}` holds {}: {}'s folder, {folder}", matcher::count_projects(&broad), d.name)
                        });
                        push_place(&mut after, folder, home_str.as_deref());
                    }
                    Some(p) => {
                        push_place(&mut after, if broad.is_empty() { full } else { spelled(t) }, home_str.as_deref());
                        if !broad.is_empty() {
                            review(
                                t,
                                format!(
                                    "holds {}, and {}'s own folder {} holds {}: set the folder first (base project paths --suggest), then run this again",
                                    matcher::count_projects(&broad),
                                    d.name,
                                    spelled(&p.path),
                                    matcher::count_projects(&folder_broad)
                                ),
                            );
                        }
                    }
                    None if !broad.is_empty() => {
                        push_place(&mut after, spelled(t), home_str.as_deref());
                        let why = if folderless {
                            format!(
                                "holds {}, and project {slug} has no folder: base project update {slug} --path <dir>, then run this again",
                                matcher::count_projects(&broad)
                            )
                        } else {
                            format!("holds {}: name the exact files or folders it is about", matcher::count_projects(&broad))
                        };
                        review(t, why);
                    }
                    None => {
                        let missing = if exists_here(&full) == Some(false) { " (it does not exist)" } else { "" };
                        reasons.push(format!("`{t}` is relative: written out as {full}{missing}"));
                        push_place(&mut after, full, home_str.as_deref());
                    }
                }
            }
            let auto_after = d.auto_inject || replaced_broad;
            if !d.auto_inject && auto_after {
                reasons.push("auto_inject goes back to true: it was F29's advice for a broad trigger, which D1 rules out".into());
            }
            let before_spelled: Vec<String> = d.paths.iter().map(|t| spelled(t)).collect();
            if after == d.paths || (after == before_spelled && reasons.is_empty() && auto_after == d.auto_inject) {
                report.unchanged += 1;
                continue;
            }
            report.changes.push(Change {
                tier,
                domain: d.name.clone(),
                before: d.paths.clone(),
                after,
                auto_inject_before: d.auto_inject,
                auto_inject_after: auto_after,
                reasons,
            });
        }
    }
    report
}

/// The report as text: each change with its reasons, then what is listed for review.
pub fn format_report(r: &Report) -> String {
    let mut out = format!(
        "base domain paths: {} domain(s) change, {} trigger(s) listed for review, {} need nothing. Nothing is written.\n",
        r.changes.len(),
        r.review.len(),
        r.unchanged
    );
    for c in &r.changes {
        out.push_str(&format!("\n{} tier · {}\n", Tier::label(c.tier), c.domain));
        out.push_str(&format!("  before: {}\n", c.before.join(", ")));
        out.push_str(&format!("  after:  {}\n", c.after.join(", ")));
        if c.auto_inject_before != c.auto_inject_after {
            out.push_str(&format!("  auto_inject: {} → {}\n", c.auto_inject_before, c.auto_inject_after));
        }
        for why in &c.reasons {
            out.push_str(&format!("  · {why}\n"));
        }
    }
    if !r.review.is_empty() {
        out.push_str("\nListed for review (these stay as they are until you decide):\n");
        for v in &r.review {
            out.push_str(&format!("  {} tier · {}: `{}` {}\n", Tier::label(v.tier), v.domain, v.trigger, v.why));
        }
    }
    out
}

/// A TOML string, quoted and escaped.
fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The report as a list `--apply` takes: one `[[domain]]` entry per change, setting that domain's whole path list in
/// that tier; what is listed for review follows as comments.
pub fn format_list(r: &Report) -> String {
    let mut out = String::from(
        "# base domain paths --suggest: exact path triggers, one entry per domain that changes (P6).\n\
         # Review it: edit or delete any entry. An entry sets that domain's whole path list in that tier's domains.toml.\n\
         #   base domain paths --apply <this file> --dry-run\n\
         #   base domain paths --apply <this file>\n",
    );
    for c in &r.changes {
        out.push_str(&format!("\n[[domain]]\ntier = {}\nname = {}\npaths = [\n", toml_str(Tier::label(c.tier)), toml_str(&c.domain)));
        for p in &c.after {
            out.push_str(&format!("  {},\n", toml_str(p)));
        }
        out.push_str("]\n");
        if c.auto_inject_before != c.auto_inject_after {
            out.push_str(&format!("auto_inject = {}\n", c.auto_inject_after));
        }
        let before: Vec<String> = c.before.iter().map(|p| toml_str(p)).collect();
        out.push_str(&format!("# was: paths = [{}]", before.join(", ")));
        if c.auto_inject_before != c.auto_inject_after {
            out.push_str(&format!(", auto_inject = {}", c.auto_inject_before));
        }
        out.push('\n');
        for why in &c.reasons {
            out.push_str(&format!("# {why}\n"));
        }
    }
    if !r.review.is_empty() {
        out.push_str("\n# Listed for review, not changed by this list:\n");
        for v in &r.review {
            out.push_str(&format!("# {} tier · {}: `{}` {}\n", Tier::label(v.tier), v.domain, v.trigger, v.why));
        }
    }
    out
}

/// `base domain paths --suggest [--out <file>] [--json]`.
pub fn suggest_cmd(cwd: &Path, out: Option<&Path>, json: bool) -> Result<()> {
    let report = suggest(cwd);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_report(&report));
    }
    if let Some(file) = out {
        std::fs::write(file, format_list(&report)).with_context(|| format!("writing {}", file.display()))?;
        if !json {
            println!("\nList written to {}. Review it, then: base domain paths --apply {}", file.display(), file.display());
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ListFile {
    #[serde(default)]
    domain: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    tier: String,
    name: String,
    paths: Vec<String>,
    auto_inject: Option<bool>,
}

/// One entry of a reviewed list, checked.
#[derive(Debug, Clone, Serialize)]
pub struct Planned {
    pub tier: Tier,
    pub domain: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub auto_inject_before: bool,
    pub auto_inject_after: bool,
}

/// `--apply` refused the list. Typed so the CLI says `Error:`, as for every refusal that wrote nothing.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// Read and check a reviewed list. Every entry is checked before anything is written: the tier is `global` or
/// `workspace` and its file holds the domain, one entry per domain, and every path is a full path (or `~`), not a
/// pattern, and not broad for its domain (D1). Any fault refuses the whole file, naming every fault.
pub fn plan(cwd: &Path, file: &Path) -> Result<Vec<Planned>> {
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let list: ListFile = toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    let ctx = crate::domain::trigger_context(cwd);
    let tiers = tier_files(cwd);
    let mut faults: Vec<String> = Vec::new();
    let mut out: Vec<Planned> = Vec::new();
    for e in &list.domain {
        let tier = match e.tier.as_str() {
            "global" => Tier::Global,
            "workspace" => Tier::Workspace,
            other => {
                faults.push(format!("{}: tier {other:?} is not global or workspace", e.name));
                continue;
            }
        };
        let Some((_, path, root)) = tiers.iter().find(|(t, _, _)| *t == tier) else {
            faults.push(format!("{}: there is no {} tier here", e.name, Tier::label(tier)));
            continue;
        };
        let domains = crate::domain::load_domains_file(path, root.as_deref());
        let Some(d) = domains.iter().find(|d| d.name == e.name) else {
            faults.push(format!("{}: no such domain in {}", e.name, path.display()));
            continue;
        };
        if out.iter().any(|p| p.tier == tier && p.domain == e.name) {
            faults.push(format!("{}: listed twice for the {} tier", e.name, Tier::label(tier)));
            continue;
        }
        for p in &e.paths {
            let t = p.trim();
            if t.contains(['*', '?']) || !(matcher::is_absolute(t) || t == "~" || t.starts_with("~/") || t.starts_with("~\\")) {
                faults.push(format!("{}: `{p}` is not a full path", e.name));
                continue;
            }
            let Some(resolved) = matcher::resolve_trigger(t, None, ctx.home.as_deref()) else {
                faults.push(format!("{}: `{p}` is not a full path", e.name));
                continue;
            };
            let broad = matcher::trigger_breadth(&resolved, &e.name, &ctx);
            if !broad.is_empty() {
                faults.push(format!(
                    "{}: `{p}` holds {}; a trigger must be one project's own folder or a file",
                    e.name,
                    matcher::count_projects(&broad)
                ));
            }
        }
        out.push(Planned {
            tier,
            domain: e.name.clone(),
            before: d.paths.clone(),
            after: e.paths.iter().map(|p| p.trim().to_string()).collect(),
            auto_inject_before: d.auto_inject,
            auto_inject_after: e.auto_inject.unwrap_or(d.auto_inject),
        });
    }
    if !faults.is_empty() {
        return Err(Refused(format!("{} refused, nothing written:\n  {}", file.display(), faults.join("\n  "))).into());
    }
    Ok(out)
}

/// `base domain paths --apply <file> [--dry-run] [--json]`: [`plan`], then one write per tier file.
pub fn apply_cmd(cwd: &Path, file: &Path, dry_run: bool, json: bool) -> Result<()> {
    let planned = plan(cwd, file)?;
    for p in &planned {
        if !json {
            let verb = if dry_run { "would set" } else { "set" };
            let mut line = format!("{verb} {} tier · {}: {} → {}", Tier::label(p.tier), p.domain, p.before.join(", "), p.after.join(", "));
            if p.auto_inject_before != p.auto_inject_after {
                line.push_str(&format!("; auto_inject {} → {}", p.auto_inject_before, p.auto_inject_after));
            }
            println!("{line}");
        }
    }
    if dry_run {
        if json {
            println!("{}", serde_json::to_string_pretty(&planned)?);
        }
        return Ok(());
    }
    for (tier, path, _) in tier_files(cwd) {
        let mine: Vec<(String, Vec<String>, bool)> = planned
            .iter()
            .filter(|p| p.tier == tier)
            .map(|p| (p.domain.clone(), p.after.clone(), p.auto_inject_after))
            .collect();
        if !mine.is_empty() {
            crate::domain::set_paths(&path, &mine).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&planned)?);
    } else {
        println!("{} domain(s) set.", planned.len());
    }
    Ok(())
}

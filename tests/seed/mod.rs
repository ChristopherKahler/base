//! The real-size seed for the session-start budget tests (lane 1, G0.7).
//!
//! A generator, not a copy. The operator's graphs are 35 MB and 21 MB and hold private notes,
//! and this repository is public. [`REAL`] is the shape measured on 2026-09-14: 24 open and 96
//! archived handoffs, 157 open and 62 archived forks, 24 active projects, 115 active tasks, 52
//! active milestones, 926 active notes totalling 970,366 characters (the parsed value oxigraph
//! returns, not the escaped bytes on disk) with the longest at 30,221, 32 domains, 2 due
//! reminders, and the legacy `[signal] max_chars = 2000`. Every value is fixed, so the same seed
//! is written on every run and every machine; the live graphs change while a round runs.
//!
//! Not generated: rules. Session start renders none, and the prompt-hook tests that need them
//! add their own.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const NS: &str = "http://ops-sys.local/ontology#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

pub struct Sizes {
    pub open_handoffs: usize,
    pub archived_handoffs: usize,
    pub open_forks: usize,
    pub archived_forks: usize,
    pub projects: usize,
    pub tasks: usize,
    pub milestones: usize,
    pub notes: usize,
    pub note_chars: usize,
    pub longest_note: usize,
    pub domains: usize,
    pub due_reminders: usize,
}

pub const REAL: Sizes = Sizes {
    open_handoffs: 24,
    archived_handoffs: 96,
    open_forks: 157,
    archived_forks: 62,
    projects: 24,
    tasks: 115,
    milestones: 52,
    notes: 926,
    note_chars: 970_366,
    longest_note: 30_221,
    domains: 32,
    due_reminders: 2,
};

/// One of everything, for tests that need a block to exist rather than a size.
pub const TINY: Sizes = Sizes {
    open_handoffs: 1,
    archived_handoffs: 1,
    open_forks: 1,
    archived_forks: 1,
    projects: 1,
    tasks: 1,
    milestones: 1,
    notes: 1,
    note_chars: 48,
    longest_note: 48,
    domains: 1,
    due_reminders: 1,
};

pub struct Seed {
    /// Use as `BASE_HOME`.
    pub home: PathBuf,
    /// The workspace: the hook's cwd.
    pub ws: PathBuf,
}

/// N-Quads for one named graph.
struct Quads {
    graph: String,
    out: String,
}

impl Quads {
    fn new(graph: &str) -> Self {
        Quads {
            graph: format!("{NS}graph/{graph}"),
            out: String::new(),
        }
    }

    fn typ(&mut self, s: &str, ty: &str) {
        let _ = writeln!(
            self.out,
            "<{NS}{s}> <{RDF_TYPE}> <{NS}{ty}> <{}> .",
            self.graph
        );
    }

    /// `v` must carry no `"`, `\` or line break: the seed writes none, so nothing is escaped.
    fn lit(&mut self, s: &str, p: &str, v: &str) {
        let _ = writeln!(self.out, "<{NS}{s}> <{NS}{p}> \"{v}\" <{}> .", self.graph);
    }

    fn date(&mut self, s: &str, p: &str, v: &str) {
        let _ = writeln!(
            self.out,
            "<{NS}{s}> <{NS}{p}> \"{v}\"^^<{XSD_DATETIME}> <{}> .",
            self.graph
        );
    }

    fn iri(&mut self, s: &str, p: &str, o: &str) {
        let _ = writeln!(self.out, "<{NS}{s}> <{NS}{p}> <{NS}{o}> <{}> .", self.graph);
    }
}

/// A fixed timestamp `minutes` after 2026-08-01 10:00 CDT. Every seeded date is in the past, so
/// every open item is due to resurface.
fn at(minutes: usize) -> String {
    let day = 1 + (minutes / 1440) % 28;
    let hour = 10 + (minutes / 60) % 12;
    let minute = minutes % 60;
    format!("2026-08-{day:02}T{hour:02}:{minute:02}:00-05:00")
}

/// Deterministic text of exactly `chars` characters, with a multi-byte `·` in every sentence so
/// bytes, characters and UTF-16 units disagree the way real notes do.
fn text_of(chars: usize, seed: usize) -> String {
    let sentence = format!("note {seed} records a decision · ");
    let mut s: String = sentence.chars().cycle().take(chars).collect();
    if s.ends_with(' ') {
        s.pop();
        s.push('.');
    }
    s
}

fn handoff(q: &mut Quads, i: usize, kind: &str, status: &str, root: &Path) {
    let slug = format!(
        "2026-08-{:02}-{:04}-seed-{kind}-{i}",
        1 + i % 28,
        1000 + i % 60
    );
    let s = format!("handoff/{slug}");
    q.typ(&s, "Handoff");
    q.lit(&s, "status", status);
    q.lit(&s, "project", &format!("project-{:02}", i % 24));
    q.lit(&s, "kind", kind);
    let doc = root.join("handoffs").join(format!("{slug}.md"));
    q.lit(
        &s,
        "handoffDoc",
        &doc.display().to_string().replace('\\', "/"),
    );
    let created = at(i * 97);
    q.date(&s, "createdAt", &created);
    q.date(&s, "lastActive", &created);
    q.date(&s, "resurfaceAt", &created);
}

/// Write the seed under `root` and return where its home and workspace are. `global_toml` is
/// appended to the global `base.toml` after the legacy key and the memory settings.
pub fn write(root: &Path, sizes: &Sizes, global_toml: &str) -> Seed {
    let home = root.join("home");
    let ws = root.join("ws");
    let gbl = home.join(".base-gbl");
    std::fs::create_dir_all(gbl.join(".base")).expect("global tier");
    std::fs::create_dir_all(ws.join(".base")).expect("workspace tier");

    let mut global = Quads::new("global/seed");
    let mut local = Quads::new("ws/seed");

    // Handoffs and forks: one in five in the global tier, as on the measured machine.
    for i in 0..sizes.open_handoffs + sizes.archived_handoffs {
        let status = if i < sizes.open_handoffs {
            "open"
        } else {
            "archived"
        };
        let q = if i % 5 == 0 { &mut global } else { &mut local };
        handoff(q, i, "handoff", status, root);
    }
    for i in 0..sizes.open_forks + sizes.archived_forks {
        let status = if i < sizes.open_forks {
            "open"
        } else {
            "archived"
        };
        let q = if i % 5 == 0 { &mut global } else { &mut local };
        handoff(q, 10_000 + i, "fork", status, root);
    }

    for i in 0..sizes.projects {
        let s = format!("project/project-{i:02}");
        local.typ(&s, "Project");
        local.lit(&s, "name", &format!("Project {i:02}"));
        local.lit(&s, "status", "active");
        local.lit(&s, "nextAction", &format!("ship slice {i}"));
        local.date(&s, "lastActive", &at(i * 131));
    }
    for i in 0..sizes.milestones {
        let s = format!("milestone/milestone-{i:02}");
        local.typ(&s, "Milestone");
        local.lit(&s, "name", &format!("Milestone {i:02}"));
        local.lit(&s, "status", "active");
        local.iri(
            &format!("project/project-{:02}", i % sizes.projects.max(1)),
            "hasMilestone",
            &s,
        );
    }
    for i in 0..sizes.tasks {
        let s = format!("task/task-{i:03}");
        local.typ(&s, "Task");
        local.lit(
            &s,
            "name",
            &format!("Task {i:03} for project {:02}", i % sizes.projects.max(1)),
        );
        local.lit(
            &s,
            "status",
            if i % 3 == 0 { "in_progress" } else { "active" },
        );
        local.date(&s, "lastActive", &at(i * 53));
    }
    for i in 0..sizes.due_reminders {
        let s = format!("reminder/seed-reminder-{i}");
        local.typ(&s, "Reminder");
        local.lit(&s, "name", &format!("Seed reminder {i} is due"));
        local.date(&s, "resurfaceAt", &at(i * 7));
    }

    // Notes: the longest first, the rest sharing what remains as evenly as whole characters allow.
    let rest = sizes.notes.saturating_sub(1);
    let remaining = sizes.note_chars.saturating_sub(sizes.longest_note);
    for i in 0..sizes.notes {
        let chars = if i == 0 {
            sizes.longest_note
        } else {
            remaining / rest + usize::from(i <= remaining % rest)
        };
        let s = format!("note/seed-note-{i:03}");
        local.typ(&s, "Note");
        local.lit(&s, "noteText", &text_of(chars, i));
        local.lit(
            &s,
            "noteType",
            if i % 4 == 0 { "correction" } else { "insight" },
        );
        local.lit(&s, "status", "active");
        local.date(&s, "createdAt", &at(i * 11));
    }

    std::fs::write(gbl.join(".base").join("graph.nq"), global.out).expect("global graph");
    std::fs::write(ws.join(".base").join("graph.nq"), local.out).expect("workspace graph");

    let mut domains = String::new();
    for i in 0..sizes.domains {
        let _ = writeln!(
            domains,
            "[[domain]]\nname = \"seed-domain-{i:02}\"\nprompt_keywords = [\"seed{i}\", \"alpha{i}\", \"beta{i}\", \"gamma{i}\"]\n"
        );
    }
    std::fs::write(ws.join(".base").join("domains.toml"), domains).expect("domains");

    std::fs::write(
        gbl.join("base.toml"),
        format!("[signal]\nmax_chars = 2000\n\n[memory]\nenabled = true\nmode = \"both\"\n\n{global_toml}"),
    )
    .expect("global base.toml");

    Seed { home, ws }
}

/// Characters in every seeded note's text, as written. The generator's own control: the
/// distribution must land exactly on the measured total.
pub fn note_chars_written(sizes: &Sizes) -> usize {
    let rest = sizes.notes.saturating_sub(1);
    let remaining = sizes.note_chars.saturating_sub(sizes.longest_note);
    (0..sizes.notes)
        .map(|i| {
            let chars = if i == 0 {
                sizes.longest_note
            } else {
                remaining / rest + usize::from(i <= remaining % rest)
            };
            text_of(chars, i).chars().count()
        })
        .sum()
}

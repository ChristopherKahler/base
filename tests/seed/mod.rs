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
//! Two exceptions, on purpose. The first [`Sizes::recent_projects`] projects are touched an hour or
//! a few hours before the seed is written, because "touched in the last 7 days" (spec B6) is
//! measured from the moment session start runs, and no fixed date stays inside that window. And
//! every due reminder falls due one day before the seed is written: lane 3's reminder clock warns
//! on a DUE NOW line from day 8 past due and archives at day 10, so a fixed date would stop
//! rendering as a plain due reminder as the calendar moves.
//!
//! Commit C adds what the B layout reads (lane doc B16): every task and milestone is linked to its
//! project, and two open handoffs share a project from different tiers, the S/W case of spec E1,
//! so one-per-project folding has something to fold.
//!
//! Not generated: rules. Session start renders none, and the prompt-hook tests that need them
//! add their own.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    /// Projects touched within the hours before the seed is written.
    pub recent_projects: usize,
}

/// The S/W pair (spec E1): the open handoff at the second index takes the first one's project. The
/// first sits in the global tier (index divisible by 5) and is created earlier; the second sits
/// in the workspace tier and is newer, so it is listed and the first folds under it.
pub const SW_PAIR: (usize, usize) = (15, 16);

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
    recent_projects: 5,
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
    recent_projects: 1,
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

/// The creation time of handoff or fork `i`, as written.
pub fn handoff_created(i: usize) -> String {
    at(i * 97)
}

/// The slug of handoff or fork `i`, as written.
pub fn handoff_slug(i: usize, kind: &str) -> String {
    format!(
        "2026-08-{:02}-{:04}-seed-{kind}-{i}",
        1 + i % 28,
        1000 + i % 60
    )
}

/// The project of open handoff `i`, as written: its own, except the newer half of the S/W pair.
pub fn handoff_project(i: usize, sizes: &Sizes) -> String {
    let n = if i == SW_PAIR.1 && SW_PAIR.1 < sizes.open_handoffs {
        SW_PAIR.0
    } else {
        i % 24
    };
    format!("project-{n:02}")
}

fn handoff(q: &mut Quads, i: usize, kind: &str, status: &str, project: &str, root: &Path) {
    let slug = handoff_slug(i, kind);
    let s = format!("handoff/{slug}");
    q.typ(&s, "Handoff");
    q.lit(&s, "status", status);
    q.lit(&s, "project", project);
    q.lit(&s, "kind", kind);
    let doc = root.join("handoffs").join(format!("{slug}.md"));
    q.lit(
        &s,
        "handoffDoc",
        &doc.display().to_string().replace('\\', "/"),
    );
    let created = handoff_created(i);
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
        handoff(q, i, "handoff", status, &handoff_project(i, sizes), root);
    }
    for i in 0..sizes.open_forks + sizes.archived_forks {
        let status = if i < sizes.open_forks {
            "open"
        } else {
            "archived"
        };
        let q = if i % 5 == 0 { &mut global } else { &mut local };
        let project = format!("project-{:02}", (10_000 + i) % 24);
        handoff(q, 10_000 + i, "fork", status, &project, root);
    }

    for i in 0..sizes.projects {
        let s = format!("project/project-{i:02}");
        local.typ(&s, "Project");
        local.lit(&s, "name", &format!("Project {i:02}"));
        local.lit(&s, "status", "active");
        local.lit(&s, "nextAction", &format!("ship slice {i}"));
        let touched = if i < sizes.recent_projects {
            (chrono::Local::now() - chrono::Duration::hours(i as i64 + 1))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        } else {
            at(i * 131)
        };
        local.date(&s, "lastActive", &touched);
    }
    for i in 0..sizes.milestones {
        let s = format!("milestone/milestone-{i:02}");
        local.typ(&s, "Milestone");
        local.lit(&s, "name", &format!("Milestone {i:02}"));
        local.lit(&s, "status", "active");
        local.date(&s, "lastActive", &at(i * 67));
        let project = format!("project/project-{:02}", i % sizes.projects.max(1));
        local.iri(&project, "hasMilestone", &s);
        local.iri(&s, "belongsTo", &project);
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
        local.iri(
            &format!("project/project-{:02}", i % sizes.projects.max(1)),
            "hasTask",
            &s,
        );
    }
    // One day past due, a second apart, oldest first: whole days past due stay at 0 for any size
    // under 86,400, inside lane 3's window for a plain DUE NOW line.
    let due_from = chrono::Local::now() - chrono::Duration::days(1);
    for i in 0..sizes.due_reminders {
        let s = format!("reminder/seed-reminder-{i}");
        local.typ(&s, "Reminder");
        local.lit(&s, "name", &format!("Seed reminder {i} is due"));
        let due = (due_from + chrono::Duration::seconds(i as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        local.date(&s, "resurfaceAt", &due);
    }

    for i in 0..sizes.notes {
        let s = format!("note/seed-note-{i:03}");
        local.typ(&s, "Note");
        local.lit(&s, "noteText", &note_text(sizes, i));
        local.lit(&s, "noteType", note_type(i));
        local.lit(&s, "status", "active");
        local.date(&s, "createdAt", &note_created(i));
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
    (0..sizes.notes)
        .map(|i| note_text(sizes, i).chars().count())
        .sum()
}

/// The text of note `i`, as written: the longest first, the rest sharing what remains as evenly
/// as whole characters allow.
pub fn note_text(sizes: &Sizes, i: usize) -> String {
    let rest = sizes.notes.saturating_sub(1);
    let remaining = sizes.note_chars.saturating_sub(sizes.longest_note);
    let chars = if i == 0 {
        sizes.longest_note
    } else {
        remaining / rest + usize::from(i <= remaining % rest)
    };
    text_of(chars, i)
}

/// The type of note `i`, as written: every fourth a correction.
pub fn note_type(i: usize) -> &'static str {
    if i.is_multiple_of(4) {
        "correction"
    } else {
        "insight"
    }
}

/// The creation time of note `i`, as written. Values repeat (minute 0 and minute 720 print the
/// same time), so any order over notes needs a tie-break.
pub fn note_created(i: usize) -> String {
    at(i * 11)
}

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// `base` from the binary `cargo test` builds, isolated the way Claude Code's hooks are driven in
/// these tests: `BASE_HOME` at the seeded home so base's write tripwire stays armed, no auto-update,
/// no detached map build, and none of the relay variables a caller's shell might carry. Exit code,
/// stdout, stderr.
fn run(
    seed: &Seed,
    args: &[&str],
    stdin: Option<&str>,
    relay_as: Option<&str>,
) -> (i32, String, String) {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(&seed.ws)
        .env("BASE_HOME", &seed.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_NO_WAKE_NUDGE")
        .env_remove("BASE_NO_AUTONAME")
        .env_remove("BASE_RELAY_AS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(title) = relay_as {
        cmd.env("BASE_RELAY_AS", title);
    }
    let mut child = cmd.spawn().expect("the base binary runs");
    let mut pipe = child.stdin.take().expect("stdin");
    if let Some(text) = stdin {
        pipe.write_all(text.as_bytes()).expect("stdin written");
    }
    drop(pipe);
    let out = child.wait_with_output().expect("base finishes");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The user-prompt-submit hook, driven as Claude Code drives it: JSON on stdin with the prompt.
///
/// The field name is `prompt`, which is what `extract_prompt` reads first (it also accepts
/// `tool_input.prompt`). Mirrors [`run_session_start`] so the two hooks are measured the same way.
pub fn run_prompt_submit(seed: &Seed, prompt: &str, session: Option<&str>) -> (i32, String, String) {
    let payload = serde_json::json!({
        "cwd": seed.ws.display().to_string(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
        "session_id": session,
    })
    .to_string();
    run(
        seed,
        &["hook", "user-prompt-submit"],
        Some(&payload),
        session.map(|_| "seed-kite"),
    )
}

/// The session-start hook, driven as Claude Code drives it: JSON on stdin. With a session id the
/// relay title is fixed, so the wake contract is part of the output and reads the same every run.
pub fn run_session_start(seed: &Seed, session: Option<&str>) -> (i32, String, String) {
    let payload = serde_json::json!({
        "cwd": seed.ws.display().to_string(),
        "hook_event_name": "SessionStart",
        "source": "startup",
        "session_id": session,
    })
    .to_string();
    run(
        seed,
        &["hook", "session-start"],
        Some(&payload),
        session.map(|_| "seed-kite"),
    )
}

/// Any other `base` command, from the seeded workspace.
pub fn run_base(seed: &Seed, args: &[&str]) -> (i32, String, String) {
    run(seed, args, None, None)
}

/// The unit Claude Code's limit counts, and the budget's.
pub fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Bytes, characters, UTF-16 units and lines, printed beside anything asserted about a size.
pub fn measured(s: &str) -> String {
    format!(
        "bytes={} chars={} utf16={} lines={}",
        s.len(),
        s.chars().count(),
        units(s),
        s.lines().count()
    )
}

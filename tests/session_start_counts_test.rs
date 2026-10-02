//! BO-06 (F10): every number session start prints agrees with every other number under the same label.
//!
//! Measured on 2026-10-01 (session 5b860473): line 1 said `projects 0 · tasks 0`, the pulse said `Projects: 28 active`
//! and `Tasks: 145 open`, and the pulse said `Reminders: 4 overdue` beside a DUE NOW of 5. The header read each count
//! off the block printed with it, so a block skipped as unchanged read 0; the pulse counted the workspace tier with
//! rules of its own. Reproduced on a copy of the operator's store at 50295b9 (FINAL STATE of BO-06).

mod seed;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use seed::{run_session_start, Seed};

const XSD_DATE: &str = "http://www.w3.org/2001/XMLSchema#date";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-bo06-counts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// Quads appended to a tier's seed graph.
struct Extra {
    graph: String,
    out: String,
}

impl Extra {
    fn new(graph: &str) -> Self {
        Extra { graph: format!("{}graph/{graph}", seed::NS), out: String::new() }
    }
    fn typ(&mut self, s: &str, ty: &str) {
        self.out.push_str(&format!("<{0}{s}> <{RDF_TYPE}> <{0}{ty}> <{1}> .\n", seed::NS, self.graph));
    }
    fn lit(&mut self, s: &str, p: &str, v: &str) {
        self.out.push_str(&format!("<{0}{s}> <{0}{p}> \"{v}\" <{1}> .\n", seed::NS, self.graph));
    }
    fn typed(&mut self, s: &str, p: &str, v: &str, ty: &str) {
        self.out.push_str(&format!("<{0}{s}> <{0}{p}> \"{v}\"^^<{ty}> <{1}> .\n", seed::NS, self.graph));
    }
    fn append_to(&self, graph_file: &Path) {
        let mut text = std::fs::read_to_string(graph_file).expect("the seed graph");
        text.push_str(&self.out);
        std::fs::write(graph_file, text).expect("the extra quads");
    }
}

fn stamp(t: chrono::DateTime<chrono::Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// A store with a little of everything session start counts, small enough that every block prints whole, so a second
/// session start skips the working set and the pulse as unchanged: the case that read 0.
fn small_store(tag: &str) -> Seed {
    let sizes = seed::Sizes {
        open_handoffs: 4,
        archived_handoffs: 2,
        open_forks: 5,
        archived_forks: 1,
        projects: 6,
        tasks: 14,
        milestones: 6,
        notes: 1,
        note_chars: 48,
        longest_note: 48,
        domains: 1,
        due_reminders: 3,
        recent_projects: 3,
    };
    let s = seed::write(&root(tag), &sizes, "");
    // One blocked project and one completed one, so the pulse's other two numbers are not 0.
    let mut ws = Extra::new("ws/seed");
    let now = stamp(chrono::Local::now());
    for (slug, status) in [("stuck-one", "blocked"), ("shipped-one", "completed")] {
        let p = format!("project/{slug}");
        ws.typ(&p, "Project");
        ws.lit(&p, "name", slug);
        ws.lit(&p, "status", status);
        ws.typed(&p, "lastActive", &now, XSD_DATETIME);
    }
    ws.append_to(&s.ws.join(".base").join("graph.nq"));
    s
}

/// Every number in line 1, by its label: `due`, `handoffs`, `shown` (the handoffs listed), `forks`, `projects`,
/// `tasks`, `milestones`, `deferred`, `withheld`.
fn header(out: &str) -> HashMap<String, usize> {
    let line = out.lines().find(|l| l.starts_with("[BASE START")).unwrap_or_else(|| panic!("no line 1:\n{out}"));
    let mut found = HashMap::new();
    for part in line.trim_start_matches("[BASE START").split(" · ") {
        let words: Vec<&str> = part.split_whitespace().collect();
        match words.as_slice() {
            [n, "due"] => found.insert("due".to_string(), n.parse().unwrap()),
            ["handoffs", n, "open", shown, "shown)"] => {
                found.insert("shown".to_string(), shown.trim_start_matches('(').parse().unwrap());
                found.insert("handoffs".to_string(), n.parse().unwrap())
            }
            [label, n] => n.parse().ok().and_then(|n| found.insert(label.to_string(), n)),
            _ => None,
        };
    }
    found
}

/// The first number after `prefix` on the line starting with it, and the second number on that line when it has one.
fn numbers_after(out: &str, prefix: &str) -> Option<(usize, Option<usize>)> {
    let line = out.lines().find(|l| l.starts_with(prefix))?;
    let mut nums = line[prefix.len()..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|w| !w.is_empty())
        .map(|w| w.parse::<usize>().unwrap());
    let first = nums.next()?;
    Some((first, nums.next()))
}

/// The numbers each block's first line and the pulse print, checked against line 1 and against the store.
fn assert_counts_agree(out: &str, run: &str, want: &HashMap<&str, usize>) {
    let h = header(out);
    for (label, n) in want {
        assert_eq!(h.get(*label), Some(n), "[{run}] line 1's {label}:\n{out}");
    }
    let block = |prefix: &str| numbers_after(out, prefix);
    if let Some((n, _)) = block("DUE NOW (") {
        assert_eq!(n, h["due"], "[{run}] DUE NOW against line 1:\n{out}");
    }
    if let Some((n, shown)) = block("HANDOFFS (") {
        assert_eq!((n, shown), (h["handoffs"], Some(h["shown"])), "[{run}] HANDOFFS against line 1:\n{out}");
    }
    for (prefix, label) in [("FORKS (", "forks"), ("PROJECTS (", "projects"), ("TASKS (", "tasks"), ("MILESTONES (", "milestones")] {
        if let Some((n, _)) = block(prefix) {
            assert_eq!(n, h[label], "[{run}] {prefix}..) against line 1:\n{out}");
        }
    }
    if let Some((n, _)) = block("Projects: ") {
        assert_eq!(n, h["projects"], "[{run}] the pulse's projects against line 1:\n{out}");
    }
    if let Some((n, _)) = block("Tasks: ") {
        assert_eq!(n, h["tasks"], "[{run}] the pulse's tasks against line 1:\n{out}");
    }
    if let Some((n, _)) = block("Reminders: ") {
        assert_eq!(n, h["due"], "[{run}] the pulse's reminders against line 1:\n{out}");
    }
}

/// F10a, F10b, F10d. On a store with projects, tasks, milestones, reminders, handoffs and forks, line 1, the pulse and
/// every block's first line print the same number for the same label, and that number is what the store holds. Then a
/// second session start, where the working set and the pulse are skipped as unchanged: line 1 keeps the same numbers.
/// Before BO-06 the second start printed `projects 0 · tasks 0 · milestones 0`, and the first start's pulse said
/// `Tasks: 9 open` (the seed's tasks with status exactly "active") beside `TASKS (14 active ...)`.
#[test]
fn session_start_counts_agree() {
    let s = small_store("agree");
    // What the store holds: 6 seeded projects (the blocked and completed ones are neither), 14 tasks, 6 milestones,
    // 3 due reminders, 4 open handoffs, 5 open forks.
    let want: HashMap<&str, usize> =
        [("due", 3), ("handoffs", 4), ("forks", 5), ("projects", 6), ("tasks", 14), ("milestones", 6)].into();

    let (code, first, err) = run_session_start(&s, Some("bo06-counts-first"));
    assert_eq!(code, 0, "{err}");
    for title in ["DUE NOW (", "HANDOFFS (", "FORKS (", "PROJECTS (", "TASKS (", "MILESTONES (", "[Workspace Pulse]"] {
        assert!(first.lines().any(|l| l.starts_with(title)), "control: the first start prints {title}:\n{first}");
    }
    assert_counts_agree(&first, "first start", &want);
    assert!(first.contains("Projects: 6 active, 1 blocked, 1 completed"), "the pulse's projects:\n{first}");
    assert!(first.contains("Tasks: 14 active"), "the pulse says tasks are active, as TASKS does:\n{first}");
    assert!(first.contains("Reminders: 3 due"), "the pulse says reminders are due, as DUE NOW does:\n{first}");

    let (code, second, err) = run_session_start(&s, Some("bo06-counts-second"));
    assert_eq!(code, 0, "{err}");
    for title in ["PROJECTS (", "TASKS (", "MILESTONES (", "[Workspace Pulse]"] {
        assert!(
            !second.lines().any(|l| l.starts_with(title)),
            "control: the second start skips {title} as unchanged, the case that read 0:\n{second}"
        );
    }
    assert_counts_agree(&second, "second start, working set unchanged", &want);
}

/// F10c, Example 2. One rule for a due reminder: live, and its surface time has passed. On 2026-10-01 a reminder that
/// surfaced the day before at midnight with no due date was in DUE NOW and not in the pulse's "overdue", and the pulse
/// counted reminders by a due date, archived and snoozed ones included, in the workspace tier only. Here:
/// - surfaced yesterday at 00:00, no due date (Example 2's reminder 5): due;
/// - in the global tier, surfaced an hour ago: due;
/// - archived, with a due date three days ago: not due;
/// - snoozed to tomorrow, with a due date two days ago: not due.
///
/// Line 1, DUE NOW and the pulse all say 2. Before BO-06 the pulse said `Reminders: 2 overdue` for the other two.
#[test]
fn overdue_rule_shared() {
    let tiny_no_reminders = seed::Sizes { due_reminders: 0, ..seed::TINY };
    let s = seed::write(&root("overdue"), &tiny_no_reminders, "");
    let now = chrono::Local::now();
    let day = chrono::Duration::days(1);
    let midnight_yesterday = (now.date_naive() - day)
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).single())
        .expect("yesterday's midnight");
    let date = |t: chrono::DateTime<chrono::Local>| t.format("%Y-%m-%d").to_string();

    let mut ws = Extra::new("ws/seed");
    let r = "reminder/surfaced-yesterday-at-midnight";
    ws.typ(r, "Reminder");
    ws.lit(r, "name", "Start the crawl");
    ws.typed(r, "resurfaceAt", &stamp(midnight_yesterday), XSD_DATETIME);
    let r = "reminder/archived-past-due";
    ws.typ(r, "Reminder");
    ws.lit(r, "name", "Handled already");
    ws.lit(r, "status", "archived");
    ws.typed(r, "resurfaceAt", &stamp(now - day * 3), XSD_DATETIME);
    ws.typed(r, "dueDate", &date(now - day * 3), XSD_DATE);
    let r = "reminder/snoozed-to-tomorrow";
    ws.typ(r, "Reminder");
    ws.lit(r, "name", "Later");
    ws.typed(r, "resurfaceAt", &stamp(now + day), XSD_DATETIME);
    ws.typed(r, "dueDate", &date(now - day * 2), XSD_DATE);
    ws.append_to(&s.ws.join(".base").join("graph.nq"));
    let mut global = Extra::new("global/seed");
    let r = "reminder/global-an-hour-ago";
    global.typ(r, "Reminder");
    global.lit(r, "name", "Renew the certificate");
    global.typed(r, "resurfaceAt", &stamp(now - chrono::Duration::hours(1)), XSD_DATETIME);
    global.append_to(&s.home.join(".base-gbl").join(".base").join("graph.nq"));

    let (code, out, err) = run_session_start(&s, Some("bo06-overdue"));
    assert_eq!(code, 0, "{err}");
    assert_eq!(header(&out).get("due"), Some(&2), "line 1:\n{out}");
    assert!(out.contains("DUE NOW (2) · all: base reminder list"), "DUE NOW:\n{out}");
    for name in ["Start the crawl", "Renew the certificate"] {
        assert!(out.lines().any(|l| l.starts_with("  ") && l.ends_with(name)), "DUE NOW lists {name}:\n{out}");
    }
    for name in ["Handled already", "Later"] {
        assert!(!out.contains(name), "{name} is not due and is listed:\n{out}");
    }
    assert!(out.contains("Reminders: 2 due"), "the pulse counts by the same rule:\n{out}");
    assert!(
        !out.lines().any(|l| l.starts_with("Reminders:") && l.contains("overdue")),
        "no second word for the same count:\n{out}"
    );
}

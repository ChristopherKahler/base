//! BO-26: the 0.16.0 upgrade needs no action from the user or their Claude: nothing a user has is damaged, and nothing
//! asks them or their Claude to run a command after the update.
//!
//! Every test drives the real binary on a fake home it builds: a global tier at `<home>/.base-gbl/.base/graph.nq` and a
//! workspace `ws` inside the home, whose own graph is `graph/ws/ws`. A home an older base ran in carries that base's
//! session-start stamp, `.hooks-wired-0.15.2`; a home without one is new. Session start runs with `BASE_NO_SPAWN=1`, so the
//! upgrade it would start in the background runs before the hook exits and what each test reads is settled; one test
//! drives the real background process.
//!
//! BO-28 adds the developer-mode step: a base.toml holding the 0.15 installer's `[devmode]` line is marked by that
//! upgrade, and the next session start turns developer mode off and says so first (the tests at the end).

mod seed;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use seed::{run_base, run_hook, Seed, NS};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The first words of every line the upgrade prints (BO-30): base and this version.
const LEAD: &str = concat!("base ", env!("CARGO_PKG_VERSION"), ":");
/// The starter pack 0.14.2 to 0.15.2 installed.
const PACK_0142: &str = include_str!("../src/starter-commands/0.14.2.toml");
/// The starter pack this build ships.
const PACK_NOW: &str = include_str!("../src/starter-commands.toml");

/// N-Quads for one workspace's graph.
struct Quads {
    graph: String,
    out: String,
}

impl Quads {
    fn ws(slug: &str) -> Self {
        Quads { graph: format!("{NS}graph/ws/{slug}"), out: String::new() }
    }
    fn note(&mut self, slug: &str, text: &str) -> &mut Self {
        let s = format!("{NS}note/{slug}");
        let _ = writeln!(self.out, "<{s}> <{RDF_TYPE}> <{NS}Note> <{}> .", self.graph);
        let _ = writeln!(self.out, "<{s}> <{NS}noteText> \"{text}\" <{}> .", self.graph);
        let _ = writeln!(self.out, "<{s}> <{NS}noteType> \"insight\" <{}> .", self.graph);
        self
    }
    fn project(&mut self, slug: &str) -> &mut Self {
        let s = format!("{NS}project/{slug}");
        let _ = writeln!(self.out, "<{s}> <{RDF_TYPE}> <{NS}Project> <{}> .", self.graph);
        let _ = writeln!(self.out, "<{s}> <{NS}name> \"{slug}\" <{}> .", self.graph);
        self
    }
}

/// A fresh home under the system temp folder, cleaned first: both tiers with a few records of their own.
fn home(tag: &str) -> Seed {
    let root = std::env::temp_dir().join(format!("base-bo26-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let s = Seed { home: root.join("home"), ws: root.join("home").join("ws") };
    std::fs::create_dir_all(s.home.join(".base-gbl").join(".base")).expect("global tier");
    std::fs::create_dir_all(s.ws.join(".base")).expect("workspace tier");
    let mut w = Quads::ws("ws");
    for i in 0..6 {
        w.note(&format!("own-{i}"), &format!("own record {i}"));
    }
    write(&ws_graph(&s), &w.out);
    let mut g = Quads::ws("base-gbl");
    g.note("global-own", "a global record");
    write(&gbl(&s).join(".base").join("graph.nq"), &g.out);
    s
}

/// Mark the home as one an older base ran in: the stamp its session start left.
fn upgraded(s: &Seed) {
    write(&gbl(s).join(".hooks-wired-0.15.2"), "");
}

/// Another workspace's project in this workspace's graph: the shape `base doctor` reports as foreign (#142).
fn with_foreign(s: &Seed) {
    let mut gone = Quads::ws("gone");
    gone.project("gone-project");
    let mut text = read(&ws_graph(s));
    text.push_str(&gone.out);
    write(&ws_graph(s), &text);
}

fn gbl(s: &Seed) -> PathBuf {
    s.home.join(".base-gbl")
}

fn ws_graph(s: &Seed) -> PathBuf {
    s.ws.join(".base").join("graph.nq")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("folder");
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// One session start, as Claude Code runs it, with the upgrade it starts settled before it returns.
fn start(s: &Seed) -> String {
    start_with(s, &[("BASE_NO_SPAWN", "1")])
}

fn start_with(s: &Seed, env: &[(&str, &str)]) -> String {
    let payload = serde_json::json!({
        "cwd": s.ws.display().to_string(),
        "hook_event_name": "SessionStart",
        "source": "startup",
        "session_id": "bo26-session",
    });
    let (code, out, err) = run_hook(s, "session-start", &payload, env);
    assert_eq!(code, 0, "session start failed:\n{out}\n{err}");
    out
}

/// The lines session start printed about an upgrade.
fn upgrade_lines(out: &str) -> Vec<String> {
    out.lines().filter(|l| l.trim_start().starts_with(LEAD)).map(|l| l.trim().to_string()).collect()
}

fn doctor(s: &Seed) -> String {
    let (_, out, err) = run_base(s, &["doctor"]);
    format!("{out}{err}")
}

fn record(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&read(&dir.join("upgrade.json"))).unwrap_or(serde_json::Value::Null)
}

/// The fix snapshots in a tier's folder.
fn fix_snapshots(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .expect("tier folder")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("graph.nq.bak-fix"))
        .collect();
    out.sort();
    out
}

/// The backups beside `file` the upgrade made.
fn upgrade_backups(file: &Path) -> Vec<PathBuf> {
    let name = file.file_name().and_then(|n| n.to_str()).expect("a name").to_string();
    let mut out: Vec<PathBuf> = std::fs::read_dir(file.parent().expect("parent"))
        .expect("folder")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{name}.BAK-")) && n.ends_with(&format!("-pre-{VERSION}")))
        })
        .collect();
    out.sort();
    out
}

/// U1: the first session start on a new version repairs a store holding another workspace's records, in the process it
/// starts after it has printed; the next session start says so once with the undo, and neither it nor the one after
/// repairs anything again.
#[test]
fn upgrade_runs_store_repair_once() {
    let s = home("once");
    with_foreign(&s);
    upgraded(&s);
    let before = doctor(&s);
    assert!(before.contains("ANOTHER workspace") && before.contains("UNHEALTHY"), "control:\n{before}");

    let first = start(&s);
    assert!(upgrade_lines(&first).is_empty(), "the first session start prints before the upgrade runs:\n{first}");
    assert!(!read(&ws_graph(&s)).contains("graph/ws/gone"), "the foreign records left the workspace graph");
    assert!(read(&s.ws.join(".base").join("foreign-gone.nq")).contains("project/gone-project"), "and were kept beside it");
    let rec = record(&s.ws.join(".base"));
    assert_eq!(rec["version"], VERSION, "{rec}");
    assert_eq!(rec["status"], "done", "{rec}");
    assert_eq!(rec["from"], "0.15.2", "{rec}");
    assert_eq!(record(&gbl(&s).join(".base"))["version"], VERSION, "the global tier is recorded too");
    let snaps = fix_snapshots(&s.ws.join(".base"));
    assert_eq!(snaps.len(), 1, "one snapshot before the repair wrote: {snaps:?}");

    let second = start(&s);
    let lines = upgrade_lines(&second);
    assert!(
        lines.iter().any(|l| l.contains(
            "We set aside 1 entry from another workspace, because base found no single place on this computer where that \
             workspace keeps its data. It is saved in a separate file in this workspace's .base folder, so nothing was lost, \
             but it no longer shows up here."
        ) && l.contains(&format!(
            "To put this workspace's base data back as it was, run `base doctor --restore \"{}",
            s.ws.join(".base").display()
        ))),
        "{second}"
    );
    let third = start(&s);
    assert!(upgrade_lines(&third).is_empty(), "said once:\n{third}");
    assert_eq!(fix_snapshots(&s.ws.join(".base")), snaps, "nothing repaired again");
    assert_eq!(record(&s.ws.join(".base"))["at"], rec["at"], "the record is the first run's");
    let after = doctor(&s);
    assert!(after.contains("Verdict: HEALTHY"), "{after}");
}

/// U1: a repair that fails leaves the graph byte for byte, says so once with the command to run by hand, and is not tried
/// again for this version. The failure here is a destination file that does not parse, which `--fix` refuses to append to.
#[test]
fn upgrade_repair_failure_leaves_store() {
    let s = home("fails");
    with_foreign(&s);
    write(&s.ws.join(".base").join("foreign-gone.nq"), "this is not n-quads\n");
    upgraded(&s);
    let graph = bytes(&ws_graph(&s));

    let (code, _, err) = run_base(&s, &["hook", "upgrade"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(bytes(&ws_graph(&s)), graph, "the graph is byte for byte as it was");
    assert_eq!(read(&s.ws.join(".base").join("foreign-gone.nq")), "this is not n-quads\n", "and the file it refused");
    let rec = record(&s.ws.join(".base"));
    assert_eq!(rec["status"], "failed", "{rec}");
    assert_eq!(rec["version"], VERSION, "recorded, so not tried again: {rec}");

    let snaps = fix_snapshots(&s.ws.join(".base"));
    run_base(&s, &["hook", "upgrade"]);
    assert_eq!(bytes(&ws_graph(&s)), graph, "not tried again");
    assert_eq!(fix_snapshots(&s.ws.join(".base")), snaps, "no second snapshot");
    let lines = upgrade_lines(&start(&s));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("The cleanup of this workspace's base data did not finish:")
            && lines[0].contains("base will not try again by itself. To run it yourself: `base doctor --fix --yes`."),
        "{lines:?}"
    );
    assert!(upgrade_lines(&start(&s)).is_empty(), "said once");
    // Session start writes the graph itself (its domain links), so "not tried again" is read from what only the repair
    // leaves: the foreign records still in place, no second snapshot, the record as the first run wrote it.
    assert!(read(&ws_graph(&s)).contains("graph/ws/gone"), "session start did not try it either");
    assert_eq!(record(&s.ws.join(".base"))["at"], rec["at"]);
}

/// A workspace domains.toml as a 0.15 user wrote it by hand: a relative trigger over two registered projects (broad), a
/// relative one that holds none, comments.
fn domains_015(s: &Seed) -> String {
    for p in ["alpha-app", "beta-app"] {
        let dir = s.ws.join("Documents").join(p);
        std::fs::create_dir_all(&dir).expect("project folder");
        let (code, out, err) = run_base(s, &["project", "add", "--name", p, "--path", &dir.display().to_string()]);
        assert_eq!(code, 0, "{out}{err}");
    }
    std::fs::create_dir_all(s.ws.join("tools")).expect("tools");
    let mut text = read(&s.ws.join(".base").join("domains.toml"));
    text.push_str(
        "\n# my notes, everywhere under Documents\n[[domain]]\nname = \"notes\"\npaths = [\"Documents\"] # broad\nrules = [\"notes rule\"]\n\n[[domain]]\nname = \"toolbox\"\npaths = [\n  \"tools\", # mine\n]\nrules = [\"toolbox rule\"]\n",
    );
    write(&s.ws.join(".base").join("domains.toml"), &text);
    text
}

/// U2 as ruled at G0: after the upgrade, with no command run, doctor is HEALTHY. The broad trigger is advice naming the
/// projects and `base domain paths --suggest`; the relative triggers are written out as the full paths they resolve to,
/// comments kept, after a backup; the triggers 0.15 left inert are named once.
#[test]
fn upgrade_path_triggers_need_no_command() {
    let s = home("triggers");
    let original = domains_015(&s);
    upgraded(&s);
    start(&s);

    let file = s.ws.join(".base").join("domains.toml");
    let now = read(&file);
    let full = |rel: &str| base::crud::project::absolute_path(&s.ws.join(rel).display().to_string(), None, None).expect("full");
    assert!(now.contains(&format!("paths = [\"{}\"] # broad", full("Documents"))), "{now}");
    assert!(now.contains(&format!("  \"{}\", # mine", full("tools"))), "{now}");
    assert!(now.contains("# my notes, everywhere under Documents"), "comments kept:\n{now}");
    let backups = upgrade_backups(&file);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(read(&backups[0]), original, "the backup is the file before");

    let report = doctor(&s);
    assert!(report.contains("Verdict: HEALTHY"), "{report}");
    assert!(
        report.contains(&format!(
            "workspace tier: path trigger `{}` on `notes` holds 2 registered projects (alpha-app, beta-app): a trigger must be one \
             project's own folder or a file (base domain paths --suggest proposes one)",
            full("Documents")
        )),
        "the broad trigger is still named, as advice:\n{report}"
    );
    assert!(!report.contains("written relative"), "{report}");

    let lines = upgrade_lines(&start(&s));
    assert!(
        lines.iter().any(|l| l.contains("Your rules tied to folders and files now spell out the whole path (2 in this workspace's domains.toml)")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains(&format!(
            "The rules of `notes` tied to `{}` now load for files in that folder that sit outside the 2 projects in it. 0.15 \
             ignored a folder that held more than one project.",
            full("Documents")
        )) && l.ends_with("To narrow them, run `base domain paths --suggest`.")),
        "{lines:?}"
    );
    assert!(upgrade_lines(&start(&s)).is_empty());
}

/// The pack an older base installed, with the user's own command after it.
fn commands_with_mine(pack: &str) -> String {
    format!("{pack}\n# my own\n[[command]]\nname = \"deploy\"\ndescription = \"Ship it\"\nrules = [\"DEPLOY MODE\"]\n")
}

/// U3 and Example 2: blocks still as an older base shipped them get this build's text, after a backup; the user's own
/// command and every other byte stay, in the file's own line endings.
#[test]
fn starter_command_updated_when_unedited() {
    for crlf in [false, true] {
        let s = home(if crlf { "commands-crlf" } else { "commands-lf" });
        let mut text = commands_with_mine(&PACK_0142.replace("\r\n", "\n"));
        if crlf {
            text = text.replace('\n', "\r\n");
        }
        let file = gbl(&s).join("commands.toml");
        write(&file, &text);
        upgraded(&s);
        start(&s);

        let now = read(&file);
        let parsed = base::command::parse_commands(&now).expect("parses");
        for name in ["base", "handoff"] {
            let mine = parsed.iter().find(|c| c.name == name).expect("still there");
            assert_eq!(Some(mine.clone()), base::command::shipped_command(name), "*{name} is this build's");
        }
        assert!(parsed.iter().find(|c| c.name == "base").unwrap().rules.iter().any(|r| r.contains("--fires-on")));
        let deploy = text.find("# my own").expect("mine");
        assert!(now.ends_with(&text[deploy..]), "the user's own command is byte for byte");
        let head = text.find("[[command]]").expect("a block");
        assert_eq!(&now[..head], &text[..head], "the comments above the pack are byte for byte");
        if crlf {
            assert!(!now.replace("\r\n", "").contains('\n'), "CRLF kept");
        }
        let backups = upgrade_backups(&file);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(read(&backups[0]), text, "the backup is the file before");

        let lines = upgrade_lines(&start(&s));
        assert!(
            lines.iter().any(|l| l.contains("Your *handoff and *base commands in your global commands.toml now have this version's text.")
                && l.contains("To undo it, run `base doctor --restore \"")),
            "{lines:?}"
        );
        assert!(upgrade_lines(&start(&s)).is_empty());
    }
}

/// U3 and Example 3: an edited `*base` whose rule add line lacks `--fires-on` is left byte for byte, and session start
/// says once what changed and where to see this version's.
#[test]
fn edited_starter_command_left_with_one_line() {
    let s = home("edited");
    let old_line = "`base rule add --domain <domain> --text \\\"...\\\"`";
    let now_pack = PACK_NOW.replace("\r\n", "\n");
    let at = now_pack.find("`base rule add --domain <domain> --text").expect("the rule add line");
    let end = at + now_pack[at..].find("`. Rules are").expect("its end") + 1;
    let text = now_pack[..at].to_string() + old_line + &now_pack[end..];
    let text = text.replace("Sweep this session's decisions, tasks, and learnings into the graph", "My own sweep");
    assert!(!text.contains("--fires-on"), "control: the old line");
    let file = gbl(&s).join("commands.toml");
    write(&file, &text);
    upgraded(&s);
    start(&s);
    assert_eq!(read(&file), text, "an edited command is left byte for byte");
    assert!(upgrade_backups(&file).is_empty(), "nothing written, nothing backed up");

    let lines = upgrade_lines(&start(&s));
    assert_eq!(
        lines,
        [format!(
            "{LEAD} Your *base command still has the old rule add line. We left it as it is because you changed that command. \
             Inside a Claude Code session, rule add now needs --keywords and --fires-on; to see this version's *base, run \
             `base commands show base --shipped`."
        )]
    );
    assert!(upgrade_lines(&start(&s)).is_empty());
    let (_, shown, _) = run_base(&s, &["commands", "show", "base", "--shipped"]);
    assert!(shown.contains("--fires-on"), "{shown}");
}

/// U4 on a small synthetic 0.15.x store, the one a new user has: the installer's base.toml, the starter pack, a workspace
/// with a relative trigger and a broad one. Install, one session start, and doctor is HEALTHY with nothing left for
/// `--fix` and no line asking for a command.
#[test]
fn doctor_healthy_after_upgrade_with_no_command() {
    let s = home("synthetic");
    write(
        &gbl(&s).join("base.toml"),
        "# base config\n[signal]\nenabled = true\nmax_chars = 2000          # injection budget per session-start (truncates past it)\n",
    );
    write(&gbl(&s).join("commands.toml"), &PACK_0142.replace("\r\n", "\n"));
    domains_015(&s);
    for stamp in [".hooks-wired-0.15.2", ".update-noticed-0.15.2", ".first-run-shown"] {
        write(&gbl(&s).join(stamp), "");
    }
    let before = doctor(&s);
    assert!(before.contains("legacy: [signal] max_chars") && before.contains("written relative"), "control:\n{before}");

    start(&s);
    let after = doctor(&s);
    assert!(after.contains("Verdict: HEALTHY"), "{after}");
    for gone in ["legacy: [signal] max_chars", "written relative", "`base doctor --fix` plans the repair of"] {
        assert!(!after.contains(gone), "{gone:?} is still asked for:\n{after}");
    }
    let base_cmd = base::command::parse_commands(&read(&gbl(&s).join("commands.toml")))
        .expect("parses")
        .into_iter()
        .find(|c| c.name == "base")
        .expect("*base");
    assert!(base_cmd.rules.iter().any(|r| r.contains("--fires-on")), "the star command works with this rule add");
    let toml = read(&gbl(&s).join("base.toml"));
    assert!(!toml.contains("max_chars") && !toml.contains("memory_chars"), "U6: the installer's value goes:\n{toml}");
}

/// U5: every change is announced once, each with an undo, and the undo puts the file back.
#[test]
fn upgrade_announces_once() {
    let s = home("announce");
    with_foreign(&s);
    write(&gbl(&s).join("base.toml"), "[signal]\nmax_chars = 2000\n");
    let commands = PACK_0142.replace("\r\n", "\n");
    write(&gbl(&s).join("commands.toml"), &commands);
    domains_015(&s);
    upgraded(&s);
    let first = start(&s);
    assert!(upgrade_lines(&first).is_empty(), "{first}");

    let lines = upgrade_lines(&start(&s));
    // Each change with its way back: the restore of the backup taken before it, and for the installer's max_chars, which
    // 0.16 reads nowhere, the setting that sets a lower limit again (BO-30, lynx's G0 ruling on row 1).
    for (want, way_back) in [
        ("this workspace's .base folder", "run `base doctor --restore \""),
        ("characters of your saved notes", "run `base config set budget.memory_chars 2000`."),
        ("your global commands.toml", "run `base doctor --restore \""),
        ("this workspace's domains.toml", "run `base doctor --restore \""),
    ] {
        let line = lines.iter().find(|l| l.contains(want)).unwrap_or_else(|| panic!("no line for {want}: {lines:?}"));
        assert!(line.contains(way_back), "{line}");
    }
    for _ in 0..2 {
        assert!(upgrade_lines(&start(&s)).is_empty(), "printed once");
    }

    // The undo printed for commands.toml puts the file back as it was.
    let line = lines.iter().find(|l| l.contains("your global commands.toml")).unwrap();
    let backup = restore_path(line);
    let (code, out, err) = run_base(&s, &["doctor", "--restore", &backup]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(read(&gbl(&s).join("commands.toml")), commands, "restored");
}

/// U6: the installer's `max_chars = 2000` is removed, not moved, so the memory block keeps 0.16's default; any other
/// value still moves.
#[test]
fn f16_installer_value_follows_default() {
    let s = home("u6");
    let toml = gbl(&s).join("base.toml");
    write(&toml, "# mine\n[signal]\nmax_chars = 2000\nenabled = true\n");
    let (code, out, err) = run_base(&s, &["doctor", "--fix", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("[signal] max_chars = 2000 removed (the installer's value, not chosen"), "{out}");
    assert_eq!(read(&toml), "# mine\n[signal]\nenabled = true\n");

    write(&toml, "[signal]\nmax_chars = 2001\n");
    let (_, out, _) = run_base(&s, &["doctor", "--fix", "--yes"]);
    assert!(out.contains("[signal] max_chars = 2001 -> [budget] memory_chars = 2001"), "{out}");
}

/// BO-30, lynx's G0 ruling on row 1: 0.16 reads the installer's old `max_chars` nowhere, so a restore of base.toml is no
/// way back to a smaller memory block. The line names the setting that is, and that command, run as printed, sets it.
#[test]
fn installer_limit_line_names_a_way_back_that_works() {
    let s = home("limit");
    write(
        &gbl(&s).join("base.toml"),
        "# base config\n[signal]\nenabled = true\nmax_chars = 2000          # injection budget per session-start (truncates past it)\n",
    );
    upgraded(&s);
    start(&s);
    let lines = upgrade_lines(&start(&s));
    let line = lines.iter().find(|l| l.contains("characters of your saved notes")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(!line.contains("--restore"), "{line}");
    let command = line.split('`').nth(1).expect("the command, in backticks");
    assert_eq!(command, "base config set budget.memory_chars 2000");
    let (_, before, _) = run_base(&s, &["config", "get", "budget.memory_chars"]);
    assert_eq!(before.trim(), "4000 (default)", "control: the upgrade left 0.16's default");
    let args: Vec<&str> = command.split(' ').skip(1).collect();
    let (code, out, err) = run_base(&s, &args);
    assert_eq!(code, 0, "{out}{err}");
    let (_, after, _) = run_base(&s, &["config", "get", "budget.memory_chars"]);
    assert_eq!(after.trim(), "2000", "the printed command set the lower limit");
    let report = doctor(&s);
    assert!(report.contains("Verdict: HEALTHY") && !report.contains("legacy: [signal] max_chars"), "{report}");
}

/// Q4: `base doctor --restore` takes only a backup base made; anything else is refused with the reason and left alone.
#[test]
fn restore_refuses_a_path_base_did_not_make() {
    let s = home("restore");
    let cases = [
        s.ws.join("notes.txt"),
        s.ws.join("graph.nq.bak-mine"),
        s.ws.join("commands.toml.BAK-20260101-000000-pre-0.16.0"),
        gbl(&s).join("settings.json"),
    ];
    for path in &cases {
        write(path, "mine\n");
        let target_before = bytes(&ws_graph(&s));
        let (code, _, err) = run_base(&s, &["doctor", "--restore", &path.display().to_string()]);
        assert_eq!(code, 1, "{} was not refused", path.display());
        assert!(err.contains("is not a backup base made, so nothing was changed"), "{err}");
        assert_eq!(read(path), "mine\n");
        assert_eq!(bytes(&ws_graph(&s)), target_before);
    }
    // Control: a config backup base made, beside the file in a tier, is taken.
    let file = gbl(&s).join("commands.toml");
    write(&file, "[[command]]\nname = \"now\"\n");
    let backup = gbl(&s).join("commands.toml.BAK-20260101-000000-pre-0.16.0");
    write(&backup, "[[command]]\nname = \"before\"\n");
    let (code, out, err) = run_base(&s, &["doctor", "--restore", &backup.display().to_string()]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(read(&file), "[[command]]\nname = \"before\"\n");
}

/// G0 condition 4: a new user (no older base ran here) gets the record and nothing printed, and nothing is repaired.
#[test]
fn a_new_home_gets_the_record_and_prints_nothing() {
    let s = home("new");
    with_foreign(&s);
    write(&gbl(&s).join("commands.toml"), &PACK_0142.replace("\r\n", "\n"));
    for _ in 0..2 {
        let out = start(&s);
        assert!(upgrade_lines(&out).is_empty(), "{out}");
    }
    let rec = record(&gbl(&s).join(".base"));
    assert_eq!(rec["status"], "fresh", "{rec}");
    assert_eq!(rec["version"], VERSION);
    assert_eq!(record(&s.ws.join(".base")), serde_json::Value::Null, "no workspace record");
    assert!(read(&ws_graph(&s)).contains("graph/ws/gone"), "nothing repaired on a new home");
    assert_eq!(read(&gbl(&s).join("commands.toml")), PACK_0142.replace("\r\n", "\n"));
}

/// G0 condition 6: `[graph] auto_migrate = false` turns the upgrade off.
#[test]
fn auto_migrate_off_turns_the_upgrade_off() {
    let s = home("off");
    with_foreign(&s);
    write(&gbl(&s).join("base.toml"), "[graph]\nauto_migrate = false\n");
    upgraded(&s);
    start(&s);
    assert!(read(&ws_graph(&s)).contains("graph/ws/gone"));
    assert_eq!(record(&s.ws.join(".base")), serde_json::Value::Null);
    assert!(upgrade_lines(&start(&s)).is_empty());
}

/// G0 condition 2 and U7: without `BASE_NO_SPAWN` session start starts the upgrade in a process of its own and returns;
/// the record lands after the session start process has exited. The test holds the upgrade lock while session start runs,
/// so the process it starts is still waiting when the hook has exited, then lets it go. The hook is read the way Claude
/// Code reads it, both pipes to end of file (`run_hook`'s `wait_with_output`): that read ends while the child still
/// waits. On Windows, before `detached::spawn`, the child held the hook's stdout open and the read waited for it.
#[test]
fn the_upgrade_process_outlives_session_start() {
    let s = home("spawn");
    with_foreign(&s);
    upgraded(&s);
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(gbl(&s).join(".base").join("upgrade.lock"))
        .expect("the lock file");
    held.try_lock().expect("the test holds the upgrade lock");
    let reading = std::time::Instant::now();
    let out = start_with(&s, &[("BASE_NO_SPAWN", "")]);
    let read_for = reading.elapsed();
    let exited = std::time::SystemTime::now();
    assert!(
        read_for < std::time::Duration::from_secs(60),
        "the hook's pipes stayed open {read_for:?}: the child held them while it waited on the lock"
    );
    assert!(upgrade_lines(&out).is_empty(), "{out}");
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(record(&s.ws.join(".base")), serde_json::Value::Null, "the upgrade waits on the lock, after the hook exited");
    drop(held);
    let rec_path = s.ws.join(".base").join("upgrade.json");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while record(&s.ws.join(".base"))["version"] != VERSION {
        assert!(std::time::Instant::now() < deadline, "no record two minutes after session start exited");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let landed = std::fs::metadata(&rec_path).and_then(|m| m.modified()).expect("record mtime");
    assert!(landed >= exited, "the record was written after session start exited");
    assert!(!read(&ws_graph(&s)).contains("graph/ws/gone"), "the repair ran in that process");
    // Let it release the lock before the home is reused or removed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while upgrade_lines(&start(&s)).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Q3 and Q2 conditions: a commands.toml or domains.toml that is a link is never written, not even through the link;
/// the commands file gets the notice instead.
#[test]
fn linked_config_files_are_never_written() {
    let s = home("linked");
    let shared = s.home.join("shared");
    let commands = PACK_0142.replace("\r\n", "\n");
    write(&shared.join("commands.toml"), &commands);
    let domains = "[[domain]]\nname = \"toolbox\"\npaths = [\"tools\"]\n";
    write(&shared.join("domains.toml"), domains);
    let links = [(shared.join("commands.toml"), gbl(&s).join("commands.toml")), (shared.join("domains.toml"), s.ws.join(".base").join("domains.toml"))];
    for (target, link) in &links {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        if let Err(e) = made {
            // A Windows account without the symlink privilege cannot make one; Linux CI always runs this.
            println!("skipped: no symlink here ({e})");
            return;
        }
    }
    upgraded(&s);
    start(&s);
    assert_eq!(read(&shared.join("commands.toml")), commands);
    assert_eq!(read(&shared.join("domains.toml")), domains);
    for (_, link) in &links {
        assert!(std::fs::symlink_metadata(link).unwrap().file_type().is_symlink(), "{} is still a link", link.display());
    }
    let lines = upgrade_lines(&start(&s));
    assert!(
        lines.iter().any(|l| l.contains("We left your global commands.toml as it is because it is a link to another file.")
            && l.contains("`base commands show <name> --shipped`")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("domains.toml")), "{lines:?}");
}

/// Code review (BO-26, finding 1): one tier's error is that tier's. Here the global tier cannot write its record; the
/// workspace tier after it is still repaired and recorded. Before, the error ended the process, which has no stderr, and
/// no tier after it ran.
#[test]
fn a_failing_tier_does_not_stop_the_next() {
    let s = home("tier-fails");
    with_foreign(&s);
    upgraded(&s);
    // The global record's temporary file is a folder, so every write of that record fails.
    std::fs::create_dir_all(gbl(&s).join(".base").join("upgrade.json.tmp")).expect("a folder in its way");

    let (code, _, err) = run_base(&s, &["hook", "upgrade"]);
    assert_eq!(code, 0, "{err}");
    let rec = record(&s.ws.join(".base"));
    assert_eq!(rec["version"], VERSION, "the workspace tier ran after the global one failed: {rec}");
    assert_eq!(rec["status"], "done", "{rec}");
    assert!(!read(&ws_graph(&s)).contains("graph/ws/gone"), "and was repaired");
    assert_eq!(record(&gbl(&s).join(".base")), serde_json::Value::Null, "the global record could not be written");
}

/// Code review (BO-26, finding 6): a tier retried after a stop part way keeps the lines it had saved and says each once.
/// The stop is the record a process leaves after its second save: the lines so far, no version.
#[test]
fn a_retried_tier_says_each_line_once() {
    let s = home("retry");
    domains_015(&s);
    upgraded(&s);
    start(&s);
    let ws_store = s.ws.join(".base");
    let mut rec = record(&ws_store);
    let saved: Vec<String> =
        rec["announce"].as_array().expect("lines waiting").iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(
        saved.iter().any(|l| l.contains("0.15 ignored a folder that held more than one project")),
        "control: the Q2b line is waiting: {saved:?}"
    );
    rec["version"] = serde_json::Value::String(String::new());
    write(&ws_store.join("upgrade.json"), &rec.to_string());

    let (code, _, err) = run_base(&s, &["hook", "upgrade"]);
    assert_eq!(code, 0, "{err}");
    let after = record(&ws_store);
    assert_eq!(after["version"], VERSION, "{after}");
    let lines: Vec<&str> = after["announce"].as_array().expect("lines").iter().map(|l| l.as_str().unwrap()).collect();
    assert_eq!(lines, saved.iter().map(String::as_str).collect::<Vec<_>>(), "each line once, none lost");
}

/// Code review (BO-26, finding 5): the undo the upgrade prints restores its snapshot even when that snapshot is the
/// oldest kept, which the restore's own pre-restore snapshot rotates out past `[graph] keep_backups` (3).
#[test]
fn restore_takes_back_a_snapshot_its_own_rotation_removes() {
    let s = home("restore-rotation");
    let dir = s.ws.join(".base");
    let target = dir.join("graph.nq.bak-fix-2026-01-01-000000");
    let wanted = read(&ws_graph(&s));
    write(&target, &wanted);
    write(&dir.join("graph.nq.bak-x1"), "x\n");
    write(&dir.join("graph.nq.bak-x2"), "x\n");
    let now = std::time::SystemTime::now();
    for (p, hours) in [(&target, 3u64), (&dir.join("graph.nq.bak-x1"), 2), (&dir.join("graph.nq.bak-x2"), 1)] {
        let f = std::fs::File::options().write(true).open(p).expect("open");
        f.set_modified(now - std::time::Duration::from_secs(hours * 3600)).expect("age it");
    }
    write(&ws_graph(&s), "");

    let (code, out, err) = run_base(&s, &["doctor", "--restore", &target.display().to_string()]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(read(&ws_graph(&s)), wanted, "restored from the snapshot its rotation took");
}

/// lynx's U4 ruling: the graph is smaller than base's own repair snapshot by what that repair recorded it took out, so
/// doctor says so plainly. A shrink past that, or a compaction snapshot with no record, keeps the data-loss warning.
#[test]
fn doctor_tells_the_repairs_own_shrink_from_a_loss() {
    let s = home("shrink");
    with_foreign(&s);
    upgraded(&s);
    start(&s);
    let after = doctor(&s);
    assert!(
        after.contains("made just before base cleaned up your base data. The cleanup took 2 lines out")
            && after.contains("so your data is 2 lines smaller, and nothing was lost"),
        "{after}"
    );
    assert!(!after.contains("possible data loss"), "{after}");

    // One more line gone than the repair took: a loss, and said so.
    let graph = read(&ws_graph(&s));
    let gone = graph.lines().find(|l| l.contains("own-0")).expect("an own line").to_string();
    write(&ws_graph(&s), &graph.replacen(&format!("{gone}\n"), "", 1));
    let loss = doctor(&s);
    assert!(loss.contains("3 lines smaller than newest backup") && loss.contains("possible data loss"), "{loss}");

    // A compaction run by hand records nothing, so its shrink keeps the warning.
    write(&ws_graph(&s), &format!("{graph}{gone}\n"));
    let (code, out, err) = run_base(&s, &["graph", "compact"]);
    assert_eq!(code, 0, "{out}{err}");
    let compacted = doctor(&s);
    assert!(compacted.contains("possible data loss"), "{compacted}");
}

/// lynx's U4 ruling: a foreign graph `--fix` leaves in place (at least as large as the workspace's own: its shape under
/// an earlier folder name) is shown with why, and the tier stays UNHEALTHY, because a person decides whose records they
/// are.
#[test]
fn doctor_says_why_a_graph_is_left_in_place() {
    let s = home("left");
    let mut old = Quads::ws("old-name");
    for i in 0..7 {
        old.note(&format!("old-{i}"), &format!("old record {i}"));
    }
    let mut text = read(&ws_graph(&s));
    text.push_str(&old.out);
    write(&ws_graph(&s), &text);

    let report = doctor(&s);
    assert!(
        report.contains(&format!(
            "base found data saved under what looks like this workspace's earlier folder name, `old-name` \
             ({NS}graph/ws/old-name). It is at least as large as this workspace's own data (21 against 18), so it is \
             probably yours from before the folder was renamed. base will not move it by itself, because only you can say \
             whose it is."
        )),
        "{report}"
    );
    assert!(report.contains("Verdict: UNHEALTHY"), "{report}");
    assert!(!report.contains("plans the repair of: records of another workspace"), "{report}");
}

/// BO-27, V4, on a store 0.15.2 wrote (BO-21 check 29's rem10 shape): reminders 15, 10 and 7 days overdue, which
/// 0.15.2 never warned about. At the first session start on this build, the one that also runs the upgrade's store
/// repair, every one of them is still listed; the 15- and 10-day ones are in DUE NOW with the warning, dated two days
/// out, and nothing is archived. The repair's move and the warning records both land in the workspace graph. Before
/// BO-27 the first start archived the 15- and 10-day reminders with no line anywhere.
#[test]
fn upgrade_from_0_15_2_keeps_overdue_reminders() {
    const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
    let s = home("rem10");
    with_foreign(&s);
    upgraded(&s);
    let g = format!("{NS}graph/ws/ws");
    let created = (chrono::Local::now() - chrono::Duration::days(20)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let reminders = [
        ("pay-the-insurance-premium", "pay the insurance premium", 15),
        ("call-the-bank-about-the-loan", "call the bank about the loan", 10),
        ("check-the-backups-ran", "check the backups ran", 7),
    ];
    let mut text = read(&ws_graph(&s));
    for (slug, name, days) in reminders {
        let day = (chrono::Local::now() - chrono::Duration::days(days)).date_naive();
        let midnight = day
            .and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
            .expect("local midnight")
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let r = format!("{NS}reminder/{slug}");
        let _ = writeln!(text, "<{r}> <{RDF_TYPE}> <{NS}Reminder> <{g}> .");
        let _ = writeln!(text, "<{r}> <{NS}name> \"{name}\" <{g}> .");
        let _ = writeln!(text, "<{r}> <{NS}resurfaceAt> \"{midnight}\"^^<{XSD}dateTime> <{g}> .");
        let _ = writeln!(text, "<{r}> <{NS}dueDate> \"{day}\"^^<{XSD}date> <{g}> .");
        let _ = writeln!(text, "<{r}> <{NS}createdAt> \"{created}\"^^<{XSD}dateTime> <{g}> .");
        let _ = writeln!(text, "<{r}> <{NS}lastActive> \"{created}\"^^<{XSD}dateTime> <{g}> .");
        let _ = writeln!(text, "<{r}> <{NS}hasDomain> <{NS}domain/unfiled> <{g}> .");
    }
    write(&ws_graph(&s), &text);
    let in_two_days = (chrono::Local::now() + chrono::Duration::days(2)).format("%Y-%m-%d").to_string();

    let first = start(&s);
    for (_, name, days) in &reminders[..2] {
        let line = first.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name} left DUE NOW:\n{first}"));
        assert!(line.contains(&format!("archives {in_two_days} unless snoozed")), "{days}-day reminder: {line}");
    }
    assert!(!first.contains("We archived your reminder"), "nothing is archived at the first start:\n{first}");
    let graph = read(&ws_graph(&s));
    assert!(!graph.contains("graph/ws/gone"), "control: the repair moved the foreign records out");
    assert_eq!(graph.matches(&format!("<{NS}warnedAt>")).count(), 2, "both warnings recorded beside the repair");

    for _ in 0..2 {
        let (code, listed, err) = run_base(&s, &["reminder", "list"]);
        assert_eq!(code, 0, "{err}");
        for (_, name, _) in &reminders {
            assert!(listed.contains(name), "{name} is missing from reminder list:\n{listed}");
        }
        start(&s);
    }
}

// ─── BO-28: the 0.15 installer's developer mode ───────────────────────────

/// The `[devmode]` line every installer from 0.7.0 to 0.15.2 wrote, byte for byte.
const DEVMODE_015: &str = "enabled = true            # false = no diagnostic block";
/// The line 0.16's installer writes, and what the upgrade turns the one above into.
const DEVMODE_016: &str = "enabled = false           # true = a diagnostic block on every response";
/// A global base.toml as a 0.15 installer left it, with a line of the user's own above it.
const BASE_TOML_015: &str = "# my settings\n\n# Appends a DEVMODE block (loaded domains + context bracket) to each response.\n[devmode]\n\
                             enabled = true            # false = no diagnostic block\n\n[bracket]\nenabled = true\n";

/// The paragraph a session start printed under its developer-mode heading, if it printed one.
fn devmode_paragraph(out: &str) -> Option<String> {
    let mut lines = out.lines();
    lines.find(|l| l.starts_with("DEVELOPER MODE TURNED OFF"))?;
    lines.next().map(str::to_string)
}

/// The backup a line's `base doctor --restore "<backup>"` names.
fn restore_path(line: &str) -> String {
    let (_, tail) = line.rsplit_once("base doctor --restore \"").expect("the line names a restore");
    tail.split('"').next().expect("the backup").to_string()
}

/// One prompt in the session the starts belong to.
fn prompt(s: &Seed) -> String {
    let payload = serde_json::json!({
        "cwd": s.ws.display().to_string(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": "what changed in the build since yesterday",
        "session_id": "bo26-session",
    });
    let (code, out, err) = run_hook(s, "user-prompt-submit", &payload, &[]);
    assert_eq!(code, 0, "prompt failed:\n{out}\n{err}");
    out
}

/// A workspace domain [`prompt`] matches. The prompt hook prints the DEVMODE block only on a prompt that matched a
/// domain, so without one a prompt says nothing about developer mode either way.
fn with_matching_domain(s: &Seed) {
    write(&s.ws.join(".base").join("domains.toml"), "[[domain]]\nname = \"builds\"\nprompt_keywords = [\"build\"]\n");
}

/// A home an older base ran in whose global base.toml is the 0.15 installer's.
fn home_015(tag: &str) -> Seed {
    let s = home(tag);
    write(&gbl(&s).join("base.toml"), BASE_TOML_015);
    with_matching_domain(&s);
    upgraded(&s);
    s
}

/// W1: the 0.15 installer's line becomes 0.16's, and nothing else in the file changes; the file is copied aside first, and
/// the tier's record says when and where. Never again: a file put back by hand stays on.
#[test]
fn upgrade_turns_installer_devmode_off() {
    let s = home_015("devmode-off");
    let toml = gbl(&s).join("base.toml");
    start(&s);
    assert_eq!(read(&toml), BASE_TOML_015, "the first start's upgrade only marks it");
    assert_eq!(record(&gbl(&s).join(".base"))["devmode_off"]["status"], "due");

    start(&s);
    assert_eq!(read(&toml), BASE_TOML_015.replace(DEVMODE_015, DEVMODE_016), "only the [devmode] line changed");
    let backups = upgrade_backups(&toml);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(read(&backups[0]), BASE_TOML_015, "backed up as it was");
    let rec = record(&gbl(&s).join(".base"));
    assert_eq!(rec["devmode_off"]["status"], "done", "{rec}");
    let recorded = PathBuf::from(rec["devmode_off"]["backup"].as_str().expect("the backup is recorded"));
    assert_eq!(recorded.file_name(), backups[0].file_name(), "{rec}");
    assert!(rec["devmode_off"]["at"].is_string(), "{rec}");

    // The user puts it back: no later start turns it off again.
    write(&toml, BASE_TOML_015);
    for _ in 0..2 {
        start(&s);
    }
    assert_eq!(read(&toml), BASE_TOML_015, "turned off once, never again");
}

/// W1, Example 2: developer mode turned on by `base config set` (the whole file written again, its comments gone), or a
/// line with a comment of the user's own, is theirs: no change, no backup, no line, no advice.
#[test]
fn user_set_devmode_is_kept() {
    let s = home("devmode-kept");
    let toml = gbl(&s).join("base.toml");
    write(&toml, BASE_TOML_015);
    with_matching_domain(&s);
    let (code, out, err) = run_base(&s, &["config", "set", "devmode.enabled", "true"]);
    assert_eq!(code, 0, "{out}{err}");
    let by_command = read(&toml);
    assert!(!by_command.contains("# false = no diagnostic block"), "control: the command wrote the file again:\n{by_command}");
    let by_hand = home("devmode-hand");
    with_matching_domain(&by_hand);
    write(&gbl(&by_hand).join("base.toml"), "[devmode]\nenabled = true            # mine, kept on on purpose\n");
    for (s, kept) in [(&s, by_command), (&by_hand, read(&gbl(&by_hand).join("base.toml")))] {
        upgraded(s);
        for n in 0..3 {
            let out = start(s);
            assert!(devmode_paragraph(&out).is_none(), "start {n}:\n{out}");
        }
        let toml = gbl(s).join("base.toml");
        assert_eq!(read(&toml), kept);
        assert!(upgrade_backups(&toml).is_empty(), "{:?}", upgrade_backups(&toml));
        assert_eq!(record(&gbl(s).join(".base"))["devmode_off"], serde_json::Value::Null);
        assert!(!doctor(s).contains("turned developer mode off"));
        assert!(prompt(s).contains("DEVMODE"), "still on");
    }
}

/// W2a: the paragraph is the first thing under the header, whole, at the start that turns developer mode off, even when
/// that start is over its byte budget and every other block is cut to its one line.
#[test]
fn devmode_off_line_is_on_the_first_screen() {
    let s = home_015("devmode-screen");
    with_foreign(&s);
    write(&gbl(&s).join("commands.toml"), &PACK_0142.replace("\r\n", "\n"));
    write(&gbl(&s).join("base.toml"), &format!("{BASE_TOML_015}\n[budget]\nsession_start_bytes = 1000\nfirst_screen_chars = 700\n"));
    start(&s);
    let out = start(&s);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() > 2, "{out}");
    assert!(lines[1].starts_with("DEVELOPER MODE TURNED OFF"), "first under the header:\n{out}");
    let paragraph = devmode_paragraph(&out).expect("the paragraph");
    assert_eq!(lines[2], paragraph);
    assert!(paragraph.ends_with("\"`.") && paragraph.contains("`base doctor --restore \""), "whole:\n{paragraph}");
    let full = read(&s.ws.join(".base").join("hook-output").join("bo26-session").join("session-start.md"));
    assert!(!upgrade_lines(&full).is_empty(), "control: the upgrade had its own lines this start:\n{full}");
    assert!(upgrade_lines(&out).is_empty(), "control: the budget cut them to one line:\n{out}");

    // lynx's ruling: the paragraph alone takes the screen past `first_screen_chars`, so that start counts as fitting and
    // its record names the block. Control: an overflow the paragraph is not part of is still reported.
    let rows = || -> Vec<serde_json::Value> {
        read(&s.ws.join(".base").join("hook-output.jsonl"))
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|r| r["hook"] == "session-start")
            .collect()
    };
    let row = rows().pop().expect("the start's row");
    assert!(row["first_screen_len_u16"].as_u64().unwrap_or(0) > 700, "control: the screen did overflow: {row}");
    assert_eq!(row["first_screen_ok"], true, "{row}");
    assert_eq!(row["first_screen_excused"], "devmode-off", "{row}");
    let toml = gbl(&s).join("base.toml");
    write(&toml, &read(&toml).replace("first_screen_chars = 700", "first_screen_chars = 100"));
    start(&s);
    let row = rows().pop().expect("the next start's row");
    assert_eq!(row["first_screen_ok"], false, "the header alone overflows 100 units: {row}");
    assert!(row.get("first_screen_excused").is_none(), "{row}");
}

/// W2b, W2c: what changed and why, what the block did, where to see the same thing without it, the command that turns it
/// back on and the restore of a backup that exists; the instruction block lets the paragraph through on that start only.
#[test]
fn devmode_off_line_names_how_to_turn_it_back_on() {
    let s = home_015("devmode-words");
    // A reminder due today: DUE NOW prints at every start, and with it the instruction block.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (code, out, err) = run_base(&s, &["reminder", "add", "--name", "renew the domain", "--due", &today]);
    assert_eq!(code, 0, "{out}{err}");
    start(&s);
    let out = start(&s);
    let paragraph = devmode_paragraph(&out).unwrap_or_else(|| panic!("no paragraph:\n{out}"));
    for want in [
        "turned developer mode off",
        "The 0.15 installer turned it on for everyone",
        "a DEVMODE block listing which of your domains base loaded and why. To see",
        "To see what base adds without the block, run `base log matches`.",
        "To turn developer mode back on, run `base config set devmode.enabled true` (while it is on, base does not update itself).",
        "To undo this change, run `base doctor --restore \"",
    ] {
        assert!(paragraph.contains(want), "{want:?} missing:\n{paragraph}");
    }
    assert!(Path::new(&restore_path(&paragraph)).is_file(), "{paragraph}");
    assert!(out.contains("Nothing prepended except the developer mode paragraph above."), "{out}");
    let next = start(&s);
    assert!(devmode_paragraph(&next).is_none() && !next.contains("except the developer mode"), "once:\n{next}");
}

/// W2d: doctor repeats the paragraph from the change until 14 days after it, and stops once the user sets developer mode
/// themselves.
#[test]
fn doctor_shows_devmode_line_for_14_days() {
    let s = home_015("devmode-doctor");
    assert!(!doctor(&s).contains("turned developer mode off"), "control: nothing before the change");
    start(&s);
    let paragraph = devmode_paragraph(&start(&s)).expect("the paragraph");
    assert!(doctor(&s).contains(&paragraph), "day 0:\n{}", doctor(&s));
    let rec_file = gbl(&s).join(".base").join("upgrade.json");
    // The paragraph names the version that made the change, not whichever base runs doctor later (code review).
    let mut rec = record(&gbl(&s).join(".base"));
    let made_by = rec["devmode_off"]["version"].clone();
    assert_eq!(made_by, VERSION, "{rec}");
    rec["devmode_off"]["version"] = serde_json::Value::from("9.9.9");
    write(&rec_file, &rec.to_string());
    assert!(doctor(&s).contains("base 9.9.9 turned developer mode off"), "{}", doctor(&s));
    rec["devmode_off"]["version"] = made_by;
    write(&rec_file, &rec.to_string());
    let at = |days: i64| {
        let mut rec = record(&gbl(&s).join(".base"));
        rec["devmode_off"]["at"] = serde_json::Value::from((chrono::Local::now() - chrono::Duration::days(days)).to_rfc3339());
        write(&rec_file, &rec.to_string());
    };
    at(13);
    assert!(doctor(&s).contains(&paragraph), "day 13");
    at(15);
    assert!(!doctor(&s).contains("turned developer mode off"), "day 15:\n{}", doctor(&s));
    at(0);
    assert!(doctor(&s).contains(&paragraph), "control: today again");
    let (code, out, err) = run_base(&s, &["config", "set", "devmode.enabled", "false"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(!doctor(&s).contains("turned developer mode off"), "set by the user:\n{}", doctor(&s));
}

/// The two ways back the paragraph prints, each run as printed: the restore puts the installer's file back, and the
/// command turns it on; either way the prompt carries the DEVMODE block again, and no later start turns it off.
#[test]
fn devmode_undo_restores() {
    for (tag, by_restore) in [("devmode-restore", true), ("devmode-command", false)] {
        let s = home_015(tag);
        start(&s);
        let paragraph = devmode_paragraph(&start(&s)).expect("the paragraph");
        assert!(!prompt(&s).contains("DEVMODE"), "control: off");
        let (code, out, err) = if by_restore {
            run_base(&s, &["doctor", "--restore", &restore_path(&paragraph)])
        } else {
            run_base(&s, &["config", "set", "devmode.enabled", "true"])
        };
        assert_eq!(code, 0, "{out}{err}");
        if by_restore {
            assert_eq!(read(&gbl(&s).join("base.toml")), BASE_TOML_015, "restored byte for byte");
        }
        let (_, got, _) = run_base(&s, &["config", "get", "devmode.enabled"]);
        assert_eq!(got.trim(), "true", "{tag}");
        assert!(prompt(&s).contains("DEVMODE"), "{tag}: the block is back");
        start(&s);
        let (_, got, _) = run_base(&s, &["config", "get", "devmode.enabled"]);
        assert_eq!(got.trim(), "true", "{tag}: not turned off again");
    }
}

/// G0 Q1: a base.toml that is a link is never written, not even through the link; one line says developer mode is
/// still on there and how to turn it off.
#[test]
fn linked_base_toml_is_left_with_a_line() {
    let s = home("devmode-linked");
    let shared = s.home.join("shared").join("base.toml");
    write(&shared, BASE_TOML_015);
    let link = gbl(&s).join("base.toml");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&shared, &link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&shared, &link);
    if let Err(e) = made {
        // A Windows account without the symlink privilege cannot make one; Linux CI always runs this.
        println!("skipped: no symlink here ({e})");
        return;
    }
    upgraded(&s);
    start(&s);
    let out = start(&s);
    assert_eq!(read(&shared), BASE_TOML_015);
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
    assert!(devmode_paragraph(&out).is_none(), "{out}");
    let lines = upgrade_lines(&out);
    assert!(
        lines.iter().any(|l| l.contains("Developer mode is still on. We left")
            && l.contains("as it is because it is a link to another file.")),
        "{lines:?}"
    );
    assert!(upgrade_lines(&start(&s)).is_empty(), "once");
}

/// G0 Q2's timing: the block stops at the start that says so. After the first start (whose upgrade only marks it) the
/// prompt still carries the DEVMODE block and nothing has been said; the second start turns it off and prints the
/// paragraph, and its prompt carries no block; the third prints nothing.
#[test]
fn devmode_off_happens_at_the_start_that_says_so() {
    let s = home_015("devmode-when");
    let first = start(&s);
    assert!(devmode_paragraph(&first).is_none(), "{first}");
    assert!(prompt(&s).contains("DEVMODE"), "on until the start that says so");
    let second = start(&s);
    assert!(devmode_paragraph(&second).is_some(), "{second}");
    assert!(!prompt(&s).contains("DEVMODE"), "off from that start");
    let third = start(&s);
    assert!(devmode_paragraph(&third).is_none() && !third.contains("DEVELOPER MODE"), "{third}");
}

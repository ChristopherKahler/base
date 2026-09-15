//! Board ruling R7, the single point revival rests on: `base handoff show` finds the handoff a
//! person means from a letter, a slug, a project name or loose words, prints the doc and what it
//! matched, lists several matches without picking one, and writes nothing (lane doc W1.2 Q1).
//!
//! Driven through the binary `cargo test` builds, over a seed of 24 open handoffs in two tiers. Every
//! test here runs red against commit B's head, where the subcommand does not exist; each one reads
//! the output before the exit code, so clap's own exit 2 for an unknown subcommand cannot pass the
//! test that expects exit 2 for several matches.

mod seed;

use std::path::Path;

use seed::{run_base, run_session_start};

const SHOW: seed::Sizes = seed::Sizes {
    open_handoffs: 24,
    archived_handoffs: 2,
    open_forks: 1,
    archived_forks: 0,
    projects: 24,
    tasks: 0,
    milestones: 0,
    notes: 1,
    note_chars: 48,
    longest_note: 48,
    domains: 1,
    due_reminders: 0,
    recent_projects: 0,
};

/// The doc path the seed writes for open handoff `i`.
fn doc_of(root: &Path, i: usize) -> String {
    root.join("handoffs")
        .join(format!("{}.md", seed::handoff_slug(i, "handoff")))
        .display()
        .to_string()
        .replace('\\', "/")
}

fn md5ish(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A letter names the handoff the last session start lettered, even once a newer handoff would take
/// that letter in a list rebuilt now. And `show` leaves both graphs byte for byte as it found them.
#[test]
fn a_letter_names_what_the_last_session_start_printed_even_after_a_newer_handoff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &SHOW, "");
    let (code, start, stderr) = run_session_start(&seed, None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        !start.is_empty(),
        "session start printed nothing. stderr: {stderr}"
    );

    // The newest open handoff in the seed is 22 (lane doc B17 works the order out).
    let newest = doc_of(tmp.path(), 22);
    let (code, out, err) = run_base(&seed, &["handoff", "show", "A"]);
    assert_eq!(
        out.lines().next(),
        Some(format!("doc: {newest}").as_str()),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("matched: letter A from the session start at "),
        "{out}"
    );
    assert_eq!(code, 0, "{out}");

    let later = tmp
        .path()
        .join("handoffs")
        .join("2026-09-14-2359-kite-project-99.md");
    let (code, out, err) = run_base(
        &seed,
        &[
            "handoff",
            "create",
            "--project",
            "project-99",
            "--doc",
            &later.display().to_string(),
        ],
    );
    assert_eq!(
        code, 0,
        "control: the newer handoff registered. stdout:\n{out}\nstderr:\n{err}"
    );

    let graphs = [
        seed.home.join(".base-gbl").join(".base").join("graph.nq"),
        seed.ws.join(".base").join("graph.nq"),
    ];
    let before: Vec<Vec<u8>> = graphs.iter().map(|g| md5ish(g)).collect();
    let (code, out, err) = run_base(&seed, &["handoff", "show", "a"]);
    assert_eq!(
        out.lines().next(),
        Some(format!("doc: {newest}").as_str()),
        "the letter moved to the newer handoff. stdout:\n{out}\nstderr:\n{err}"
    );
    assert_eq!(code, 0, "{out}");
    let after: Vec<Vec<u8>> = graphs.iter().map(|g| md5ish(g)).collect();
    assert!(before == after, "base handoff show wrote to a graph");
}

/// With no letters file, a letter is resolved against the list rebuilt now, and the output says so.
#[test]
fn without_a_letters_file_the_letter_is_rebuilt_and_says_so() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &SHOW, "");
    let (code, out, err) = run_base(&seed, &["handoff", "show", "a"]);
    assert_eq!(
        out.lines().next(),
        Some(format!("doc: {}", doc_of(tmp.path(), 22)).as_str()),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("matched: letter A, from a list rebuilt now"),
        "{out}"
    );
    assert!(
        out.contains(
            "note: no session start here left a letters file, so the letters were rebuilt now"
        ),
        "{out}"
    );
    assert_eq!(code, 0, "{out}");
}

/// A slug, a project name in any case, and loose words each resolve to exactly one handoff, and the
/// output names the rule that matched.
#[test]
fn a_slug_a_project_and_loose_words_each_resolve_to_one_handoff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &SHOW, "");
    let slug5 = seed::handoff_slug(5, "handoff");
    let cases: [(&[&str], usize, &str); 3] = [
        (&[slug5.as_str()], 5, "matched: the slug"),
        (&["PROJECT-07"], 7, "matched: the project name"),
        (
            &["resume", "the", "seed", "handoff", "1012"],
            12,
            "matched: 3 of 5 words",
        ),
    ];
    for (query, i, rule) in cases {
        let mut args = vec!["handoff", "show"];
        args.extend(query.iter().copied());
        let (code, out, err) = run_base(&seed, &args);
        assert_eq!(
            out.lines().next(),
            Some(format!("doc: {}", doc_of(tmp.path(), i)).as_str()),
            "{query:?}. stdout:\n{out}\nstderr:\n{err}"
        );
        assert!(out.contains(rule), "{query:?}: {out}");
        assert!(
            out.contains(&format!(
                "handoff: {} · project project-{i:02} · ",
                seed::handoff_slug(i, "handoff")
            )),
            "{query:?}: {out}"
        );
        assert_eq!(code, 0, "{query:?}: {out}");
    }
}

/// Two open handoffs share project-15 from two tiers (the seed's S/W pair): both are listed with
/// their tier, no doc line is printed, nothing is picked, exit 2.
#[test]
fn several_matches_are_listed_and_none_is_picked() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &SHOW, "");
    let (code, out, err) = run_base(&seed, &["handoff", "show", "project-15"]);
    assert!(
        out.starts_with(
            "2 open handoffs match \"project-15\" by the project name. None was picked;"
        ),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    for (i, tier) in [(15, "global tier"), (16, "workspace tier")] {
        let want = format!(
            "  {} · project project-15 · {tier} · ",
            seed::handoff_slug(i, "handoff")
        );
        assert!(
            out.lines().any(|l| l.starts_with(&want)),
            "no {want:?} line: {out}"
        );
    }
    assert!(
        !out.lines().any(|l| l.starts_with("doc: ")),
        "a doc was picked: {out}"
    );
    assert_eq!(code, 2, "{out}");
}

/// No match says so, names the tiers it searched, and exits 1.
#[test]
fn no_match_exits_one_and_names_what_it_searched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &SHOW, "");
    let (code, out, err) = run_base(&seed, &["handoff", "show", "zebra"]);
    assert!(
        out.starts_with("no open handoff matches \"zebra\"\n"),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    assert!(out.contains("searched: "), "{out}");
    assert_eq!(code, 1, "{out}");
}

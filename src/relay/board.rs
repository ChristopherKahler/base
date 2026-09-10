use std::path::Path;

use super::{age_str, claim_expired, list_projects, parse_ts, relay_root, RelayStore, DEAD_AFTER_SECS};

// ─── Operator board ──────────────────────────────────────────
//
// The single view the operator governs from: every registered session, its
// phase, worktree, liveness, claims, and pending-message pressure — across
// every relay store in the workspace.

pub fn print_board(cwd: &Path, project: Option<&str>) {
    let Some(root) = relay_root(cwd) else {
        eprintln!("No relay stores in this workspace. Create one: base relay init --project <name>");
        return;
    };
    let projects = match project {
        Some(p) => vec![p.to_string()],
        None => list_projects(&root),
    };
    if projects.is_empty() {
        eprintln!("No relay stores in this workspace. Create one: base relay init --project <name>");
        return;
    }

    for p in &projects {
        let store = RelayStore { root: root.join(p), project: p.clone() };
        if !store.exists() {
            eprintln!("Relay store '{p}' not found.");
            continue;
        }
        print_store_board(&store);
    }
}

fn print_store_board(store: &RelayStore) {
    let reg = store.load_registry();
    let claims = store.load_claims();
    let messages = store.all_messages();

    println!("═══ RELAY: {} ═══", store.project);
    println!(
        "{} sessions · {} messages · {} claims",
        reg.sessions.len(),
        messages.len(),
        claims.claims.len()
    );

    if !reg.sessions.is_empty() {
        println!("\n| Session | Phase | Worktree | Bound | Last seen | Watching | Pending |");
        println!("|---------|-------|----------|-------|-----------|----------|---------|");
        for entry in reg.sessions.values() {
            let alive = parse_ts(&entry.last_heartbeat)
                .map(|t| (chrono::Local::now() - t).num_seconds() < DEAD_AFTER_SECS)
                .unwrap_or(false);
            let liveness = if alive {
                age_str(&entry.last_heartbeat)
            } else {
                format!("DEAD ({})", age_str(&entry.last_heartbeat))
            };
            println!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                entry.title,
                entry.phase.as_deref().unwrap_or("-"),
                entry.worktree,
                if entry.session_id.is_some() { "✓" } else { "-" },
                liveness,
                // The holder comes from THIS store's own binding, not the global
                // registry (#132 C9): measured 2026-09-10, 118 rows here against
                // 27 global titles with only 9 in both, so resolving globally
                // would leave 109 rows unable to name a holder at all.
                super::wake::watch_cell_for(&entry.title, entry.session_id.as_deref()),
                store.pending_for(&entry.title).len(),
            );
        }

        // Who is touching each sentinel (#132). The Watching cell says WHICH
        // STATE a title is in; this says WHICH SESSION, with the id in full —
        // a partial id would invite completing it into the wrong one, and a
        // full id in every table row wrecks a board the operator scans
        // constantly. Bounded by the number of loops actually running: a title
        // that nothing is touching has no line here, because its cell already
        // says everything there is to say.
        let watchers: Vec<(String, String)> = reg
            .sessions
            .values()
            .filter_map(|e| {
                super::wake::watch_detail_for(&e.title, e.session_id.as_deref())
                    .map(|d| (e.title.clone(), d))
            })
            .collect();
        if !watchers.is_empty() {
            println!(
                "\nWATCHERS ({} — who last touched each title's sentinel):",
                watchers.len()
            );
            for (title, detail) in &watchers {
                println!("  {title} {detail}");
            }
        }
    }

    if !claims.claims.is_empty() {
        println!("\nClaims:");
        for c in claims.claims.values() {
            let status = if claim_expired(c) { " [EXPIRED]" } else { "" };
            println!("  {} — {} ({} ago){}: {}", c.resource, c.by, age_str(&c.ts), status, c.note);
        }
    }

    // Unrouted: addressed to a title nobody has registered.
    let unrouted: Vec<_> = messages
        .iter()
        .filter(|m| {
            m.to != "all"
                && !m.to.starts_with("phase:")
                && !reg.sessions.contains_key(&m.to)
                && !reg.sessions.values().any(|e| e.session_id.as_deref() == Some(m.to.as_str()))
        })
        .collect();
    if !unrouted.is_empty() {
        println!("\nUNROUTED ({} — recipient never registered):", unrouted.len());
        for m in unrouted.iter().take(5) {
            println!("  → {} from {} ({}): {}", m.to, m.from, m.mtype, truncate(&m.msg, 60));
        }
    }
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

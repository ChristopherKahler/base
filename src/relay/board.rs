use std::path::Path;

use super::{age_str, claim_expired, list_projects, relay_root, RelayStore};

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
            // Which session holds this title RIGHT NOW. A store row is keyed
            // on the title, so a retired seat leaves its row behind and the next
            // heartbeat on that title refreshes it.
            let live = super::session_registry::resolve(&entry.title).map(|e| e.session_id);
            let liveness = super::row_liveness(
                entry.session_id.as_deref(),
                live.as_deref(),
                &entry.last_heartbeat,
            );
            println!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                entry.title,
                entry.phase.as_deref().unwrap_or("-"),
                entry.worktree,
                if entry.session_id.is_some() { "✓" } else { "-" },
                liveness,
                super::wake::watch_cell(&entry.title),
                store.pending_for(&entry.title).len(),
            );
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

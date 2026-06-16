pub mod active_awareness;
pub mod flow_resurface;
pub mod memory;
pub mod pulse;
pub mod staleness;
pub mod suppression;

use std::path::Path;

use anyhow::Result;

use crate::config::BaseConfig;

/// A signal result: name, priority (lower = higher), and output text.
struct SignalResult {
    name: String,
    priority: u32,
    output: String,
}

/// Combined signal output: content (suppression-checked) + diagnostics (always emitted).
pub struct SignalOutput {
    pub content: String,
    /// No-match tags: `<hook-query:no-match>` for each query that ran but found nothing.
    /// Always emitted — bypass suppression so operator can verify queries executed.
    pub diagnostics: Vec<String>,
}

/// Run all signals with suppression and budget cap. Returns content + diagnostics.
pub fn run_signals(cwd: &Path, config: &BaseConfig, hook: &str) -> Result<SignalOutput> {
    if !config.signal.enabled {
        return Ok(SignalOutput { content: String::new(), diagnostics: vec![] });
    }

    let ns = &config.namespace;
    let sig = &config.signal;
    let base_dir = crate::config::find_workspace_base(cwd);

    let mut results: Vec<SignalResult> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();

    match memory::run(cwd, config) {
        Ok(output) if !output.is_empty() => {
            results.push(SignalResult { name: "memory".into(), priority: 0, output });
        }
        Ok(_) => {}
        Err(e) => eprintln!("base: signal 'memory' failed: {e}"),
    }

    match active_awareness::run(cwd, ns, sig) {
        Ok(output) if !output.is_empty() => {
            results.push(SignalResult { name: "active-awareness".into(), priority: 1, output });
        }
        Ok(_) => diagnostics.push(format!("<{hook}-active-awareness:no-match>")),
        Err(e) => eprintln!("base: signal 'active-awareness' failed: {e}"),
    }
    match pulse::run(cwd, ns, sig) {
        Ok(output) if !output.is_empty() => {
            results.push(SignalResult { name: "pulse".into(), priority: 2, output });
        }
        Ok(_) => diagnostics.push(format!("<{hook}-pulse:no-match>")),
        Err(e) => eprintln!("base: signal 'pulse' failed: {e}"),
    }
    // flow_resurface's stale_detection runs the same query with richer output —
    // when flow resurface is active it is the single source of stale notices.
    if config.flow.enabled && config.flow.resurface {
        diagnostics.push(format!("<{hook}-staleness:superseded-by-flow>"));
    } else {
        match staleness::run(cwd, ns, sig) {
            Ok(output) if !output.is_empty() => {
                results.push(SignalResult { name: "staleness".into(), priority: 3, output });
            }
            Ok(_) => diagnostics.push(format!("<{hook}-staleness:no-match>")),
            Err(e) => eprintln!("base: signal 'staleness' failed: {e}"),
        }
    }

    // Flow resurface signal (gated by [flow] config)
    if config.flow.enabled && config.flow.resurface {
        match flow_resurface::run(cwd, ns, &config.flow, hook) {
            Ok((output, flow_diags)) => {
                if !output.is_empty() {
                    results.push(SignalResult { name: "flow-resurface".into(), priority: 2, output });
                }
                diagnostics.extend(flow_diags);
            }
            Err(e) => eprintln!("base: signal 'flow-resurface' failed: {e}"),
        }
    }

    // Sort by priority
    results.sort_by_key(|r| r.priority);

    // Apply suppression: skip signals whose output hasn't changed
    let mut state = base_dir
        .as_deref()
        .map(suppression::SignalState::load)
        .unwrap_or_default();

    let novel: Vec<&SignalResult> = results
        .iter()
        .filter(|r| {
            let hash = suppression::hash_output(&r.output);
            state.is_novel(&r.name, hash)
        })
        .collect();

    if novel.is_empty() {
        return Ok(SignalOutput { content: String::new(), diagnostics });
    }

    // Apply budget cap
    let mut combined = String::new();
    let mut chars_used = 0;
    let mut dropped = 0;

    for result in &novel {
        if result.priority == 1 || chars_used + result.output.len() <= sig.max_chars {
            combined.push_str(&result.output);
            combined.push('\n');
            chars_used += result.output.len();

            // Update suppression state for emitted signals
            let hash = suppression::hash_output(&result.output);
            state.update(&result.name, hash);
        } else {
            dropped += 1;
        }
    }

    if dropped > 0 {
        combined.push_str(&format!("[+{dropped} signals suppressed — budget cap]\n"));
    }

    // Save suppression state
    if let Some(ref base_dir) = base_dir {
        let _ = state.save(base_dir);
    }

    Ok(SignalOutput {
        content: combined.trim_end().to_string(),
        diagnostics,
    })
}

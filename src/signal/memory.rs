use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::BaseConfig;
use crate::crud;

const MAX_CHARS: usize = 4000;

pub fn run(cwd: &Path, config: &BaseConfig) -> Result<String> {
    if !config.memory.enabled || config.memory.mode == "claude" {
        return Ok(String::new());
    }

    let ns = &config.namespace;
    let p = &ns.prefix;

    let sparql = format!(
        "SELECT DISTINCT ?text ?type ?created WHERE {{\n\
           GRAPH ?g {{\n\
             ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" .\n\
             OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
           }}\n\
         }}\n\
         ORDER BY ?type DESC(?created)"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let text = term_str(&row, "text");
            let note_type = term_str(&row, "type");
            (text, note_type)
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut corrections = Vec::new();
    let mut other = Vec::new();

    for (text, note_type) in &rows {
        let t = note_type.rsplit('/').next().unwrap_or(note_type);
        if t == "correction" || t == "feedback" {
            corrections.push(format!("- [correction] {text}"));
        } else {
            other.push(format!("- [{t}] {text}"));
        }
    }

    let mut out = String::from("<base-memory>\n");
    let mut chars = out.len();

    for line in corrections.iter().chain(other.iter()) {
        if chars + line.len() + 1 > MAX_CHARS {
            let remaining = corrections.len() + other.len()
                - out.matches("\n- [").count();
            if remaining > 0 {
                out.push_str(&format!("... and {remaining} more notes\n"));
            }
            break;
        }
        out.push_str(line);
        out.push('\n');
        chars += line.len() + 1;
    }

    out.push_str("</base-memory>");
    Ok(out)
}

fn term_str(row: &oxigraph::sparql::QuerySolution, var: &str) -> String {
    row.get(var)
        .map(|t| match t {
            oxigraph::model::Term::Literal(l) => l.value().to_string(),
            oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

//! The memory signal: active notes printed whole at session start, inside their own budget.
//!
//! It used to select every active note with no limit, format all of them, and cut the result at
//! 4,000 bytes. Measured on the operator machine, 2026-09-15: 943 active notes holding 982,525
//! UTF-16 units of text, read to print 6. Board ruling R6 gives the block its own budget,
//! `[budget] memory_chars`, counted in UTF-16 units.
//!
//! THE UNIT HERE IS DELIBERATE AND IT IS NOT THE HOST'S. `memory_chars` counts UTF-16 because it
//! is a READABILITY limit - how much of a block a reader takes in - and not a delivery one. The
//! host counts BYTES, measured 2026-09-20, and the delivery budgets were renamed to `_bytes` that
//! same day. This one keeps its name because its name is honest.
//!
//! DO NOT SWEEP THIS IN WITH THOSE. The sentence above used to end "the UTF-16 units Claude Code
//! counts", which is right about base and wrong about the host - and that exact confusion is what
//! cost a full round. Deleting it would erase the one place the distinction is visible.

use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::model::Term;
use oxigraph::sparql::{QueryResults, QuerySolution};
use oxigraph::store::Store;

use crate::config::{BaseConfig, NamespaceConfig};
use crate::crud;
use crate::emit::u16_len;

/// Notes read per query. A page, not a cap: the next page is read only when every note of the
/// current one fit, so a raised budget is never stopped short by the query.
pub const PAGE: usize = 50;

/// The command that lists every note the block counts: the same tier and the same notes.
pub const LIST_COMMAND: &str = "base learn --list";

const OPEN: &str = "<base-memory>\n";
const CLOSE: &str = "</base-memory>";

/// What the memory signal rendered, and what it counted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MemoryBlock {
    /// The block, or empty when there is no active note.
    pub text: String,
    /// Active notes counted.
    pub total: usize,
    /// Notes printed, each whole.
    pub shown: usize,
    /// Notes read from the graph to print them.
    pub fetched: usize,
}

pub fn run(cwd: &Path, config: &BaseConfig) -> Result<MemoryBlock> {
    if !config.memory.enabled || config.memory.mode == "claude" {
        return Ok(MemoryBlock::default());
    }
    let store = crud::load_workspace_graph(cwd)?;
    render(&store, &config.namespace, config.budget.memory_chars)
}

/// The block for the active notes in `store`, filled with whole notes to at most `budget` UTF-16
/// units. Corrections come first, then feedback, then the rest by type, each newest first: the
/// order this block has always printed. The fill stops at the first note that does not fit, and a
/// last line counts the notes left out and names the command that lists them.
pub fn render(store: &Store, ns: &NamespaceConfig, budget: usize) -> Result<MemoryBlock> {
    let total = count(store, ns)?;
    if total == 0 {
        return Ok(MemoryBlock::default());
    }
    // Room for the count line at its longest, so adding it never takes the block past the budget.
    let mut used = u16_len(OPEN) + u16_len(CLOSE) + u16_len(&withheld_line(total));
    let mut text = String::from(OPEN);
    let (mut shown, mut fetched) = (0, 0);
    'pages: loop {
        let notes = page(store, ns, fetched)?;
        let read = notes.len();
        fetched += read;
        for (note_type, note_text) in notes {
            let line = format!("- [{}] {note_text}\n", label(&note_type));
            let units = u16_len(&line);
            if used + units > budget {
                break 'pages;
            }
            text.push_str(&line);
            used += units;
            shown += 1;
        }
        if read < PAGE {
            break;
        }
    }
    if shown < total {
        text.push_str(&withheld_line(total - shown));
    }
    text.push_str(CLOSE);
    Ok(MemoryBlock {
        text,
        total,
        shown,
        fetched,
    })
}

/// The last line of a block that could not print every note.
fn withheld_line(n: usize) -> String {
    let noun = if n == 1 { "note" } else { "notes" };
    format!("{n} {noun} withheld · all: {LIST_COMMAND}\n")
}

/// The bracketed label: the last path segment of the note's type, with feedback printed as a
/// correction, as it always was.
fn label(note_type: &str) -> &str {
    match note_type.rsplit('/').next().unwrap_or(note_type) {
        "feedback" => "correction",
        other => other,
    }
}

/// The notes both queries read: active notes inside one named graph, the set `base learn --list`
/// lists, with `?rank` 0 for corrections and feedback and 1 for every other type.
fn pattern(ns: &NamespaceConfig) -> String {
    let p = &ns.prefix;
    let transient = crate::ontology::transient::sparql_exclude(ns, "n");
    format!(
        "GRAPH ?g {{\n\
           ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" .\n\
           OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
           {transient}\
         }}\n\
         BIND(IF(REPLACE(STR(?type), \"^.*/\", \"\") IN (\"correction\", \"feedback\"), 0, 1) AS ?rank)"
    )
}

/// Every row [`page`] can return, counted over the same pattern and the same projection, so the
/// withheld count is the notes left out and never a page size.
fn count(store: &Store, ns: &NamespaceConfig) -> Result<usize> {
    let sparql = format!(
        "{}\nSELECT (COUNT(*) AS ?total) WHERE {{\n\
           {{ SELECT DISTINCT ?n ?text ?type ?created ?rank WHERE {{\n{}\n}} }}\n\
         }}",
        crud::prefixes(ns),
        pattern(ns)
    );
    let QueryResults::Solutions(mut rows) = crate::store::query(store, &sparql)? else {
        anyhow::bail!("the memory count query returned no solutions");
    };
    let row = rows
        .next()
        .context("the memory count query returned no row")??;
    match row.get("total") {
        Some(Term::Literal(l)) => l
            .value()
            .parse()
            .with_context(|| format!("the memory count is not a number: {}", l.value())),
        other => anyhow::bail!("the memory count query bound no total: {other:?}"),
    }
}

/// One page of notes in print order: rank, type, newest first, then the note's IRI, so the order
/// is total and a page boundary never moves a note.
fn page(store: &Store, ns: &NamespaceConfig, offset: usize) -> Result<Vec<(String, String)>> {
    let sparql = format!(
        "{}\nSELECT DISTINCT ?n ?text ?type ?created ?rank WHERE {{\n{}\n}}\n\
         ORDER BY ?rank ?type DESC(?created) ?n\n\
         LIMIT {PAGE} OFFSET {offset}",
        crud::prefixes(ns),
        pattern(ns)
    );
    let QueryResults::Solutions(rows) = crate::store::query(store, &sparql)? else {
        anyhow::bail!("the memory page query returned no solutions");
    };
    rows.map(|row| -> Result<(String, String)> {
        let row = row?;
        Ok((term_str(&row, "type"), term_str(&row, "text")))
    })
    .collect()
}

fn term_str(row: &QuerySolution, var: &str) -> String {
    row.get(var)
        .map(|t| match t {
            Term::Literal(l) => l.value().to_string(),
            Term::NamedNode(n) => n.as_str().to_string(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Note = (String, String, String, String);

    /// An in-memory store holding `notes` as (slug, type, text, createdAt), in one named graph.
    fn store_with(ns: &NamespaceConfig, notes: &[Note]) -> Store {
        let uri = &ns.uri;
        let graph = format!("<{uri}graph/ws/memory-test>");
        let mut nq = String::new();
        for (slug, note_type, text, created) in notes {
            let s = format!("<{uri}note/{slug}>");
            nq.push_str(&format!(
                "{s} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{uri}Note> {graph} .\n"
            ));
            nq.push_str(&format!("{s} <{uri}noteText> \"{text}\" {graph} .\n"));
            nq.push_str(&format!("{s} <{uri}noteType> \"{note_type}\" {graph} .\n"));
            nq.push_str(&format!("{s} <{uri}status> \"active\" {graph} .\n"));
            nq.push_str(&format!(
                "{s} <{uri}createdAt> \"{created}\"^^<http://www.w3.org/2001/XMLSchema#dateTime> {graph} .\n"
            ));
        }
        let store = Store::new().expect("in-memory store");
        store
            .load_from_reader(oxigraph::io::RdfFormat::NQuads, nq.as_bytes())
            .expect("the test notes load");
        store
    }

    fn note(slug: &str, note_type: &str, text: &str, created: &str) -> Note {
        (slug.into(), note_type.into(), text.into(), created.into())
    }

    /// `n` dated notes, every third a correction.
    fn many(n: usize) -> Vec<Note> {
        (0..n)
            .map(|i| {
                note(
                    &format!("n{i:03}"),
                    if i % 3 == 0 { "correction" } else { "insight" },
                    &format!("note {i} says something worth keeping"),
                    &format!("2026-08-01T{:02}:{:02}:00Z", 10 + i / 60, i % 60),
                )
            })
            .collect()
    }

    /// Rank 04: the query is bounded. A budget that fills inside the first page reads one page, and
    /// the total still counts every note. New code, so it is proven by mutation: without the page
    /// query's `LIMIT` it reads all 120.
    #[test]
    fn a_budget_that_fills_inside_one_page_reads_one_page() {
        let ns = NamespaceConfig::default();
        let store = store_with(&ns, &many(120));
        let block = render(&store, &ns, 300).expect("renders");
        assert_eq!(block.total, 120, "{}", block.text);
        assert!(
            block.shown > 0,
            "the budget holds at least one note: {}",
            block.text
        );
        assert!(
            block.fetched <= PAGE,
            "read {} notes to print {}: {}",
            block.fetched,
            block.shown,
            block.text
        );
    }

    /// The order this block has always printed (auk's ruling on B19): corrections newest first,
    /// then feedback newest first, then the rest by type, each newest first. A feedback note is
    /// newer than both corrections and an insight is newer than the decision, so an order by date
    /// alone would differ in both ranks. Proven by mutation: drop `?type` from the ORDER BY.
    #[test]
    fn notes_print_corrections_then_feedback_then_the_rest_by_type_each_newest_first() {
        let ns = NamespaceConfig::default();
        let notes = [
            note(
                "c-old",
                "correction",
                "correction written first",
                "2026-08-01T10:00:00Z",
            ),
            note(
                "c-new",
                "correction",
                "correction written third",
                "2026-08-03T10:00:00Z",
            ),
            note(
                "f-mid",
                "feedback",
                "feedback written second",
                "2026-08-02T10:00:00Z",
            ),
            note(
                "f-new",
                "feedback",
                "feedback written last of all",
                "2026-08-09T10:00:00Z",
            ),
            note(
                "i-new",
                "insight",
                "insight newer than the decision",
                "2026-08-08T10:00:00Z",
            ),
            note(
                "d-old",
                "decision",
                "decision older than the insight",
                "2026-08-04T10:00:00Z",
            ),
            note(
                "i-old",
                "insight",
                "insight written fifth",
                "2026-08-05T10:00:00Z",
            ),
        ];
        let store = store_with(&ns, &notes);
        let block = render(&store, &ns, 100_000).expect("renders");
        let printed: Vec<&str> = block
            .text
            .lines()
            .filter(|l| l.starts_with("- ["))
            .collect();
        assert_eq!(
            printed,
            vec![
                "- [correction] correction written third",
                "- [correction] correction written first",
                "- [correction] feedback written last of all",
                "- [correction] feedback written second",
                "- [decision] decision older than the insight",
                "- [insight] insight newer than the decision",
                "- [insight] insight written fifth",
            ],
            "{}",
            block.text
        );
        assert_eq!((block.total, block.shown), (7, 7), "{}", block.text);
    }

    /// A page is not a cap: a budget over every note reads page after page and prints them all.
    /// Proven by mutation: read only the first page and 50 of the 120 print.
    #[test]
    fn a_budget_over_every_note_reads_every_page() {
        let ns = NamespaceConfig::default();
        let store = store_with(&ns, &many(120));
        let block = render(&store, &ns, 1_000_000).expect("renders");
        assert_eq!(
            (block.total, block.shown, block.fetched),
            (120, 120, 120),
            "{}",
            block.text
        );
        assert!(
            !block.text.contains("withheld"),
            "nothing was withheld: {}",
            block.text
        );
    }
}

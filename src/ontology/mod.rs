use anyhow::Result;
use oxigraph::io::RdfFormat;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;

pub mod transient;

/// The ops: vocabulary template, embedded at compile time.
/// Namespace prefix and URI are replaced at runtime from BaseConfig.
const OPS_TTL: &str = include_str!("ops.ttl");

const DEFAULT_PREFIX: &str = "ops";
const DEFAULT_URI: &str = "http://ops-sys.local/ontology#";

/// Load the vocabulary into the store's default graph with configured namespace.
pub fn load_vocabulary(store: &Store, ns: &NamespaceConfig) -> Result<()> {
    let ttl = if ns.prefix == DEFAULT_PREFIX && ns.uri == DEFAULT_URI {
        // Fast path: no replacement needed
        OPS_TTL.to_string()
    } else {
        // Replace namespace URI first, then prefix shorthand
        OPS_TTL
            .replace(DEFAULT_URI, &ns.uri)
            .replace(
                &format!("@prefix {DEFAULT_PREFIX}:"),
                &format!("@prefix {}:", ns.prefix),
            )
            .replace(
                &format!("{DEFAULT_PREFIX}:"),
                &format!("{}:", ns.prefix),
            )
    };
    store.load_from_reader(RdfFormat::Turtle, ttl.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_embeds_and_parses() {
        let store = Store::new().unwrap();
        load_vocabulary(&store, &NamespaceConfig::default()).unwrap();
        assert!(store.len().unwrap() > 0, "Vocabulary should load triples");
    }

    #[test]
    fn vocabulary_loads_with_custom_namespace() {
        let store = Store::new().unwrap();
        let ns = NamespaceConfig {
            prefix: "mybase".into(),
            uri: "http://example.com/base#".into(),
        };
        load_vocabulary(&store, &ns).unwrap();

        // Verify classes exist under new namespace
        let sparql =
            "ASK { <http://example.com/base#Project> a <http://www.w3.org/2000/01/rdf-schema#Class> }";
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(yes) => {
                assert!(yes, "Project class should exist under custom namespace")
            }
            _ => panic!("Expected boolean result"),
        }
    }
}

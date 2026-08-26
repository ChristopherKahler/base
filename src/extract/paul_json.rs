use crate::config::NamespaceConfig;

/// Extract paul.json fields into (predicate, value) triples.
/// Returns None if JSON is invalid or missing required fields.
pub fn extract(content: &str, file_path: &str, ns: &NamespaceConfig) -> Option<Vec<(String, String)>> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let p = &ns.prefix;

    let name = json.get("name")?.as_str()?;
    let mut triples = Vec::new();

    // Type and identity — register as Project (same type as paul.toml)
    // Legacy paul.json projects get the same IRI scheme as paul.toml
    triples.push(("rdf:type".into(), format!("{p}:Project")));
    triples.push((format!("{p}:name"), format!("\"{}\"", name)));
    triples.push((
        format!("{p}:path"),
        format!("\"{}\"", crate::crud::path_literal(file_path)),
    ));

    // Version
    if let Some(version) = json.get("version").and_then(|v| v.as_str()) {
        triples.push((format!("{p}:description"), format!("\"version: {version}\"")));
    }

    // Phase info
    if let Some(phase) = json.get("phase") {
        if let Some(phase_name) = phase.get("name").and_then(|v| v.as_str()) {
            triples.push((format!("{p}:description"), format!("\"phase: {phase_name}\"")));
        }
        if let Some(phase_status) = phase.get("status").and_then(|v| v.as_str()) {
            triples.push((format!("{p}:status"), format!("\"{phase_status}\"")));
        }
    }

    // Milestone
    if let Some(milestone) = json.get("milestone")
        && let Some(ms_name) = milestone.get("name").and_then(|v| v.as_str())
    {
        triples.push((format!("{p}:description"), format!("\"milestone: {ms_name}\"")));
    }

    // Last updated timestamp → lastActive
    if let Some(ts) = json
        .get("timestamps")
        .and_then(|t| t.get("updated_at"))
        .and_then(|v| v.as_str())
    {
        triples.push((format!("{p}:lastActive"), format!("\"{ts}\"^^xsd:dateTime")));
    }

    Some(triples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_paul_json() {
        let content = r#"{
            "name": "base-v2",
            "version": "0.0.0",
            "phase": { "name": "Hook Engine", "status": "complete" },
            "milestone": { "name": "v0.1" },
            "timestamps": { "updated_at": "2026-06-01T10:00:00-05:00" }
        }"#;
        let ns = NamespaceConfig::default();
        let triples = extract(content, ".paul/paul.json", &ns).unwrap();
        assert!(triples.iter().any(|(_, v)| v.contains("base-v2")));
        assert!(triples.iter().any(|(_, v)| v.contains("complete")));
        assert!(triples.iter().any(|(_, v)| v.contains("Project")));
    }

    #[test]
    fn invalid_json_returns_none() {
        let ns = NamespaceConfig::default();
        assert!(extract("not json {{{", "paul.json", &ns).is_none());
    }
}

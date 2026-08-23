use std::path::Path;

use oxigraph::sparql::QueryResults;

use base::config::NamespaceConfig;

#[test]
fn test_vocabulary_loads() {
    let store = oxigraph::store::Store::new().unwrap();
    base::ontology::load_vocabulary(&store, &NamespaceConfig::default()).unwrap();

    // Verify all 16 classes exist
    let classes = [
        "Project",
        "App",
        "Framework",
        "TrackingProject",
        "Task",
        "Decision",
        "Entity",
        "Person",
        "Organization",
        "Goal",
        "Reminder",
        "Workspace",
        "Domain",
        "Rule",
        "Document",
        "PaulProject",
        "Milestone",
        "FileChange",
        "AcceptanceCriteria",
        "AcceptanceCriteriaResult",
        "LedgerEntry",
        "OperatorNote",
    ];
    for class in &classes {
        let sparql = format!(
            "ASK {{ <http://ops-sys.local/ontology#{class}> a <http://www.w3.org/2000/01/rdf-schema#Class> }}"
        );
        match store.query(&sparql).unwrap() {
            QueryResults::Boolean(yes) => assert!(yes, "Class ops:{class} should exist"),
            _ => panic!("Expected boolean result for ASK query"),
        }
    }

    // Verify core predicates exist
    let predicates = [
        "name",
        "description",
        "status",
        "priority",
        "lastActive",
        "createdAt",
        "updatedAt",
        "supersedes",
        "supersededBy",
        "belongsTo",
        "hasDomain",
        "hasRule",
        "hasGoal",
        "hasTask",
        "hasProject",
        "blockedBy",
        "nextAction",
        "revenue",
        "hasMilestone",
        "hasFileChange",
        "hasACResult",
        "hasTag",
        "hasSection",
        "operatorNote",
        "due",
        "text",
        "index",
        "source",
        "filesModified",
    ];
    for pred in &predicates {
        let sparql = format!(
            "ASK {{ <http://ops-sys.local/ontology#{pred}> a <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> }}"
        );
        match store.query(&sparql).unwrap() {
            QueryResults::Boolean(yes) => assert!(yes, "Predicate ops:{pred} should exist"),
            _ => panic!("Expected boolean result for ASK query"),
        }
    }
}

#[test]
fn test_workspace_graph_loads() {
    let path = Path::new("tests/fixtures/sample-workspace.nq");
    let store = base::store::load_graph(path).unwrap();

    // SELECT active projects
    let sparql = r#"
        PREFIX ops: <http://ops-sys.local/ontology#>
        SELECT ?name WHERE {
            GRAPH <http://ops-sys.local/ontology#graph/ws/chris-ai-systems> {
                ?project ops:status "active" ;
                         ops:name ?name .
            }
        }
    "#;

    match store.query(sparql).unwrap() {
        QueryResults::Solutions(solutions) => {
            let names: Vec<String> = solutions
                .map(|s| {
                    let s = s.unwrap();
                    s.get("name").unwrap().to_string()
                })
                .collect();
            assert!(
                names.iter().any(|n| n.contains("CaseGate")),
                "Should find CaseGate v2 as active project, got: {names:?}"
            );
            // skool-community is blocked, not active
            assert!(
                !names.iter().any(|n| n.contains("Skool")),
                "Skool should NOT appear in active projects"
            );
        }
        _ => panic!("Expected solutions"),
    }
}

#[test]
fn test_round_trip() {
    let source = Path::new("tests/fixtures/sample-workspace.nq");
    let store = base::store::load_graph(source).unwrap();

    // Count triples before write-back
    let count_before = store.len().unwrap();
    assert!(count_before > 0, "Should have triples loaded");

    // Write to temp file
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp_path = tmp_dir.path().join("roundtrip.nq");
    base::store::write_back(&store, &tmp_path, base::changelog::Change::Op("test")).unwrap();

    // Reload from written file
    let store2 = base::store::load_graph(&tmp_path).unwrap();
    let count_after = store2.len().unwrap();

    assert_eq!(
        count_before, count_after,
        "Triple count should survive round-trip"
    );

    // Same query should return same results
    let sparql = r#"
        PREFIX ops: <http://ops-sys.local/ontology#>
        SELECT ?name WHERE {
            GRAPH <http://ops-sys.local/ontology#graph/ws/chris-ai-systems> {
                ?project ops:status "active" ;
                         ops:name ?name .
            }
        }
    "#;
    match store2.query(sparql).unwrap() {
        QueryResults::Solutions(solutions) => {
            let names: Vec<String> = solutions
                .map(|s| s.unwrap().get("name").unwrap().to_string())
                .collect();
            assert!(
                names.iter().any(|n| n.contains("CaseGate")),
                "Round-tripped store should still find CaseGate"
            );
        }
        _ => panic!("Expected solutions"),
    }
}

#[test]
fn test_cross_tier_query() {
    let global = Path::new("tests/fixtures/sample-global.nq");
    let workspace = Path::new("tests/fixtures/sample-workspace.nq");
    let store = base::store::load_graphs(&[global, workspace]).unwrap();

    // Traverse: workspace project → hasGoal → global goal, get goal name
    let sparql = r#"
        PREFIX ops: <http://ops-sys.local/ontology#>
        SELECT ?project_name ?goal_name WHERE {
            GRAPH <http://ops-sys.local/ontology#graph/ws/chris-ai-systems> {
                ?project ops:hasGoal ?goal ;
                         ops:name ?project_name .
            }
            GRAPH <http://ops-sys.local/ontology#graph/global> {
                ?goal ops:name ?goal_name .
            }
        }
    "#;

    match store.query(sparql).unwrap() {
        QueryResults::Solutions(solutions) => {
            let rows: Vec<(String, String)> = solutions
                .map(|s| {
                    let s = s.unwrap();
                    (
                        s.get("project_name").unwrap().to_string(),
                        s.get("goal_name").unwrap().to_string(),
                    )
                })
                .collect();
            assert!(!rows.is_empty(), "Cross-tier query should return results");
            assert!(
                rows.iter()
                    .any(|(p, g)| p.contains("CaseGate") && g.contains("North Star")),
                "CaseGate should link to North Star goal, got: {rows:?}"
            );
        }
        _ => panic!("Expected solutions"),
    }
}

#[test]
fn test_atomic_write_no_corrupt() {
    let source = Path::new("tests/fixtures/sample-workspace.nq");
    let store = base::store::load_graph(source).unwrap();

    let tmp_dir = tempfile::tempdir().unwrap();
    let out_path = tmp_dir.path().join("atomic.nq");

    // Write
    base::store::write_back(&store, &out_path, base::changelog::Change::Op("test")).unwrap();

    // Verify file exists
    assert!(
        out_path.exists(),
        "Output file should exist after write_back"
    );

    // Verify temp file was cleaned up (renamed away)
    let tmp_file = out_path.with_extension("nq.tmp");
    assert!(
        !tmp_file.exists(),
        "Temp file should not remain after atomic rename"
    );

    // Verify file is valid TriG by re-parsing
    let store2 = base::store::load_graph(&out_path).unwrap();
    assert!(
        store2.len().unwrap() > 0,
        "Re-parsed file should contain triples"
    );
}

//! `base ast query --file` must stay linear in map size.
//!
//! Measured 2026-09-01 on a real 25,303-entity map: `--file` took 112 s while
//! `--contains` and `--imports` on the same store took 1.1 s, and the hook's
//! compact file query took 0.3 s. The only difference was where the
//! `ops:sourceFile` pattern sat: after the OPTIONAL, oxigraph 0.4 evaluated
//! Join(LeftJoin(BGP, line), BGP{sourceFile}) without the index. This test
//! builds a 20,000-entity map and refuses the query if it takes longer than a
//! bound the fixed shape clears by an order of magnitude (1.6 s measured at
//! 25k). The old shape needed over a minute here.

use base::config::NamespaceConfig;
use std::fmt::Write as _;
use std::time::Instant;

#[test]
fn file_query_is_linear_in_map_size() {
    let tmp = tempfile::tempdir().unwrap();
    let sidecar = tmp.path().join(".base-ast");
    std::fs::create_dir_all(&sidecar).unwrap();

    // 20,000 entities across 400 files, each with type, label, sourceFile and
    // sourceLine — the four predicates the query touches.
    let mut ttl = String::from(
        "@prefix ops: <http://ops-sys.local/ontology#> .\n\
         @prefix code: <http://ops-sys.local/code#> .\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\n",
    );
    for i in 0..20_000u32 {
        let file = format!("src/mod_{}.rs", i % 400);
        writeln!(
            ttl,
            "code:e{i} a ops:Function ;\n    rdfs:label \"fn_{i}()\" ;\n    ops:sourceFile \"{file}\" ;\n    ops:sourceLine {} .\n",
            i % 900 + 1
        )
        .unwrap();
    }
    std::fs::write(sidecar.join("ast.ttl"), ttl).unwrap();

    let ns = NamespaceConfig::default();
    let t0 = Instant::now();
    base::crud::ast_query::file(tmp.path(), &ns, "mod_7.rs").expect("file query runs");
    let took = t0.elapsed();
    assert!(
        took.as_secs_f64() < 15.0,
        "`ast query --file` took {took:?} on a 20k-entity map — the sourceFile pattern \
         has drifted out of the first BGP again (see src/crud/ast_query.rs::file)"
    );
}

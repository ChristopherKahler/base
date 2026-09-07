//! #40: the size fuse guards refreshes as well as first builds, and counts the files the
//! extractor actually turns into entities (markdown included).

use base::hook::automap::{gate, measure_tree, MapPlan};

#[test]
fn a_refresh_over_the_fuse_is_refused_like_a_build() {
    assert_eq!(gate(MapPlan::Refresh, true), MapPlan::NeedsConfirm, "an existing map does not license an unattended refresh of an oversized tree");
    assert_eq!(gate(MapPlan::Build, true), MapPlan::NeedsConfirm);
    assert_eq!(gate(MapPlan::Refresh, false), MapPlan::Refresh);
    assert_eq!(gate(MapPlan::Build, false), MapPlan::Build);
    assert_eq!(gate(MapPlan::Debounced, true), MapPlan::Debounced, "the fuse never turns a skip into a refusal");
}

#[test]
fn markdown_counts_toward_the_fuse() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    for i in 0..7 {
        std::fs::write(docs.join(format!("page{i}.md")), "# heading\n").unwrap();
    }
    std::fs::write(dir.path().join("notes.txt"), "not an entity source").unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    let size = measure_tree(dir.path());
    assert_eq!(size.sources, 8, "7 markdown files and 1 code file are sources; the .txt is not");
    assert!(size.entries >= 9);
}

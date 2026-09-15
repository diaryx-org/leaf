use leaf_ffi::LeafDoc;
#[test]
fn repro() {
    let src = std::fs::read_to_string("/tmp/getting-started-body.md").unwrap();
    let doc = LeafDoc::new(src, "markdown".into()).unwrap();
    let v = doc.view();
    assert!(!v.rows.is_empty());
}

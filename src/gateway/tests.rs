#[test]
fn no_lookup() {
    let error = super::default_gateway().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
}

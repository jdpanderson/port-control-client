//! For the fuzz target: any text as a URL, and as a reference to join. A
//! path must be safe in a request line, and a URL must read back the same
//! from its text.

use super::Url;

pub(crate) fn run(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let base = Url::parse("http://192.168.1.1:5000/dev/rootDesc.xml").expect("a valid URL");
    for url in [Url::parse(text), base.join(text)].into_iter().flatten() {
        assert!(url.path.starts_with('/'));
        assert!(url.path.bytes().all(|b| b.is_ascii_graphic()));
        assert_eq!(Url::parse(&url.to_string()), Some(url.clone()));
    }
}

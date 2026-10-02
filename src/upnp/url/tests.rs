use super::*;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn parse() {
    let u = url("http://192.168.1.1:5000/rootDesc.xml");
    assert_eq!(u.addr, "192.168.1.1:5000".parse().unwrap());
    assert_eq!(u.path, "/rootDesc.xml");
    assert_eq!(url("HTTP://10.0.0.1").to_string(), "http://10.0.0.1:80/");
    assert_eq!(url(" http://10.0.0.1/a?b=c#d ").path, "/a?b=c");
}

#[test]
fn rejects() {
    for s in [
        "https://192.168.1.1/",
        "http://router.local/",
        "http://user@192.168.1.1/",
        "http://192.168.1.1:0/",
        "http://192.168.1.1:99999/",
        "http://[::1]/",
        "http://192.168.1.1/a b",
        "http://192.168.1.1/a\r\nX: y",
    ] {
        assert_eq!(Url::parse(s), None, "{s}");
    }
}

#[test]
fn join() {
    let base = url("http://192.168.1.1:5000/dev/rootDesc.xml");
    assert_eq!(
        base.join("/ctl/IPConn").unwrap().to_string(),
        "http://192.168.1.1:5000/ctl/IPConn"
    );
    assert_eq!(base.join("ctl/IPConn").unwrap().path, "/dev/ctl/IPConn");
    assert_eq!(
        base.join("http://192.168.1.1:6000/x").unwrap().addr.port(),
        6000
    );
    assert_eq!(base.join(""), None);
    assert_eq!(base.join("//192.168.1.2/x"), None);
    assert_eq!(base.join("ftp://192.168.1.1/x"), None);
}

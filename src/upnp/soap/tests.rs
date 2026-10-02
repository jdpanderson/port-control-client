use super::*;

const KIND: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";

#[test]
fn request_body() {
    let body = envelope(
        KIND,
        "AddPortMapping",
        &[
            ("NewRemoteHost", ""),
            ("NewPortMappingDescription", "a<b&\"c\""),
        ],
    );
    assert!(body.contains(&format!(r#"<u:AddPortMapping xmlns:u="{KIND}">"#)));
    assert!(body.contains("<NewRemoteHost></NewRemoteHost>"));
    assert!(body.contains(
        "<NewPortMappingDescription>a&lt;b&amp;&quot;c&quot;</NewPortMappingDescription>"
    ));
    // The body must be XML that a gateway can read.
    roxmltree::Document::parse(&body).unwrap();
}

#[test]
fn response() {
    let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body><u:GetExternalIPAddressResponse xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1">
<NewExternalIPAddress> 203.0.113.5 </NewExternalIPAddress>
</u:GetExternalIPAddressResponse></s:Body></s:Envelope>"#;
    let out = output(xml, "GetExternalIPAddress").unwrap();
    assert_eq!(arg(&out, "NewExternalIPAddress"), Some("203.0.113.5"));
    assert!(output(xml, "AddPortMapping").is_err());
}

#[test]
fn fault_code() {
    let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>
<detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0">
<errorCode>718</errorCode><errorDescription>ConflictInMappingEntry</errorDescription>
</UPnPError></detail></s:Fault></s:Body></s:Envelope>"#;
    assert!(matches!(
        fault(xml),
        Some(Failure::Refused { code: 718, .. })
    ));
    assert!(fault("<html>Internal error</html>").is_none());
}

#[test]
fn temporary_faults() {
    let xml = |code| format!("<UPnPError><errorCode>{code}</errorCode></UPnPError>");
    for (code, want) in [(501, true), (728, true), (606, false), (718, false)] {
        let got = fault(&xml(code));
        let ok = matches!(got, Some(Failure::Refused { temporary, .. }) if temporary == want);
        assert!(ok, "{code}: {got:?}");
    }
}

#[test]
fn characters_xml_cannot_contain() {
    // Found by the fuzzer: a control character made the request
    // invalid XML.
    let value = "a\u{5}b\rc\u{FFFF}d\te\n";
    let body = envelope(
        KIND,
        "AddPortMapping",
        &[("NewPortMappingDescription", value)],
    );
    let doc = roxmltree::Document::parse(&body).unwrap();
    let text = doc
        .descendants()
        .find(|n| n.tag_name().name() == "NewPortMappingDescription")
        .and_then(|n| n.text());
    assert_eq!(text, Some("ab\rcd\te\n"));
}

#[test]
fn escapes() {
    assert_eq!(
        escape("<a & 'b'> \"c\""),
        "&lt;a &amp; &apos;b&apos;&gt; &quot;c&quot;"
    );
}

#[test]
fn error_names() {
    let known = [
        401, 402, 501, 606, 714, 715, 716, 718, 724, 725, 726, 727, 728, 729, 732,
    ];
    for code in known {
        assert_ne!(error_name(code), "unknown", "{code}");
    }
    assert_eq!(error_name(999), "unknown");
}

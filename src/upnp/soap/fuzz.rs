//! For the fuzz target: any text as a SOAP response, and as an argument
//! value. The value must reach the gateway as XML that reads back the same,
//! without the characters that XML cannot contain.

use super::{envelope, fault, output, xml_char};

pub(crate) fn run(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let _ = output(&text, "AddPortMapping");
    let _ = fault(&text);
    let kind = "urn:schemas-upnp-org:service:WANIPConnection:1";
    let body = envelope(
        kind,
        "AddPortMapping",
        &[("NewPortMappingDescription", &text)],
    );
    let doc = roxmltree::Document::parse(&body).expect("the request is XML");
    let value = doc
        .descendants()
        .find(|n| n.tag_name().name() == "NewPortMappingDescription")
        .and_then(|n| n.text())
        .unwrap_or_default();
    let carried: String = text.chars().filter(|c| xml_char(*c)).collect();
    assert_eq!(value, carried);
}

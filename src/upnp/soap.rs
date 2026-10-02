//! SOAP calls to a WAN connection service (UPnP Device Architecture,
//! section 3).

use std::fmt::Write as _;

use super::{device::Service, http};
use crate::error::{Failure, Result};

/// The output arguments of a call, by name.
pub(crate) type Output = Vec<(String, String)>;

/// Calls `action` with `args`, in the order the service defines them.
pub(crate) async fn call(service: &Service, action: &str, args: &[(&str, &str)]) -> Result<Output> {
    let body = envelope(service.kind, action, args);
    let soap_action = format!("\"{}#{action}\"", service.kind);
    let headers = [
        ("Content-Type", "text/xml; charset=\"utf-8\""),
        ("SOAPAction", soap_action.as_str()),
    ];
    let response = http::post(&service.control, &headers, body.as_bytes()).await?;
    let xml = String::from_utf8_lossy(&response.body);
    if response.status == 200 {
        output(&xml, action)
    } else {
        Err(fault(&xml).unwrap_or(Failure::HttpStatus(response.status)))
    }
}

/// The value of output argument `name`.
pub(crate) fn arg<'a>(output: &'a Output, name: &str) -> Option<&'a str> {
    output
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

fn envelope(kind: &str, action: &str, args: &[(&str, &str)]) -> String {
    let mut s = String::from(concat!(
        r#"<?xml version="1.0"?>"#,
        "\r\n",
        r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" "#,
        r#"s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body>"#,
    ));
    let _ = write!(s, r#"<u:{action} xmlns:u="{kind}">"#);
    for (name, value) in args {
        let _ = write!(s, "<{name}>{}</{name}>", escape(value));
    }
    let _ = write!(s, "</u:{action}></s:Body></s:Envelope>\r\n");
    s
}

/// `s` as XML text. Characters that XML cannot contain are left out. A
/// carriage return is written as a reference, because XML parsers turn a
/// plain one into a line feed.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|c| xml_char(*c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

/// A character that XML 1.0 can carry (section 2.2): not most control
/// characters, and not U+FFFE or U+FFFF.
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

fn output(xml: &str, action: &str) -> Result<Output> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|_| Failure::BadReply("bad SOAP response"))?;
    let name = format!("{action}Response");
    let response = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .ok_or(Failure::BadReply("no SOAP response element"))?;
    Ok(response
        .children()
        .filter(|c| c.is_element())
        .map(|c| {
            let value = c.text().unwrap_or_default().trim();
            (c.tag_name().name().to_owned(), value.to_owned())
        })
        .collect())
}

/// The UPnP error in a SOAP fault, if it has one.
fn fault(xml: &str) -> Option<Failure> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let code: u16 = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "errorCode")?
        .text()?
        .trim()
        .parse()
        .ok()?;
    Some(Failure::Refused {
        code,
        name: error_name(code),
        // A general failure, or no free ports: the gateway may still have
        // the mapping, and the same request may work later.
        temporary: matches!(code, 501 | 728),
    })
}

/// Failure codes from the UPnP Device Architecture and WANIPConnection.
fn error_name(code: u16) -> &'static str {
    match code {
        401 => "Invalid Action",
        402 => "Invalid Args",
        501 => "Action Failed",
        606 => "Action not authorized",
        714 => "NoSuchEntryInArray",
        715 => "WildCardNotPermittedInSrcIP",
        716 => "WildCardNotPermittedInExtPort",
        718 => "ConflictInMappingEntry",
        724 => "SamePortValuesRequired",
        725 => "OnlyPermanentLeasesSupported",
        726 => "RemoteHostOnlySupportsWildcard",
        727 => "ExternalPortOnlySupportsWildcard",
        728 => "NoPortMapsAvailable",
        729 => "ConflictWithOtherMechanisms",
        732 => "WildCardNotPermittedInIntPort",
        _ => "unknown",
    }
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;

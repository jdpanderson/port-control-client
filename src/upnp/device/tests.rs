use super::*;

/// The shape of a miniupnpd description: the connection service is two
/// devices down.
const NESTED: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
        <controlURL>/ctl/L3F</controlURL>
      </service>
    </serviceList>
    <deviceList>
      <device>
        <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
        <deviceList>
          <device>
            <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
            <serviceList>
              <service>
                <serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
                <controlURL>/ctl/PPPConn</controlURL>
              </service>
              <service>
                <serviceType>
                  urn:schemas-upnp-org:service:WANIPConnection:1
                </serviceType>
                <controlURL>/ctl/IPConn</controlURL>
              </service>
            </serviceList>
          </device>
        </deviceList>
      </device>
    </deviceList>
  </device>
</root>"#;

fn location() -> Url {
    Url::parse("http://192.168.1.1:5000/rootDesc.xml").unwrap()
}

#[test]
fn nested_services_best_first() {
    let found = services(NESTED, &location()).unwrap();
    let got: Vec<_> = found
        .iter()
        .map(|s| (s.kind, s.control.to_string()))
        .collect();
    assert_eq!(
        got,
        [
            (SERVICES[1], "http://192.168.1.1:5000/ctl/IPConn".to_owned()),
            (
                SERVICES[2],
                "http://192.168.1.1:5000/ctl/PPPConn".to_owned()
            ),
        ]
    );
}

#[test]
fn url_base_and_other_hosts() {
    let xml = r#"<root xmlns="urn:schemas-upnp-org:device-1-0">
          <URLBase>http://192.168.1.1:49000/</URLBase>
          <device><serviceList>
            <service>
              <serviceType>urn:schemas-upnp-org:service:WANIPConnection:2</serviceType>
              <controlURL>upnp/control/WANIPConn1</controlURL>
            </service>
            <service>
              <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
              <controlURL>http://192.168.1.99/ctl</controlURL>
            </service>
          </serviceList></device>
        </root>"#;
    let found = services(xml, &location()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, SERVICES[0]);
    assert_eq!(
        found[0].control.to_string(),
        "http://192.168.1.1:49000/upnp/control/WANIPConn1"
    );
}

#[test]
fn bad_xml() {
    assert!(services("<root><device>", &location()).is_err());
    assert!(services("not xml", &location()).is_err());
}

#[test]
fn services_without_a_type_or_control_url() {
    let xml = r#"<root><device><serviceList>
          <service><controlURL>/a</controlURL></service>
          <service>
            <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
          </service>
          <service>
            <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
            <controlURL>ftp://192.168.1.1/x</controlURL>
          </service>
        </serviceList></device></root>"#;
    assert_eq!(services(xml, &location()).unwrap(), []);
}

use super::{
    DnsClient, TXT_TTL, TxtName, TxtRecord,
    api::{ApiClient, ApiRequest},
    longest_zone, sigv4,
};
use anyhow::anyhow;
use async_trait::async_trait;
use hyper::Method;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use time::OffsetDateTime;

pub const API_URL: &str = "https://route53.amazonaws.com";
const API_PATH: &str = "/2013-04-01";
/// Route 53 is a global service, and requests to it are signed for this region.
const REGION: &str = "us-east-1";
const SERVICE: &str = "route53";
/// The unreserved characters of RFC 3986 stay as they are in a signed query.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub struct Route53 {
    api: ApiClient,
    access_key_id: String,
    secret_access_key: String,
}

impl Route53 {
    pub fn new(api: ApiClient, access_key_id: &str, secret_access_key: &str) -> Self {
        Self {
            api,
            access_key_id: access_key_id.to_string(),
            secret_access_key: secret_access_key.to_string(),
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &str,
        body: Vec<u8>,
    ) -> anyhow::Result<String> {
        let amz_date = sigv4::amz_date(OffsetDateTime::now_utc());
        let full_path = format!("{}{path}", self.api.base_path());
        let authorization = sigv4::authorization(
            &self.access_key_id,
            &self.secret_access_key,
            &sigv4::SigningInput {
                method: method.as_str(),
                host: self.api.authority(),
                path: &full_path,
                query,
                amz_date: &amz_date,
                payload: &body,
                region: REGION,
                service: SERVICE,
            },
        )?;
        let target = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        let mut request = ApiRequest::new(method, target)
            .header("x-amz-date", amz_date)
            .header("authorization", authorization);
        if !body.is_empty() {
            request = request.body("text/xml", body);
        }
        let response = self.api.send(request).await?;
        Ok(String::from_utf8_lossy(&response).into_owned())
    }

    async fn hosted_zones(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut zones = Vec::new();
        let mut query = String::new();
        loop {
            let path = format!("{API_PATH}/hostedzone");
            let xml = self.call(Method::GET, &path, &query, Vec::new()).await?;
            zones.extend(parse_hosted_zones(&xml));
            match (
                xml_value(&xml, "IsTruncated"),
                xml_value(&xml, "NextMarker"),
            ) {
                (Some("true"), Some(marker)) => {
                    query = format!("marker={}", utf8_percent_encode(marker, QUERY_VALUE));
                }
                _ => return Ok(zones),
            }
        }
    }

    async fn zone_id(&self, fqdn: &str) -> anyhow::Result<String> {
        let zones = self.hosted_zones().await?;
        longest_zone(fqdn, &zones, zone_name)
            .map(|(_, (_, id))| id.clone())
            .ok_or_else(|| anyhow!("no public Route 53 hosted zone contains {fqdn}"))
    }

    async fn change(&self, record: &TxtRecord, action: &str) -> anyhow::Result<()> {
        let path = format!("{API_PATH}/hostedzone/{}/rrset/", record.zone);
        let body = change_batch(action, &record.fqdn, &record.values);
        self.call(Method::POST, &path, "", body.into_bytes())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl DnsClient for Route53 {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        let zone = self.zone_id(&name.fqdn).await?;
        let record = TxtRecord::new(name, zone);
        // One UPSERT holds every value, because Route 53 keeps one record set for each name.
        self.change(&record, "UPSERT").await?;
        Ok(record)
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        // A DELETE must repeat the values and the TTL of the record set exactly.
        self.change(record, "DELETE").await
    }
}

/// Public hosted zones as `(name, id)` pairs. The name ends with a dot.
fn parse_hosted_zones(xml: &str) -> Vec<(String, String)> {
    xml.split("<HostedZone>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</HostedZone>").next()?;
            if xml_value(block, "PrivateZone") == Some("true") {
                return None;
            }
            let id = xml_value(block, "Id")?.trim_start_matches("/hostedzone/");
            let name = xml_value(block, "Name")?;
            Some((name.to_string(), id.to_string()))
        })
        .collect()
}

fn zone_name(zone: &(String, String)) -> &str {
    &zone.0
}

/// The text of the first `<tag>` element.
fn xml_value<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))?;
    Some(&xml[start..start + end])
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn change_batch(action: &str, fqdn: &str, values: &[String]) -> String {
    let records: String = values
        .iter()
        .map(|value| {
            format!(
                "<ResourceRecord><Value>{}</Value></ResourceRecord>",
                xml_escape(&format!("\"{value}\""))
            )
        })
        .collect();
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<ChangeResourceRecordSetsRequest xmlns="https://route53.amazonaws.com/doc/2013-04-01/">"#,
            "<ChangeBatch><Changes><Change><Action>{action}</Action><ResourceRecordSet>",
            "<Name>{name}.</Name><Type>TXT</Type><TTL>{ttl}</TTL>",
            "<ResourceRecords>{records}</ResourceRecords>",
            "</ResourceRecordSet></Change></Changes></ChangeBatch>",
            "</ChangeResourceRecordSetsRequest>"
        ),
        action = action,
        name = xml_escape(fqdn),
        ttl = TXT_TTL,
        records = records,
    )
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn private_zones_are_skipped() {
        let xml = concat!(
            "<ListHostedZonesResponse><HostedZones>",
            "<HostedZone><Id>/hostedzone/ZPRIVATE</Id><Name>example.com.</Name>",
            "<Config><PrivateZone>true</PrivateZone></Config></HostedZone>",
            "<HostedZone><Id>/hostedzone/ZPUBLIC</Id><Name>example.com.</Name>",
            "<Config><PrivateZone>false</PrivateZone></Config></HostedZone>",
            "</HostedZones><IsTruncated>false</IsTruncated></ListHostedZonesResponse>"
        );
        assert_eq!(
            parse_hosted_zones(xml),
            vec![("example.com.".to_string(), "ZPUBLIC".to_string())]
        );
    }

    #[test]
    fn a_change_batch_quotes_and_escapes_every_value() {
        let batch = change_batch(
            "UPSERT",
            "_acme-challenge.example.com",
            &["a<b".to_string(), "c".to_string()],
        );
        assert!(batch.contains("<Action>UPSERT</Action>"));
        assert!(batch.contains("<Name>_acme-challenge.example.com.</Name>"));
        assert!(batch.contains(
            "<ResourceRecord><Value>\"a&lt;b\"</Value></ResourceRecord><ResourceRecord><Value>\"c\"</Value></ResourceRecord>"
        ));
    }
}

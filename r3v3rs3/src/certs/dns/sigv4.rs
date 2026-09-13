//! AWS Signature Version 4 for requests that sign only the `host` and `x-amz-date` headers.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

type HmacSha256 = Hmac<Sha256>;

pub struct SigningInput<'a> {
    pub method: &'a str,
    pub host: &'a str,
    /// The URI path, already encoded.
    pub path: &'a str,
    /// The canonical query string: sorted and encoded, without the `?`.
    pub query: &'a str,
    /// The time in the `x-amz-date` format, for example `20150830T123600Z`.
    pub amz_date: &'a str,
    pub payload: &'a [u8],
    pub region: &'a str,
    pub service: &'a str,
}

pub fn amz_date(time: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

/// The value of the `authorization` header.
pub fn authorization(
    access_key_id: &str,
    secret_access_key: &str,
    input: &SigningInput,
) -> anyhow::Result<String> {
    let date = input.amz_date.get(..8).unwrap_or_default();
    let scope = format!("{date}/{}/{}/aws4_request", input.region, input.service);
    let canonical_request = format!(
        "{}\n{}\n{}\nhost:{}\nx-amz-date:{}\n\nhost;x-amz-date\n{}",
        input.method,
        input.path,
        input.query,
        input.host,
        input.amz_date,
        hex::encode(Sha256::digest(input.payload))
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        input.amz_date,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let mut key = format!("AWS4{secret_access_key}").into_bytes();
    for part in [date, input.region, input.service, "aws4_request"] {
        key = hmac(&key, part.as_bytes())?;
    }
    let signature = hex::encode(hmac(&key, string_to_sign.as_bytes())?);
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders=host;x-amz-date, Signature={signature}"
    ))
}

fn hmac(key: &[u8], data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(key)?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

#[cfg(test)]
mod test {
    use super::*;

    /// The `get-vanilla` case of the AWS Signature Version 4 test suite.
    #[test]
    fn the_signature_matches_the_aws_test_suite() {
        let input = SigningInput {
            method: "GET",
            host: "example.amazonaws.com",
            path: "/",
            query: "",
            amz_date: "20150830T123600Z",
            payload: b"",
            region: "us-east-1",
            service: "service",
        };
        let header = authorization(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            &input,
        )
        .unwrap();
        assert_eq!(
            header,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn amz_date_uses_the_basic_iso_8601_format() {
        let time = OffsetDateTime::from_unix_timestamp(1_440_938_160).unwrap();
        assert_eq!(amz_date(time), "20150830T123600Z");
    }
}

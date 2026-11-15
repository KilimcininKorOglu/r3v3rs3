//! JSON Web Tokens signed with RS256, which a Google Cloud service account and a GitHub App send
//! to their token endpoints.

use anyhow::anyhow;
use aws_lc_rs::{
    rand::SystemRandom,
    signature::{KeyPair as _, RSA_PKCS1_SHA256, RsaKeyPair},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

/// A JWT with `claims`, signed with `key`. The error message never contains the key.
pub fn rs256(key: &RsaKeyPair, claims: &Value) -> anyhow::Result<String> {
    let header = json!({ "alg": "RS256", "typ": "JWT" });
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let mut signature = vec![0; key.public_key().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        message.as_bytes(),
        &mut signature,
    )
    .map_err(|_| anyhow!("failed to sign the token"))?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

//! Admin accounts: an Argon2 password hash, an optional TOTP secret, and WebAuthn passkeys.

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use r3v3rs3_api::auth::{Account, LoginMethod, LoginRequest, LoginResponse};
use r3v3rs3_api::error::Error;
use std::collections::HashMap;
use totp_rs::{Builder, Secret, Totp};
use tracing::error;

pub fn new_account(password: &str, totp: bool) -> anyhow::Result<Account> {
    let password = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|_| anyhow::anyhow!("failed to hash password"))?
        .to_string();
    Ok(Account {
        password,
        totp: totp.then(|| Totp::default().secret().to_base32()),
        ..Default::default()
    })
}

/// Checks a login request against the account of its user name.
pub fn verify(account: Option<&Account>, request: LoginRequest) -> Result<LoginResponse, Error> {
    let name = &request.username;
    let Some(account) = account else {
        error!(%name, "account not found: {name}");
        return Err(Error::InvalidLoginCredentials);
    };
    match request.method {
        LoginMethod::Password { password } => verify_password(account, &password),
        LoginMethod::Totp { token } => verify_totp(name, account, &token),
        LoginMethod::Passkey {
            credential_id,
            authenticator_data,
            client_data_json,
            signature,
        } => {
            let mut clone = account.clone();
            let (resp, _) = verify_passkey_credential(
                &mut clone,
                &credential_id,
                &authenticator_data,
                &client_data_json,
                &signature,
            )?;
            Ok(resp)
        }
    }
}

/// Checks a login request against stored accounts and updates passkey counter if needed.
pub fn verify_and_update(
    accounts: &mut HashMap<String, Account>,
    request: LoginRequest,
) -> Result<(LoginResponse, bool), Error> {
    match request.method {
        LoginMethod::Password { password } => {
            let account = accounts
                .get(&request.username)
                .ok_or(Error::InvalidLoginCredentials)?;
            let response = verify_password(account, &password)?;
            Ok((response, false))
        }
        LoginMethod::Totp { token } => {
            let account = accounts
                .get(&request.username)
                .ok_or(Error::InvalidLoginCredentials)?;
            let response = verify_totp(&request.username, account, &token)?;
            Ok((response, false))
        }
        LoginMethod::Passkey {
            credential_id,
            authenticator_data,
            client_data_json,
            signature,
        } => {
            let account = find_account_for_passkey(accounts, &request.username, &credential_id)?;
            verify_passkey_credential(
                account,
                &credential_id,
                &authenticator_data,
                &client_data_json,
                &signature,
            )
        }
    }
}

fn find_account_for_passkey<'a>(
    accounts: &'a mut HashMap<String, Account>,
    username: &str,
    credential_id: &str,
) -> Result<&'a mut Account, Error> {
    if !username.is_empty() {
        return accounts
            .get_mut(username)
            .ok_or(Error::InvalidLoginCredentials);
    }
    accounts
        .values_mut()
        .find(|acc| acc.passkeys.iter().any(|pk| pk.id == credential_id))
        .ok_or(Error::InvalidLoginCredentials)
}

fn verify_password(account: &Account, password: &str) -> Result<LoginResponse, Error> {
    if let Err(err) =
        Argon2::default().verify_password(password.as_bytes(), account.password.as_str())
    {
        error!(%err, "failed to verify password: {err}");
        return Err(Error::InvalidLoginCredentials);
    }
    if account.totp.is_some() {
        return Ok(LoginResponse::TotpRequired);
    }
    Ok(LoginResponse::Success)
}

fn verify_totp(name: &str, account: &Account, token: &str) -> Result<LoginResponse, Error> {
    let Some(encoded) = &account.totp else {
        error!(%name, "totp not found: {name}");
        return Err(Error::InvalidLoginCredentials);
    };
    let secret = Secret::try_from_base32(encoded).map_err(|_| Error::InvalidLoginCredentials)?;
    let totp = Builder::new().with_secret(secret).build_noncompliant();
    if totp.check_current(token).is_some() {
        return Ok(LoginResponse::Success);
    }
    Err(Error::InvalidLoginCredentials)
}

pub fn decode_base64_or_hex(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim().trim_end_matches('=');
    if let Ok(bytes) = hex::decode(trimmed) {
        return Some(bytes);
    }
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(trimmed) {
        return Some(bytes);
    }
    STANDARD_NO_PAD.decode(trimmed).ok()
}

pub fn decode_p256_public_key(input: &str) -> Option<Vec<u8>> {
    let bytes = decode_base64_or_hex(input)?;
    if bytes.len() == 65 && bytes[0] == 0x04 {
        Some(bytes)
    } else if bytes.len() == 91 && bytes[26] == 0x04 {
        Some(bytes[26..].to_vec())
    } else {
        None
    }
}

fn check_auth_data(
    auth_data: &[u8],
    stored_counter: u32,
    expected_rp_id: Option<&str>,
) -> Result<u32, Error> {
    if auth_data.len() < 37 {
        error!("authenticator data too short: {}", auth_data.len());
        return Err(Error::InvalidLoginCredentials);
    }
    if let Some(rp_id) = expected_rp_id {
        let expected_hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, rp_id.as_bytes());
        if &auth_data[..32] != expected_hash.as_ref() {
            error!("rpIdHash mismatch in authenticator data");
            return Err(Error::InvalidLoginCredentials);
        }
    }
    let flags = auth_data[32];
    if (flags & 0x01) == 0 {
        error!("user present (UP) flag not set");
        return Err(Error::InvalidLoginCredentials);
    }
    let counter = u32::from_be_bytes(auth_data[33..37].try_into().unwrap());
    if stored_counter > 0 && counter > 0 && counter <= stored_counter {
        error!(
            stored_counter,
            counter, "replay or cloned authenticator detected"
        );
        return Err(Error::InvalidLoginCredentials);
    }
    Ok(counter)
}

fn verify_p256_signature(
    public_key_text: &str,
    auth_data: &[u8],
    client_data: &[u8],
    sig_bytes: &[u8],
) -> Result<(), Error> {
    let pk_bytes = decode_p256_public_key(public_key_text).ok_or_else(|| {
        error!("invalid stored passkey public key");
        Error::InvalidLoginCredentials
    })?;

    let client_hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, client_data);
    let mut signed_message = Vec::with_capacity(auth_data.len() + 32);
    signed_message.extend_from_slice(auth_data);
    signed_message.extend_from_slice(client_hash.as_ref());

    let verifier = UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &pk_bytes);
    verifier.verify(&signed_message, sig_bytes).map_err(|err| {
        error!(%err, "passkey signature verification failed");
        Error::InvalidLoginCredentials
    })
}

fn verify_passkey_credential(
    account: &mut Account,
    credential_id: &str,
    auth_data_b64: &str,
    client_data_b64: &str,
    sig_b64: &str,
) -> Result<(LoginResponse, bool), Error> {
    let credential = account
        .passkeys
        .iter_mut()
        .find(|pk| pk.id == credential_id)
        .ok_or(Error::InvalidLoginCredentials)?;

    let auth_data = decode_base64_or_hex(auth_data_b64).ok_or(Error::InvalidLoginCredentials)?;
    let client_data =
        decode_base64_or_hex(client_data_b64).ok_or(Error::InvalidLoginCredentials)?;
    let sig_bytes = decode_base64_or_hex(sig_b64).ok_or(Error::InvalidLoginCredentials)?;

    let client_data_val: Option<serde_json::Value> = serde_json::from_slice(&client_data).ok();
    let expected_rp_id = client_data_val
        .as_ref()
        .and_then(|v| v.get("origin"))
        .and_then(|v| v.as_str())
        .and_then(|o| url::Url::parse(o).ok())
        .and_then(|u| u.host_str().map(|h| h.to_string()));

    let counter = check_auth_data(&auth_data, credential.counter, expected_rp_id.as_deref())?;
    verify_p256_signature(&credential.public_key, &auth_data, &client_data, &sig_bytes)?;

    let updated = if counter > credential.counter {
        credential.counter = counter;
        true
    } else {
        false
    };
    Ok((LoginResponse::Success, updated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use r3v3rs3_api::auth::PasskeyCredential;

    #[test]
    fn passkey_verification_flow_and_counter_update() -> anyhow::Result<()> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)?;
        let key_pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref())?;

        let public_key_sec1 = hex::encode(key_pair.public_key().as_ref());
        let credential_id = "test-cred-123";

        let account = Account {
            password: "hashed_password".into(),
            passkeys: vec![PasskeyCredential {
                id: credential_id.into(),
                name: "My Passkey".into(),
                public_key: public_key_sec1,
                counter: 5,
                created_at: 1000,
            }],
            ..Default::default()
        };

        let rp_id = "localhost";
        let rp_hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, rp_id.as_bytes());
        // Simulated authenticatorData: 32 bytes rpIdHash + 1 byte flags (0x01 UP) + 4 bytes counter
        let mut auth_data = rp_hash.as_ref().to_vec();
        auth_data.push(0x01); // UP flag set
        auth_data.extend_from_slice(&10u32.to_be_bytes()); // counter = 10

        let client_data_json =
            br#"{"type":"webauthn.get","challenge":"abc","origin":"http://localhost:46492"}"#;
        let client_hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, client_data_json);

        let mut signed = Vec::new();
        signed.extend_from_slice(&auth_data);
        signed.extend_from_slice(client_hash.as_ref());

        let sig = key_pair.sign(&rng, &signed)?;
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.as_ref());
        let auth_b64 = URL_SAFE_NO_PAD.encode(&auth_data);
        let client_b64 = URL_SAFE_NO_PAD.encode(client_data_json);

        let mut accounts = HashMap::new();
        accounts.insert("admin".into(), account.clone());

        // 1. Successful login with username
        let (resp, updated) = verify_and_update(
            &mut accounts,
            LoginRequest {
                username: "admin".into(),
                method: LoginMethod::Passkey {
                    credential_id: credential_id.into(),
                    authenticator_data: auth_b64.clone(),
                    client_data_json: client_b64.clone(),
                    signature: sig_b64.clone(),
                },
                insecure: false,
            },
        )?;
        assert!(matches!(resp, LoginResponse::Success));
        assert!(updated);
        assert_eq!(accounts["admin"].passkeys[0].counter, 10);

        // 2. Successful usernameless login (resident key) with incremented counter = 11
        let mut auth_data2 = rp_hash.as_ref().to_vec();
        auth_data2.push(0x01);
        auth_data2.extend_from_slice(&11u32.to_be_bytes()); // counter = 11

        let mut signed2 = Vec::new();
        signed2.extend_from_slice(&auth_data2);
        signed2.extend_from_slice(client_hash.as_ref());

        let sig2 = key_pair.sign(&rng, &signed2)?;
        let sig2_b64 = URL_SAFE_NO_PAD.encode(sig2.as_ref());
        let auth2_b64 = URL_SAFE_NO_PAD.encode(&auth_data2);

        let (resp2, _) = verify_and_update(
            &mut accounts,
            LoginRequest {
                username: String::new(),
                method: LoginMethod::Passkey {
                    credential_id: credential_id.into(),
                    authenticator_data: auth2_b64,
                    client_data_json: client_b64.clone(),
                    signature: sig2_b64,
                },
                insecure: false,
            },
        )?;
        assert!(matches!(resp2, LoginResponse::Success));
        assert_eq!(accounts["admin"].passkeys[0].counter, 11);

        // 3. Counter replay detection: lower or same counter should fail
        let mut stale_auth_data = rp_hash.as_ref().to_vec();
        stale_auth_data.push(0x01);
        stale_auth_data.extend_from_slice(&11u32.to_be_bytes()); // counter still 11, stored is 11
        let stale_auth_b64 = URL_SAFE_NO_PAD.encode(&stale_auth_data);

        let res = verify_and_update(
            &mut accounts,
            LoginRequest {
                username: "admin".into(),
                method: LoginMethod::Passkey {
                    credential_id: credential_id.into(),
                    authenticator_data: stale_auth_b64,
                    client_data_json: client_b64,
                    signature: sig_b64,
                },
                insecure: false,
            },
        );
        assert!(res.is_err());

        // 4. rpIdHash mismatch rejected
        let mut bad_auth = vec![0x99; 32];
        bad_auth.push(0x01);
        bad_auth.extend_from_slice(&20u32.to_be_bytes());
        assert!(check_auth_data(&bad_auth, 0, Some("localhost")).is_err());

        Ok(())
    }
}

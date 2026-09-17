//! Admin accounts: an Argon2 password hash and an optional TOTP secret.

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use r3v3rs3_api::auth::{Account, LoginMethod, LoginRequest, LoginResponse};
use r3v3rs3_api::error::Error;
use totp_rs::{Builder, Secret, Totp};
use tracing::error;

pub fn new_account(password: &str, totp: bool) -> anyhow::Result<Account> {
    let salt = SaltString::generate(OsRng);
    let password = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
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
    }
}

fn verify_password(account: &Account, password: &str) -> Result<LoginResponse, Error> {
    let parsed_hash = PasswordHash::new(&account.password).map_err(|err| {
        error!(%err, "failed to parse password hash: {err}");
        Error::InvalidLoginCredentials
    })?;
    if let Err(err) = Argon2::default().verify_password(password.as_bytes(), &parsed_hash) {
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

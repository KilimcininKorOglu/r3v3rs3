//! Sign-in sessions of the admin API and of the session authentication of the proxies.

use crate::clock::unix_ms;
use r3v3rs3_api::app::AdminConfig;
use r3v3rs3_api::error::Error;
use rand::distributions::{Alphanumeric, DistString};
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The shortest session lifetime. A shorter `session_expiry` uses this lifetime.
pub const MINIMUM_SESSION_EXPIRY: Duration = Duration::from_secs(5 * 60);

const TOKEN_LENGTH: usize = 32;

/// The lifetime of the admin and proxy sessions.
pub fn expiry(config: &AdminConfig) -> Duration {
    config.session_expiry.max(MINIMUM_SESSION_EXPIRY)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionScope {
    /// A sign-in to the admin API that waits for the TOTP code.
    Login,
    Admin,
    /// A sign-in through the session authentication of a proxy.
    Proxy,
}

impl SessionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Admin => "admin",
            Self::Proxy => "proxy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The user name of an admin session, or the host of a proxy session.
    pub subject: String,
    /// The start of the session in seconds since the Unix epoch. The nodes of a cluster share the
    /// record, so the start is a wall-clock time.
    pub started_at: u64,
}

impl SessionRecord {
    pub fn new(subject: &str) -> Self {
        Self {
            subject: subject.to_string(),
            started_at: unix_now(),
        }
    }

    pub fn is_active(&self, expiry: Duration) -> bool {
        unix_now().saturating_sub(self.started_at) < expiry.as_secs()
    }
}

pub fn new_token() -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), TOKEN_LENGTH)
}

fn unix_now() -> u64 {
    unix_ms() / 1000
}

#[async_trait::async_trait]
pub trait SessionBackend: Send + Sync {
    /// Starts a session and returns its token.
    async fn create(
        &self,
        scope: SessionScope,
        subject: &str,
        expiry: Duration,
    ) -> Result<String, Error>;

    async fn get(&self, scope: SessionScope, token: &str) -> Result<Option<SessionRecord>, Error>;

    async fn remove(&self, scope: SessionScope, token: &str) -> Result<(), Error>;

    /// Removes the sessions that are older than `expiry`.
    async fn remove_expired(&self, expiry: Duration) -> Result<(), Error>;
}

type LocalMap = HashMap<(SessionScope, String), SessionRecord>;

/// The sessions of a server without a cluster, in memory.
#[derive(Default)]
pub struct LocalSessions {
    sessions: Mutex<LocalMap>,
}

impl LocalSessions {
    fn sessions(&self) -> MutexGuard<'_, LocalMap> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl SessionBackend for LocalSessions {
    /// Also drops the expired sessions, so the map does not grow without a limit.
    async fn create(
        &self,
        scope: SessionScope,
        subject: &str,
        expiry: Duration,
    ) -> Result<String, Error> {
        let token = new_token();
        let mut sessions = self.sessions();
        sessions.retain(|_, record| record.is_active(expiry));
        sessions.insert((scope, token.clone()), SessionRecord::new(subject));
        Ok(token)
    }

    async fn get(&self, scope: SessionScope, token: &str) -> Result<Option<SessionRecord>, Error> {
        Ok(self.sessions().get(&(scope, token.to_string())).cloned())
    }

    async fn remove(&self, scope: SessionScope, token: &str) -> Result<(), Error> {
        self.sessions().remove(&(scope, token.to_string()));
        Ok(())
    }

    async fn remove_expired(&self, expiry: Duration) -> Result<(), Error> {
        self.sessions().retain(|_, record| record.is_active(expiry));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    #[tokio::test]
    async fn a_session_is_found_only_in_its_scope_until_it_is_removed() -> Result<(), Error> {
        let sessions = LocalSessions::default();
        let token = sessions.create(SessionScope::Admin, "admin", HOUR).await?;

        let record = sessions.get(SessionScope::Admin, &token).await?;
        assert_eq!(
            record.map(|record| record.subject).as_deref(),
            Some("admin")
        );
        assert_eq!(sessions.get(SessionScope::Login, &token).await?, None);
        assert_eq!(sessions.get(SessionScope::Admin, "other").await?, None);

        sessions.remove(SessionScope::Admin, &token).await?;
        assert_eq!(sessions.get(SessionScope::Admin, &token).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn expired_sessions_are_removed() -> Result<(), Error> {
        let sessions = LocalSessions::default();
        let token = sessions
            .create(SessionScope::Proxy, "example.com", HOUR)
            .await?;
        sessions.remove_expired(HOUR).await?;
        assert!(sessions.get(SessionScope::Proxy, &token).await?.is_some());
        sessions.remove_expired(Duration::ZERO).await?;
        assert_eq!(sessions.get(SessionScope::Proxy, &token).await?, None);
        Ok(())
    }

    #[test]
    fn a_record_is_active_for_its_expiry() {
        let old = SessionRecord {
            subject: "admin".into(),
            started_at: unix_now() - 120,
        };
        assert!(old.is_active(Duration::from_secs(180)));
        assert!(!old.is_active(Duration::from_secs(60)));
        let config = AdminConfig {
            session_expiry: Duration::from_secs(1),
            ..Default::default()
        };
        assert_eq!(expiry(&config), MINIMUM_SESSION_EXPIRY);
    }
}

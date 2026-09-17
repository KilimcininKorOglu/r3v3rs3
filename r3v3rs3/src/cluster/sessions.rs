//! Sessions in the cluster store, so a session that one node starts is valid on every node.

use super::storage::{KvStorage, unavailable};
use crate::sessions::{SessionBackend, SessionRecord, SessionScope, new_token};
use r3v3rs3_api::error::Error;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tracing::error;

impl KvStorage {
    /// The key holds the SHA-256 digest of the token, so the store never holds a token.
    fn session_key(&self, scope: SessionScope, token: &str) -> String {
        let digest = hex::encode(Sha256::digest(token.as_bytes()));
        self.layout().session(scope, &digest)
    }
}

#[async_trait::async_trait]
impl SessionBackend for KvStorage {
    /// The leader removes the expired sessions of every node, so a new session does not.
    async fn create(
        &self,
        scope: SessionScope,
        record: SessionRecord,
        _expiry: Duration,
    ) -> Result<String, Error> {
        let token = new_token();
        let record = serde_json::to_vec(&record).map_err(|err| {
            error!("failed to encode the session: {err}");
            Error::FailedToSaveConfig
        })?;
        let key = self.session_key(scope, &token);
        self.put_unconditional(key, &record, None)
            .await
            .map_err(unavailable)?;
        Ok(token)
    }

    async fn get(&self, scope: SessionScope, token: &str) -> Result<Option<SessionRecord>, Error> {
        let key = self.session_key(scope, token);
        let Some(item) = self.store().get(&key).await.map_err(unavailable)? else {
            return Ok(None);
        };
        match self.decode_json::<SessionRecord>(&item) {
            Ok(record) => Ok(Some(record)),
            Err(err) => {
                error!(key, "invalid session: {err:#}");
                Ok(None)
            }
        }
    }

    async fn remove(&self, scope: SessionScope, token: &str) -> Result<(), Error> {
        let key = self.session_key(scope, token);
        self.delete_unconditional(vec![key])
            .await
            .map_err(unavailable)
    }

    /// Also removes the sessions that do not open.
    async fn remove_expired(&self, expiry: Duration) -> Result<(), Error> {
        let prefix = self.layout().sessions();
        self.delete_stale(&prefix, |item| {
            !self
                .decode_json::<SessionRecord>(item)
                .is_ok_and(|record| record.is_active(expiry))
        })
        .await
        .map_err(unavailable)
    }
}

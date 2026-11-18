//! MongoDB as a single-member replica set. Only a replica set writes the oplog that a
//! point-in-time recovery replays, and a replica set with authentication needs a key file
//! (MongoDB manual, "Convert a Standalone mongod to a Replica Set" and "Deploy Self-Managed
//! Replica Set With Keyfile Authentication").

use super::Platform;
use super::backup::mongodb_uri;
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::id::ShortId;
use std::time::Duration;

/// The name of the replica set of every managed MongoDB.
pub(super) const REPLICA_SET: &str = "rs0";

/// The time a new member takes to become a writable primary.
const PRIMARY_TIMEOUT: Duration = Duration::from_secs(60);

/// The start command of a MongoDB replica set member. The key file lives in the data volume,
/// so it survives a new deployment; mongod refuses a key file that others can read, and runs
/// as the `mongodb` user, which must own it. The entrypoint creates the root user first.
pub(super) const MONGO_REPLICA_START: &str = "k=/data/db/r3v3rs3.keyfile; \
if [ ! -s \"$k\" ]; then head -c 756 /dev/urandom | base64 | tr -d '\\n' > \"$k.tmp\" && mv \"$k.tmp\" \"$k\"; fi; \
chmod 400 \"$k\" && chown mongodb:mongodb \"$k\" && \
exec docker-entrypoint.sh mongod --replSet rs0 --keyFile \"$k\" --bind_ip_all";

/// Whether a database runs as a replica set: a local MongoDB.
pub(super) fn replica_set(engine: Option<&str>, target: ShortId) -> bool {
    engine == Some("mongodb") && super::is_local(target)
}

/// The mongosh script that initiates the replica set once, with the member named by the
/// alias of the database on the shared network, and prints whether the member is a writable
/// primary. A set that runs already is left as it is.
fn initiate_script(host: &str, port: u16) -> String {
    format!(
        "try {{ rs.status(); }} catch (e) {{ \
if (e.codeName !== 'NotYetInitialized') {{ throw e; }} \
rs.initiate({{_id: '{REPLICA_SET}', members: [{{_id: 0, host: '{host}:{port}'}}]}}); }} \
print(db.hello().isWritablePrimary === true ? 'primary' : 'waiting');"
    )
}

impl Platform {
    /// Initiates the replica set of a new MongoDB container and waits until it takes writes.
    pub(super) async fn ensure_replica_set(
        &self,
        db_id: ShortId,
        runtime: &dyn ContainerRuntime,
        container: &ContainerName,
        port: u16,
    ) -> anyhow::Result<()> {
        let info = r3v3rs3_api::platform::engine_info("mongodb")
            .context("the engine mongodb is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        let uri = mongodb_uri(&info.user, &password)?;
        let script = initiate_script(&super::deploy::app_host(db_id), port);
        let deadline = tokio::time::Instant::now() + PRIMARY_TIMEOUT;
        loop {
            let answer = run_script(runtime, container, &uri, &script).await;
            if answer.as_ref().is_ok_and(|out| out.trim() == "primary") {
                return Ok(());
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "the replica set did not elect a primary in {} seconds: {answer:?}",
                PRIMARY_TIMEOUT.as_secs()
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

async fn run_script(
    runtime: &dyn ContainerRuntime,
    container: &ContainerName,
    uri: &str,
    script: &str,
) -> anyhow::Result<String> {
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        "mongosh \"$MONGO_URI\" --quiet --eval \"$MONGO_SCRIPT\"".to_string(),
    ];
    let env = vec![format!("MONGO_URI={uri}"), format!("MONGO_SCRIPT={script}")];
    let out = runtime.exec_capture(container, command, env).await?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

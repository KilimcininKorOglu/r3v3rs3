//! Container registry credentials for private image pulls.

use crate::clock::unix_ms;
use crate::platform::Platform;
use crate::platform::image_check::parse_image_target;
use crate::platform::store::PlatformStore;
use anyhow::{Context as _, bail};
use r3v3rs3_api::container::ImageRef;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{RegistryAuth, RegistryEntry, RegistryRequest};

pub(crate) fn registry_aad(id: ShortId) -> String {
    format!("registry/{id}/secret")
}

fn validate_registry_fields(server: &str, username: &str) -> anyhow::Result<()> {
    if server.is_empty() {
        bail!(Error::InvalidRegistry {
            reason: "server cannot be empty".into(),
        });
    }
    if username.is_empty() {
        bail!(Error::InvalidRegistry {
            reason: "username cannot be empty".into(),
        });
    }
    Ok(())
}

async fn ensure_server_available(
    store: &PlatformStore,
    current_id: Option<ShortId>,
    server: &str,
) -> anyhow::Result<()> {
    if let Some(existing) = store.registry_by_server(server).await?
        && Some(existing.id) != current_id
    {
        bail!(Error::RegistryExists {
            server: server.to_string(),
        });
    }
    Ok(())
}

fn match_registry_server<'a>(
    registries: &'a [RegistryEntry],
    host: &str,
) -> Option<&'a RegistryEntry> {
    registries.iter().find(|r| {
        r.server.eq_ignore_ascii_case(host)
            || (host == "registry-1.docker.io" && r.server.eq_ignore_ascii_case("docker.io"))
            || (host == "docker.io" && r.server.eq_ignore_ascii_case("registry-1.docker.io"))
    })
}

impl Platform {
    /// Lists all stored container registries.
    pub async fn registries(&self) -> anyhow::Result<Vec<RegistryEntry>> {
        self.store.list_registries().await
    }

    /// Retrieves a single registry by ID.
    pub async fn registry(&self, id: ShortId) -> anyhow::Result<Option<RegistryEntry>> {
        self.store.registry(id).await
    }

    /// Adds a new container registry credential.
    pub async fn add_registry(&self, req: RegistryRequest) -> anyhow::Result<RegistryEntry> {
        let server = req.server.trim().to_string();
        let username = req.username.trim().to_string();
        validate_registry_fields(&server, &username)?;

        let password = req.password.as_deref().unwrap_or("").trim();
        if password.is_empty() {
            bail!(Error::InvalidRegistry {
                reason: "password or token is required".into(),
            });
        }

        ensure_server_available(&self.store, None, &server).await?;

        let id = ShortId::new();
        let now = unix_ms();
        let entry = RegistryEntry {
            id,
            server,
            username,
            created_at: now,
            updated_at: now,
        };

        let sealed = self.keys().seal(&registry_aad(id), password.as_bytes())?;

        self.store.insert_registry(&entry, &sealed).await?;

        Ok(entry)
    }

    /// Updates existing container registry credentials.
    pub async fn update_registry(
        &self,
        id: ShortId,
        req: RegistryRequest,
    ) -> anyhow::Result<RegistryEntry> {
        let existing = self
            .store
            .registry(id)
            .await?
            .ok_or_else(|| Error::IdNotFound { id: id.to_string() })?;

        let server = req.server.trim().to_string();
        let username = req.username.trim().to_string();
        validate_registry_fields(&server, &username)?;
        ensure_server_available(&self.store, Some(id), &server).await?;

        let sealed = match req.password.as_deref().map(str::trim) {
            Some(pw) if !pw.is_empty() => Some(self.keys().seal(&registry_aad(id), pw.as_bytes())?),
            _ => None,
        };

        let updated = RegistryEntry {
            id,
            server,
            username,
            created_at: existing.created_at,
            updated_at: unix_ms(),
        };

        self.store
            .update_registry(&updated, sealed.as_deref())
            .await?;

        Ok(updated)
    }

    /// Deletes a container registry credential.
    pub async fn delete_registry(&self, id: ShortId) -> anyhow::Result<()> {
        let deleted = self.store.delete_registry(id).await?;
        if !deleted {
            bail!(Error::IdNotFound { id: id.to_string() });
        }
        Ok(())
    }

    /// Resolves registry authentication credentials for an image reference.
    pub async fn registry_auth_for_image(
        &self,
        image: &str,
    ) -> anyhow::Result<Option<RegistryAuth>> {
        let Ok(image_ref) = image.parse::<ImageRef>() else {
            return Ok(None);
        };
        let (host, _, _) = parse_image_target(&image_ref);
        let registries = self.store.list_registries().await?;

        let Some(registry) = match_registry_server(&registries, &host) else {
            return Ok(None);
        };

        let sealed = self
            .store
            .registry_secret(registry.id)
            .await?
            .context("missing registry secret")?;

        let password_bytes = self
            .keys()
            .open(&registry_aad(registry.id), &sealed)
            .context("failed to unseal registry password")?;

        let password =
            String::from_utf8(password_bytes).context("registry password is not valid utf-8")?;

        Ok(Some(RegistryAuth {
            username: registry.username.clone(),
            password,
            serveraddress: registry.server.clone(),
        }))
    }
}

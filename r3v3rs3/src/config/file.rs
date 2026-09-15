use super::{account, build_info, storage::Storage};
use crate::cdn::CdnRanges;
use crate::certs::{
    acme::{AcmeAccount, AcmeEntry},
    Cert,
};
use anyhow::Context as _;
use indexmap::map::IndexMap;
use r3v3rs3_api::{
    app::AppConfig,
    auth::{Account, LoginRequest, LoginResponse, Role},
    cert::CertKind,
    id::ShortId,
};
use r3v3rs3_api::{
    error::Error,
    port::{Port, PortEntry},
    proxy::{Proxy, ProxyEntry},
};
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::fs;
use tokio::io::AsyncReadExt;
use toml_edit::DocumentMut;
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "T: serde::Serialize",
    deserialize = "T: serde::de::DeserializeOwned"
))]
struct Versioned<T> {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(flatten, default)]
    pub data: T,
}

fn default_version() -> String {
    build_info::PKG_VERSION.to_owned()
}

/// Logs a failed write with its path. The caller gets an error without the path.
fn saved(path: &Path, result: anyhow::Result<()>) -> Result<(), Error> {
    result.map_err(|err| {
        error!(?path, "failed to save: {err}");
        Error::FailedToSaveConfig
    })
}

/// Writes a file that holds secrets, such as ACME account keys, so only the owner can read it.
async fn write_private(path: &Path, contents: String) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The creation mode does not apply to a file that an earlier version created.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .await?;
    }
    file.write_all(contents.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(all(test, unix))]
mod private_file_test {
    use super::write_private;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn an_existing_readable_file_becomes_owner_only() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!("r3v3rs3-acme-{}.toml", std::process::id()));
        std::fs::write(&path, "")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;

        write_private(&path, "version = \"1\"".to_string()).await?;

        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        let contents = std::fs::read_to_string(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(mode, 0o600);
        assert_eq!(contents, "version = \"1\"");
        Ok(())
    }

    #[tokio::test]
    async fn a_new_file_is_created_owner_only() -> anyhow::Result<()> {
        let path =
            std::env::temp_dir().join(format!("r3v3rs3-acme-new-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);

        write_private(&path, String::new()).await?;

        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        std::fs::remove_file(&path)?;
        assert_eq!(mode, 0o600);
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod accounts_file_test {
    use super::FileStorage;
    use r3v3rs3_api::auth::{Account, Role};
    use std::collections::{BTreeSet, HashMap};
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn accounts_keep_their_roles_in_an_owner_only_file() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "r3v3rs3-accounts-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        let files = FileStorage::new(&dir);
        let editor = Account {
            password: "$argon2id$hash".into(),
            role: Role::Editor,
            proxies: Some(BTreeSet::from(["web".parse()?])),
            ..Default::default()
        };
        let accounts = HashMap::from([("editor".to_string(), editor)]);

        let saved = files.save_accounts_impl(&accounts).await;
        let mode = std::fs::metadata(dir.join("accounts.toml"))
            .map(|metadata| metadata.permissions().mode() & 0o777);
        let loaded = files.read_accounts().await;
        std::fs::remove_dir_all(&dir)?;

        saved?;
        assert_eq!(mode?, 0o600);
        let loaded = loaded?;
        assert_eq!(loaded["editor"].role, Role::Editor);
        assert_eq!(loaded["editor"].proxies, accounts["editor"].proxies);
        Ok(())
    }
}

pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
        }
    }

    /// Reads `config.toml`. Unlike [`Storage::load_app_config`], a missing or invalid file is an
    /// error.
    pub async fn read_app_config(&self) -> anyhow::Result<AppConfig> {
        self.load_app_config_impl(&self.dir.join("config.toml"))
            .await
    }

    /// Removes the directory of the certificate below each kind directory.
    async fn delete_cert_impl(&self, certs: &Path, id: ShortId) -> anyhow::Result<()> {
        if !fs::try_exists(certs).await? {
            return Ok(());
        }
        let pattern = format!("*/{id}");
        let walker = globwalk::GlobWalkerBuilder::from_patterns(certs, &[&pattern]).build()?;
        for entry in walker {
            fs::remove_dir_all(entry?.path()).await?;
        }
        Ok(())
    }

    async fn save_cdn_ranges_impl(&self, path: &Path, ranges: &CdnRanges) -> anyhow::Result<()> {
        fs::create_dir_all(&self.dir).await?;
        info!(?path, "save CDN IP ranges");
        fs::write(path, serde_json::to_vec(ranges)?).await?;
        Ok(())
    }

    async fn load_cdn_ranges_impl(&self, path: &Path) -> anyhow::Result<CdnRanges> {
        info!(?path, "load CDN IP ranges");
        Ok(serde_json::from_slice(&fs::read(path).await?)?)
    }

    async fn save_app_config_impl(&self, path: &Path, config: &AppConfig) -> anyhow::Result<()> {
        fs::create_dir_all(path.parent().unwrap()).await?;
        info!(?path, "save config");
        let mut doc = toml_edit::ser::to_document(&config)?;
        doc["version"] = toml_edit::value(build_info::PKG_VERSION);
        fs::write(path, doc.to_string()).await?;
        Ok(())
    }

    async fn load_app_config_impl(&self, path: &Path) -> anyhow::Result<AppConfig> {
        info!(?path, "load config");
        let content = fs::read_to_string(path).await?;
        Ok(toml::from_str(&content)?)
    }

    async fn save_ports_impl(&self, path: &Path, ports: &[PortEntry]) -> anyhow::Result<()> {
        fs::create_dir_all(path.parent().unwrap()).await?;
        info!(?path, "save config");
        let mut doc = match self.load_document(path).await {
            Ok(doc) => doc,
            Err(err) => {
                warn!(?path, %err, "failed to load config");
                DocumentMut::new()
            }
        };

        let mut unused = doc
            .as_table()
            .iter()
            .map(|(key, _)| key.to_string())
            .collect::<HashSet<_>>();
        for port in ports {
            let (id, entry): (ShortId, Port) = port.clone().into();
            let id = id.to_string();
            doc[&id].clone_from(toml_edit::ser::to_document(&entry)?.as_item());
            unused.remove(&id);
        }
        for key in unused {
            doc.remove(&key);
        }

        doc["version"] = toml_edit::value(build_info::PKG_VERSION);
        fs::write(path, doc.to_string()).await?;
        Ok(())
    }

    async fn load_document(&self, path: &Path) -> anyhow::Result<DocumentMut> {
        info!(?path, "load config");
        let content = fs::read_to_string(path).await?;
        Ok(content.parse::<DocumentMut>()?)
    }

    async fn load_ports_impl(&self, path: &Path) -> anyhow::Result<Vec<PortEntry>> {
        info!(?path, "load config");
        let content = fs::read_to_string(path).await?;
        let table: Versioned<IndexMap<ShortId, Port>> = toml::from_str(&content)?;
        Ok(table.data.into_iter().map(|entry| entry.into()).collect())
    }

    async fn load_proxies_impl(&self, path: &Path) -> anyhow::Result<Vec<ProxyEntry>> {
        info!(?path, "load proxies");
        let content = fs::read_to_string(path).await?;
        let table: Versioned<IndexMap<ShortId, Proxy>> = toml::from_str(&content)?;
        Ok(table.data.into_iter().map(|entry| entry.into()).collect())
    }

    async fn save_proxies_impl(&self, path: &Path, proxies: &[ProxyEntry]) -> anyhow::Result<()> {
        fs::create_dir_all(path.parent().unwrap()).await?;
        info!(?path, "save config");
        let mut doc = match self.load_document(path).await {
            Ok(doc) => doc,
            Err(err) => {
                warn!(?path, %err, "failed to load config");
                DocumentMut::new()
            }
        };

        let mut unused = doc
            .as_table()
            .iter()
            .map(|(key, _)| key.to_string())
            .collect::<HashSet<_>>();
        for site in proxies {
            let (id, entry): (ShortId, Proxy) = site.clone().into();
            let id = id.to_string();
            doc[&id].clone_from(toml_edit::ser::to_document(&entry)?.as_item());
            unused.remove(&id);
        }
        for key in unused {
            doc.remove(&key);
        }

        doc["version"] = toml_edit::value(build_info::PKG_VERSION);
        fs::write(path, doc.to_string()).await?;
        Ok(())
    }

    async fn save_cert_impl(&self, path: &Path, cert: &Cert) -> anyhow::Result<()> {
        fs::create_dir_all(path).await?;
        info!(?path, "save cert");
        fs::write(path.join("cert.pem"), &cert.pem_chain).await?;
        if let Some(key) = &cert.pem_key {
            fs::write(path.join("key.pem"), key).await?;
        }
        Ok(())
    }

    async fn save_acme_impl(&self, path: &Path, acme: &AcmeEntry) -> anyhow::Result<()> {
        fs::create_dir_all(path.parent().unwrap()).await?;
        info!(?path, "save config");
        let mut doc = match self.load_document(path).await {
            Ok(doc) => doc,
            Err(err) => {
                warn!(?path, %err, "failed to load config");
                DocumentMut::new()
            }
        };

        let (id, entry): (ShortId, AcmeAccount) = acme.clone().into();
        let id = id.to_string();
        doc[&id].clone_from(toml_edit::ser::to_document(&entry)?.as_item());

        doc["version"] = toml_edit::value(build_info::PKG_VERSION);
        write_private(path, doc.to_string()).await?;
        Ok(())
    }

    async fn delete_acme_impl(&self, path: &Path, id: ShortId) -> anyhow::Result<()> {
        info!(?path, "delete acme");
        let mut doc = match self.load_document(path).await {
            Ok(doc) => doc,
            Err(err) => {
                warn!(?path, %err, "failed to load config");
                DocumentMut::new()
            }
        };

        doc.remove(&id.to_string());
        doc["version"] = toml_edit::value(build_info::PKG_VERSION);
        write_private(path, doc.to_string()).await?;
        Ok(())
    }

    pub async fn load_certs_impl(
        &self,
        path: &Path,
        kind: CertKind,
    ) -> anyhow::Result<Vec<Arc<Cert>>> {
        let walker = globwalk::GlobWalkerBuilder::from_patterns(
            path.join(kind.to_string()),
            &["*/cert.pem"],
        )
        .build()?
        .filter_map(Result::ok);

        let mut certs = Vec::new();
        for pem in walker {
            let chain = pem.path();
            let key = pem.path().parent().unwrap().join("key.pem");
            let mut chain_data = Vec::new();
            let mut key_data = Vec::new();

            match fs::File::open(&chain).await {
                Ok(mut file) => {
                    if let Err(err) = file.read_to_end(&mut chain_data).await {
                        error!(path = ?chain, "failed to load: {err}");
                    }
                }
                Err(err) => {
                    error!(path = ?chain, "failed to load: {err}");
                }
            }

            match fs::File::open(&key).await {
                Ok(mut file) => {
                    if let Err(err) = file.read_to_end(&mut key_data).await {
                        error!(path = ?key, "failed to load: {err}");
                    }
                }
                Err(err) => {
                    error!(path = ?key, "failed to load: {err}");
                }
            }

            let key_data = if key_data.is_empty() {
                None
            } else {
                Some(key_data)
            };

            match Cert::new(kind, chain_data, key_data) {
                Ok(cert) => certs.push(Arc::new(cert)),
                Err(err) => error!(?path, "failed to load: {err}"),
            }
        }
        Ok(certs)
    }

    pub async fn load_acmes_impl(&self, path: &Path) -> anyhow::Result<Vec<AcmeEntry>> {
        info!(?path, "load acmes");
        let content = fs::read_to_string(path).await?;
        let table: Versioned<IndexMap<ShortId, AcmeAccount>> = toml::from_str(&content)?;
        Ok(table.data.into_iter().map(|entry| entry.into()).collect())
    }

    async fn add_account_impl(
        &self,
        name: &str,
        password: &str,
        totp: bool,
        role: Role,
    ) -> anyhow::Result<Account> {
        fs::create_dir_all(&self.dir).await?;
        let path = self.dir.join("accounts.toml");
        info!(?path, "save account");

        let mut doc = match fs::read_to_string(&path).await {
            Ok(content) => content.parse::<DocumentMut>().unwrap_or_default(),
            Err(_) => DocumentMut::default(),
        };

        let mut account = account::new_account(password, totp)?;
        account.role = role;
        doc[name].clone_from(toml_edit::ser::to_document(&account)?.as_item());

        doc["version"] = toml_edit::value(build_info::PKG_VERSION);
        write_private(&path, doc.to_string()).await?;
        Ok(account)
    }

    async fn read_accounts(&self) -> anyhow::Result<HashMap<String, Account>> {
        let path = self.dir.join("accounts.toml");
        info!(?path, "load accounts");
        let content = fs::read_to_string(&path).await?;
        let accounts: Versioned<HashMap<String, Account>> = toml::from_str(&content)?;
        Ok(accounts.data)
    }

    /// Writes every account. The file holds password hashes and TOTP secrets, so only the owner
    /// can read it.
    async fn save_accounts_impl(&self, accounts: &HashMap<String, Account>) -> anyhow::Result<()> {
        fs::create_dir_all(&self.dir).await?;
        let path = self.dir.join("accounts.toml");
        info!(?path, "save accounts");
        let table = Versioned {
            version: default_version(),
            data: accounts.iter().collect::<BTreeMap<_, _>>(),
        };
        write_private(&path, toml::to_string(&table)?).await
    }

    /// Reads every file. A missing file is empty, but a file that cannot be read is an error, and
    /// `config.toml` must exist.
    pub async fn read_state(&self) -> anyhow::Result<FileState> {
        let ports = self.dir.join("ports.toml");
        let proxies = self.dir.join("proxies.toml");
        let acmes = self.dir.join("acme.toml");
        let accounts = self.dir.join("accounts.toml");
        let cdn = self.dir.join("cdn-ranges.json");
        Ok(FileState {
            config: self.read_app_config().await?,
            ports: if_exists(&ports, self.load_ports_impl(&ports)).await?,
            proxies: if_exists(&proxies, self.load_proxies_impl(&proxies)).await?,
            certs: self.load_certs().await,
            acmes: if_exists(&acmes, self.load_acmes_impl(&acmes)).await?,
            accounts: if_exists(&accounts, self.read_accounts()).await?,
            cdn_ranges: if_exists(&cdn, async {
                self.load_cdn_ranges_impl(&cdn).await.map(Some)
            })
            .await?,
        })
    }
}

/// The state that the files of a node store.
#[derive(Default)]
pub struct FileState {
    pub config: AppConfig,
    pub ports: Vec<PortEntry>,
    pub proxies: Vec<ProxyEntry>,
    pub certs: Vec<Arc<Cert>>,
    pub acmes: Vec<AcmeEntry>,
    pub accounts: HashMap<String, Account>,
    pub cdn_ranges: Option<CdnRanges>,
}

/// Reads a file when it exists. A missing file gives the default value.
async fn if_exists<T: Default>(
    path: &Path,
    read: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    if !fs::try_exists(path).await? {
        return Ok(T::default());
    }
    read.await
        .with_context(|| format!("failed to read {}", path.display()))
}

#[async_trait::async_trait]
impl Storage for FileStorage {
    async fn save_app_config(&self, config: &AppConfig) -> Result<(), Error> {
        let path = self.dir.join("config.toml");
        let result = self.save_app_config_impl(&path, config).await;
        saved(&path, result)
    }

    async fn load_app_config(&self) -> AppConfig {
        let dir = &self.dir;
        let path = dir.join("config.toml");
        match self.load_app_config_impl(&path).await {
            Ok(config) => config,
            Err(err) => {
                warn!(?path, "failed to load: {err}");
                Default::default()
            }
        }
    }

    async fn save_ports(&self, entries: &[PortEntry]) -> Result<(), Error> {
        let path = self.dir.join("ports.toml");
        let result = self.save_ports_impl(&path, entries).await;
        saved(&path, result)
    }

    async fn load_ports(&self) -> Vec<PortEntry> {
        let dir = &self.dir;
        let path = dir.join("ports.toml");
        match self.load_ports_impl(&path).await {
            Ok(ports) => ports,
            Err(err) => {
                warn!(?path, "failed to load: {err}");
                Default::default()
            }
        }
    }

    async fn save_proxies(&self, proxies: &[ProxyEntry]) -> Result<(), Error> {
        let path = self.dir.join("proxies.toml");
        let result = self.save_proxies_impl(&path, proxies).await;
        saved(&path, result)
    }

    async fn load_proxies(&self) -> Vec<ProxyEntry> {
        let dir = &self.dir;
        let path = dir.join("proxies.toml");
        match self.load_proxies_impl(&path).await {
            Ok(proxies) => proxies,
            Err(err) => {
                warn!(?path, "failed to load: {err}");
                Default::default()
            }
        }
    }

    async fn save_cert(&self, cert: &Cert) -> Result<(), Error> {
        let path = self
            .dir
            .join("certs")
            .join(cert.kind.to_string())
            .join(cert.id().to_string());
        let result = self.save_cert_impl(&path, cert).await;
        saved(&path, result)
    }

    async fn save_acme(&self, acme: &AcmeEntry) -> Result<(), Error> {
        let path = self.dir.join("acme.toml");
        let result = self.save_acme_impl(&path, acme).await;
        saved(&path, result)
    }

    async fn delete_acme(&self, id: ShortId) -> Result<(), Error> {
        let path = self.dir.join("acme.toml");
        let result = self.delete_acme_impl(&path, id).await;
        saved(&path, result)
    }

    async fn delete_cert(&self, id: ShortId) -> Result<(), Error> {
        let path = self.dir.join("certs");
        let result = self.delete_cert_impl(&path, id).await;
        saved(&path, result)
    }

    async fn load_acmes(&self) -> Vec<AcmeEntry> {
        let dir = &self.dir;
        let path = dir.join("acme.toml");
        match self.load_acmes_impl(&path).await {
            Ok(acmes) => acmes,
            Err(err) => {
                warn!(?path, "failed to load: {err}");
                Default::default()
            }
        }
    }

    async fn load_certs(&self) -> Vec<Arc<Cert>> {
        let dir = &self.dir;
        let path = dir.join("certs");
        let mut certs = Vec::new();
        // Every kind has its own directory below `certs`.
        for kind in [CertKind::Server, CertKind::Client, CertKind::Root] {
            match self.load_certs_impl(&path, kind).await {
                Ok(mut entries) => certs.append(&mut entries),
                Err(err) => {
                    warn!(?path, %kind, "failed to load: {err}");
                }
            }
        }
        certs
    }

    async fn add_account(
        &self,
        name: &str,
        password: &str,
        totp: bool,
        role: Role,
    ) -> Result<Account, Error> {
        self.add_account_impl(name, password, totp, role)
            .await
            .map_err(|_| Error::FailedToCreateAccount)
    }

    async fn load_accounts(&self) -> Result<HashMap<String, Account>, Error> {
        let path = self.dir.join("accounts.toml");
        if_exists(&path, self.read_accounts()).await.map_err(|err| {
            error!(?path, "failed to load: {err:#}");
            Error::FailedToLoadAccounts
        })
    }

    async fn save_accounts(&self, accounts: &HashMap<String, Account>) -> Result<(), Error> {
        let path = self.dir.join("accounts.toml");
        let result = self.save_accounts_impl(accounts).await;
        saved(&path, result)
    }

    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error> {
        let accounts = self.read_accounts().await.map_err(|err| {
            error!(%err, "failed to load accounts: {err}");
            Error::InvalidLoginCredentials
        })?;
        let account = accounts.get(&request.username).cloned();
        account::verify(account.as_ref(), request)
    }

    async fn save_cdn_ranges(&self, ranges: &CdnRanges) -> Result<(), Error> {
        let path = self.dir.join("cdn-ranges.json");
        let result = self.save_cdn_ranges_impl(&path, ranges).await;
        saved(&path, result)
    }

    async fn load_cdn_ranges(&self) -> Option<CdnRanges> {
        let path = self.dir.join("cdn-ranges.json");
        match self.load_cdn_ranges_impl(&path).await {
            Ok(ranges) => Some(ranges),
            Err(err) => {
                warn!(?path, "failed to load: {err}");
                None
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn every_certificate_kind_is_loaded_after_save() {
        let dir = std::env::temp_dir().join(format!("r3v3rs3-file-certs-{}", std::process::id()));
        let storage = FileStorage::new(&dir);

        let ca = Cert::new_ca().unwrap();
        let san = ["client.example.com".parse().unwrap()];
        let server = Cert::new_self_signed(&san, &ca).unwrap();
        let client = Cert::new_client(&san, &ca).unwrap();
        for cert in [&ca, &server, &client] {
            storage.save_cert(cert).await.unwrap();
        }

        let loaded = storage.load_certs().await;
        std::fs::remove_dir_all(&dir).unwrap();

        for cert in [&ca, &server, &client] {
            let found = loaded
                .iter()
                .find(|loaded| loaded.id == cert.id)
                .unwrap_or_else(|| panic!("{:?} certificate is not loaded", cert.kind));
            assert_eq!(found.kind, cert.kind);
            assert!(found.key.is_some());
        }
    }
}

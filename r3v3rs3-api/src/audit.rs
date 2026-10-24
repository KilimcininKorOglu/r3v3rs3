//! The audit log: the changes that the accounts make through the admin API, and the sign-ins of
//! the admin panel.

use serde_derive::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::{IntoParams, ToSchema};

/// The longest summary of an entry in bytes.
pub const MAX_SUMMARY_LENGTH: usize = 512;

/// The entries of a query without `limit`.
pub const DEFAULT_QUERY_LIMIT: u32 = 100;

/// The most entries of a query.
pub const MAX_QUERY_LIMIT: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Login,
    LoginFailed,
    Logout,
    AddAccount,
    UpdateAccount,
    DeleteAccount,
    UpdateConfig,
    RefreshCdnRanges,
    /// An account refreshed the app store catalog.
    RefreshAppStore,
    /// An account refreshed the engine catalog.
    RefreshEngines,
    AddPort,
    UpdatePort,
    DeletePort,
    ResetPort,
    AddProxy,
    UpdateProxy,
    DeleteProxy,
    PurgeProxyCache,
    AddAccessList,
    UpdateAccessList,
    DeleteAccessList,
    AddCert,
    DeleteCert,
    AddAcme,
    UpdateAcme,
    DeleteAcme,
    /// An account ordered the certificates of an ACME entry again at once.
    RequestAcme,
    AddDnsCredential,
    UpdateDnsCredential,
    DeleteDnsCredential,
    AddApp,
    UpdateApp,
    DeleteApp,
    UpdateAppEnv,
    DeployApp,
    RollbackApp,
    /// An account opened a terminal into the container of an app.
    ExecApp,
    /// An account uploaded or deleted a file of an app.
    AppFile,
    /// An account created a database app.
    CreateDatabase,
    /// An account cloned a database app.
    CloneDatabase,
    /// An account created a read replica of a database app.
    CreateDatabaseReplica,
    /// An account deleted a database app.
    DeleteDatabase,
    /// An account took the backup of a database.
    BackupDatabase,
    /// An account restored the backup of a database.
    RestoreDatabase,
    /// An account restored a database from an S3 remote backup snapshot.
    RestoreDatabaseFromS3,
    /// An account performed point-in-time recovery on a database.
    PitrRestoreDatabase,
    /// An account archived a WAL or binary log segment of a database.
    ArchiveDatabaseWal,
    /// An account changed the backup settings.
    UpdateBackupConfig,
    SetAppGitToken,
    DeleteAppGitToken,
    AddTarget,
    DeleteTarget,
    NewTargetToken,
    EnrollAgent,
    UpgradeServer,
    NewAppWebhookSecret,
    DeleteAppWebhookSecret,
    /// A signed webhook request of a Git provider started or queued a deployment.
    AppWebhook,
    AddGitConnection,
    UpdateGitConnection,
    DeleteGitConnection,
    UpdatePlatformSettings,
    /// An account of the Git provider authorized a connection.
    ConnectGitConnection,
    /// An account installed the webhook of an app at its Git provider again.
    InstallAppHook,
    /// An account approved an MCP client in the OAuth authorization flow.
    AuthorizeMcpClient,
    /// An account checked for image updates of an app.
    CheckAppUpdate,
    /// An account added a scheduled command to an app.
    AddScheduledCommand,
    /// An account updated a scheduled command of an app.
    UpdateScheduledCommand,
    /// An account deleted a scheduled command of an app.
    DeleteScheduledCommand,
    /// An account executed a scheduled command of an app.
    RunScheduledCommand,
    /// An account approved a pending deployment of an app.
    ApproveDeployment,
    /// An account rejected a pending deployment of an app.
    RejectDeployment,
    /// An account removed a login ban for an IP address.
    UnbanIp,
    /// An account upgraded an app to the latest version in the app store catalog.
    UpgradeAppFromCatalog,
    /// An account rotated the platform encryption key and re-sealed stored secrets.
    RotatePlatformKey,
    /// An account registered a WebAuthn passkey.
    RegisterPasskey,
    /// An account deleted a WebAuthn passkey.
    DeletePasskey,
    /// An account created an API token.
    CreateApiToken,
    /// An account deleted an API token.
    DeleteApiToken,
    /// An account added container registry credentials.
    AddRegistry,
    /// An account updated container registry credentials.
    UpdateRegistry,
    /// An account deleted container registry credentials.
    DeleteRegistry,
    /// An account pushed an upgrade to an agent target.
    UpgradeTarget,
    /// An account restored a configuration backup.
    RestoreConfig,
    /// An account created a disaster recovery backup.
    CreateDisasterRecoveryBackup,
    /// An account restored platform state from a disaster recovery backup.
    RestoreDisasterRecoveryBackup,
    /// An account added an environment variable group.
    AddEnvGroup,
    /// An account updated an environment variable group.
    UpdateEnvGroup,
    /// An account deleted an environment variable group.
    DeleteEnvGroup,
    /// An account added a project.
    AddProject,
    /// An account updated a project.
    UpdateProject,
    /// An account deleted a project.
    DeleteProject,
    /// An account added an environment to a project.
    AddEnvironment,
    /// An account updated an environment in a project.
    UpdateEnvironment,
    /// An account deleted an environment from a project.
    DeleteEnvironment,
    /// An account updated environment variables of a project environment.
    UpdateEnvironmentEnv,
    /// An account synchronized or cloned environment variables across project environments.
    SyncEnvironmentEnv,
    /// An account added an outbound webhook.
    AddOutboundWebhook,
    /// An account updated an outbound webhook.
    UpdateOutboundWebhook,
    /// An account deleted an outbound webhook.
    DeleteOutboundWebhook,
    /// An account tested an outbound webhook.
    TestOutboundWebhook,
    /// An account drained a target for maintenance.
    DrainTarget,
    /// An account undrained a target, restoring it to traffic.
    UndrainTarget,
    /// An account updated the node-level threshold alert rules.
    UpdateNodeAlertRules,
    /// An account truncated or vacuumed container log files.
    VacuumLogs,
    /// An account or system quarantined an IP address.
    QuarantineIp,
    /// An account unquarantined / unbanned an IP address.
    UnquarantineIp,
    /// An account updated the auto-quarantine configuration.
    UpdateQuarantineConfig,
    /// An account updated the GeoIP database configuration.
    UpdateGeoIpConfig,
    /// An account or scheduled task triggered a GeoIP database update.
    RefreshGeoIpDatabase,
    /// An account added an error page theme.
    AddErrorPageTheme,
    /// An account updated an error page theme.
    UpdateErrorPageTheme,
    /// An account deleted an error page theme.
    DeleteErrorPageTheme,
    /// An account applied a predefined security header profile to a proxy.
    ApplySecurityProfile,
    /// An operator terminated an active long-lived connection or stream.
    TerminateConnection,
}

impl AuditAction {
    pub const ALL: [AuditAction; 111] = [
        Self::Login,
        Self::LoginFailed,
        Self::Logout,
        Self::AddAccount,
        Self::UpdateAccount,
        Self::DeleteAccount,
        Self::UpdateConfig,
        Self::RefreshCdnRanges,
        Self::RefreshAppStore,
        Self::RefreshEngines,
        Self::AddPort,
        Self::UpdatePort,
        Self::DeletePort,
        Self::ResetPort,
        Self::AddProxy,
        Self::UpdateProxy,
        Self::DeleteProxy,
        Self::PurgeProxyCache,
        Self::AddAccessList,
        Self::UpdateAccessList,
        Self::DeleteAccessList,
        Self::AddCert,
        Self::DeleteCert,
        Self::AddAcme,
        Self::UpdateAcme,
        Self::DeleteAcme,
        Self::AddApp,
        Self::UpdateApp,
        Self::DeleteApp,
        Self::UpdateAppEnv,
        Self::DeployApp,
        Self::RollbackApp,
        Self::ExecApp,
        Self::AppFile,
        Self::CreateDatabase,
        Self::CloneDatabase,
        Self::CreateDatabaseReplica,
        Self::DeleteDatabase,
        Self::BackupDatabase,
        Self::RestoreDatabase,
        Self::RestoreDatabaseFromS3,
        Self::PitrRestoreDatabase,
        Self::ArchiveDatabaseWal,
        Self::UpdateBackupConfig,
        Self::SetAppGitToken,
        Self::DeleteAppGitToken,
        Self::AddTarget,
        Self::DeleteTarget,
        Self::NewTargetToken,
        Self::EnrollAgent,
        Self::UpgradeServer,
        Self::NewAppWebhookSecret,
        Self::DeleteAppWebhookSecret,
        Self::AppWebhook,
        Self::AddGitConnection,
        Self::UpdateGitConnection,
        Self::DeleteGitConnection,
        Self::UpdatePlatformSettings,
        Self::ConnectGitConnection,
        Self::InstallAppHook,
        Self::AuthorizeMcpClient,
        Self::CheckAppUpdate,
        Self::AddScheduledCommand,
        Self::UpdateScheduledCommand,
        Self::DeleteScheduledCommand,
        Self::RunScheduledCommand,
        Self::ApproveDeployment,
        Self::RejectDeployment,
        Self::UnbanIp,
        Self::UpgradeAppFromCatalog,
        Self::RotatePlatformKey,
        Self::RegisterPasskey,
        Self::DeletePasskey,
        Self::CreateApiToken,
        Self::DeleteApiToken,
        Self::AddRegistry,
        Self::UpdateRegistry,
        Self::DeleteRegistry,
        Self::UpgradeTarget,
        Self::RestoreConfig,
        Self::CreateDisasterRecoveryBackup,
        Self::RestoreDisasterRecoveryBackup,
        Self::AddEnvGroup,
        Self::UpdateEnvGroup,
        Self::DeleteEnvGroup,
        Self::AddProject,
        Self::UpdateProject,
        Self::DeleteProject,
        Self::AddEnvironment,
        Self::UpdateEnvironment,
        Self::DeleteEnvironment,
        Self::UpdateEnvironmentEnv,
        Self::SyncEnvironmentEnv,
        Self::AddOutboundWebhook,
        Self::UpdateOutboundWebhook,
        Self::DeleteOutboundWebhook,
        Self::TestOutboundWebhook,
        Self::DrainTarget,
        Self::UndrainTarget,
        Self::UpdateNodeAlertRules,
        Self::VacuumLogs,
        Self::QuarantineIp,
        Self::UnquarantineIp,
        Self::UpdateQuarantineConfig,
        Self::UpdateGeoIpConfig,
        Self::RefreshGeoIpDatabase,
        Self::AddErrorPageTheme,
        Self::UpdateErrorPageTheme,
        Self::DeleteErrorPageTheme,
        Self::ApplySecurityProfile,
        Self::TerminateConnection,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AuditEntry {
    /// The Unix time in milliseconds.
    pub time: u64,
    /// The account. A failed sign-in has the typed username.
    pub username: String,
    /// The IP address of the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "192.0.2.10")]
    pub client: Option<IpAddr>,
    pub action: AuditAction,
    /// The id of the changed port, proxy, certificate, ACME entry or app, or the changed username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// The names, the addresses and the roles of the change. It holds no password, token or key.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The cluster node that recorded the entry. Empty without a cluster.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
}

/// The filter of an audit log query. Each field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    /// The earliest time in Unix milliseconds. The default is 31 days before `until`.
    pub since: Option<u64>,
    /// The latest time in Unix milliseconds. The default is the current time.
    pub until: Option<u64>,
    /// The account of the entries.
    pub username: Option<String>,
    /// The id of the changed resource, or the changed username.
    pub resource_id: Option<String>,
    /// The action of the entries.
    pub action: Option<AuditAction>,
    /// Free-text search matching username, resource_id, or summary.
    pub search: Option<String>,
    /// The most entries in the response: 100 by default, at most 500.
    pub limit: Option<u32>,
}

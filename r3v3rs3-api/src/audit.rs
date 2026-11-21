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
    CreateAccount,
    UpdateAccount,
    DeleteAccount,
    UpdateConfig,
    RefreshCdnRanges,
    /// An account refreshed the app store catalog.
    RefreshStore,
    /// An account refreshed the engine catalog.
    RefreshDatabaseEngines,
    CreatePort,
    UpdatePort,
    DeletePort,
    ResetPort,
    CreateProxy,
    UpdateProxy,
    DeleteProxy,
    PurgeProxyCache,
    CreateAccessList,
    UpdateAccessList,
    DeleteAccessList,
    UploadCert,
    DeleteCert,
    CreateAcme,
    UpdateAcme,
    DeleteAcme,
    /// An account ordered the certificates of an ACME entry again at once.
    RequestAcme,
    CreateDnsCredential,
    UpdateDnsCredential,
    DeleteDnsCredential,
    CreateApp,
    UpdateApp,
    DeleteApp,
    UpdateAppEnv,
    DeployApp,
    RollbackDeployment,
    /// An account opened a terminal into the container of an app.
    OpenAppTerminal,
    /// An account uploaded or deleted a file of an app.
    DeleteAppFile,
    /// An account created a database app.
    CreateDatabase,
    /// An account cloned a database app.
    CloneDatabase,
    /// An account created a read replica of a database app.
    CreateDatabaseReplica,
    /// An account deleted a database app.
    DeleteDatabase,
    /// An account read the passwords of the accounts of a database.
    GetDatabaseCredentials,
    /// An account gave the root account or a tenant of a database a new password.
    ResetDatabasePassword,
    /// An account removed a tenant of a shared database server with its data.
    DeleteDatabaseTenant,
    /// An account took the backup of a database.
    CreateDatabaseBackup,
    /// An account deleted a backup of a database.
    DeleteDatabaseBackup,
    /// An account restored the backup of a database.
    RestoreDatabaseBackup,
    /// An account restored a database from an S3 remote backup snapshot.
    RestoreS3DatabaseBackup,
    /// An account performed point-in-time recovery on a database.
    RestoreDatabasePitr,
    /// An account archived a WAL or binary log segment of a database.
    ArchiveDatabaseWal,
    /// An account changed the backup settings.
    UpdateBackupConfig,
    UpdateAppGitToken,
    DeleteAppGitToken,
    CreateTarget,
    DeleteTarget,
    CreateTargetToken,
    EnrollAgent,
    UpgradeServer,
    CreateAppWebhookSecret,
    DeleteAppWebhookSecret,
    /// A signed webhook request of a Git provider started or queued a deployment.
    ReceiveAppWebhook,
    CreateGitConnection,
    UpdateGitConnection,
    DeleteGitConnection,
    UpdatePrunePolicy,
    /// An account of the Git provider authorized a connection.
    InstallGithubApp,
    /// An account installed the webhook of an app at its Git provider again.
    InstallAppHook,
    /// An account approved an MCP client in the OAuth authorization flow.
    ApproveMcpClient,
    /// An account checked for image updates of an app.
    CheckAppUpdate,
    /// An account added a scheduled command to an app.
    CreateAppScheduledCommand,
    /// An account updated a scheduled command of an app.
    UpdateAppScheduledCommand,
    /// An account deleted a scheduled command of an app.
    DeleteAppScheduledCommand,
    /// An account executed a scheduled command of an app.
    RunAppScheduledCommand,
    /// An account approved a pending deployment of an app.
    ApproveDeployment,
    /// An account rejected a pending deployment of an app.
    RejectDeployment,
    /// An account cancelled an active or pending deployment of an app.
    CancelDeployment,
    /// An account removed a login ban for an IP address.
    UnbanIp,
    /// An account upgraded an app to the latest version in the app store catalog.
    UpgradeAppCatalog,
    /// An account rotated the platform encryption key and re-sealed stored secrets.
    RotatePlatformKey,
    /// An account registered a WebAuthn passkey.
    RegisterPasskey,
    /// An account deleted a WebAuthn passkey.
    DeletePasskey,
    /// An account created an API token.
    CreateToken,
    /// An account deleted an API token.
    DeleteToken,
    /// An account added container registry credentials.
    CreateRegistry,
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
    CreateEnvGroup,
    /// An account updated an environment variable group.
    UpdateEnvGroup,
    /// An account deleted an environment variable group.
    DeleteEnvGroup,
    /// An account added a project.
    CreateProject,
    /// An account updated a project.
    UpdateProject,
    /// An account deleted a project.
    DeleteProject,
    /// An account added an environment to a project.
    CreateEnvironment,
    /// An account updated an environment in a project.
    UpdateEnvironment,
    /// An account deleted an environment from a project.
    DeleteEnvironment,
    /// An account updated environment variables of a project environment.
    UpdateEnvironmentEnv,
    /// An account synchronized or cloned environment variables across project environments.
    SyncEnvironmentEnv,
    /// An account added an outbound webhook.
    CreateOutboundWebhook,
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
    /// An account set the geographical region of a target.
    UpdateTargetRegion,
    /// An account updated the node-level threshold alert rules.
    UpdateNodeAlerts,
    /// An account truncated or vacuumed container log files.
    VacuumAppLogs,
    /// An account or system quarantined an IP address.
    QuarantineIp,
    /// An account unquarantined / unbanned an IP address.
    UnquarantineIp,
    /// An account updated the auto-quarantine configuration.
    UpdateQuarantineConfig,
    /// An account updated the GeoIP database configuration.
    UpdateGeoipConfig,
    /// An account or scheduled task triggered a GeoIP database update.
    UpdateGeoipDatabase,
    /// An account added an error page theme.
    CreateErrorPageTheme,
    /// An account updated an error page theme.
    UpdateErrorPageTheme,
    /// An account deleted an error page theme.
    DeleteErrorPageTheme,
    /// An account applied a predefined security header profile to a proxy.
    ApplyProxySecurityHeaderProfile,
    /// An operator terminated an active long-lived connection or stream.
    TerminateTrafficConnection,
    /// An account generated secrets for an app's environment variables.
    GenerateAppEnvSecrets,
    /// An account updated the Certificate Transparency monitor configuration.
    UpdateCtConfig,
    /// An account triggered an on-demand Certificate Transparency log check.
    CheckCtNow,
    /// An account dismissed or updated a Certificate Transparency rogue certificate alert.
    DismissCtAlert,
    UploadAppFile,
    CreateVolumeBackup,
    RestoreVolumeBackup,
    DeleteVolumeBackup,
    ImportVolumeArchive,
    TestDatabaseBackup,
    RunDatabaseBackupNow,
    UpgradeAllTargets,
    RunPrune,
    /// The platform skipped an automatic job, such as a webhook deployment or a scheduled
    /// backup, of a stopped app.
    SkipStoppedApp,
    VacuumPlatformLogs,
    UpdateAnomalyRules,
    RestoreDisasterRecoveryUpload,
    CompleteGitOauth,
    DeleteManyCerts,
    ResetErrorPageThemes,
    UpdateProxyStreamPriority,
}

impl AuditAction {
    pub const ALL: [AuditAction; 142] = [
        Self::Login,
        Self::LoginFailed,
        Self::Logout,
        Self::CreateAccount,
        Self::UpdateAccount,
        Self::DeleteAccount,
        Self::UpdateConfig,
        Self::RefreshCdnRanges,
        Self::RefreshStore,
        Self::RefreshDatabaseEngines,
        Self::CreatePort,
        Self::UpdatePort,
        Self::DeletePort,
        Self::ResetPort,
        Self::CreateProxy,
        Self::UpdateProxy,
        Self::DeleteProxy,
        Self::PurgeProxyCache,
        Self::CreateAccessList,
        Self::UpdateAccessList,
        Self::DeleteAccessList,
        Self::UploadCert,
        Self::DeleteCert,
        Self::CreateAcme,
        Self::UpdateAcme,
        Self::DeleteAcme,
        Self::CreateApp,
        Self::UpdateApp,
        Self::DeleteApp,
        Self::UpdateAppEnv,
        Self::DeployApp,
        Self::RollbackDeployment,
        Self::OpenAppTerminal,
        Self::DeleteAppFile,
        Self::CreateDatabase,
        Self::CloneDatabase,
        Self::CreateDatabaseReplica,
        Self::DeleteDatabase,
        Self::GetDatabaseCredentials,
        Self::ResetDatabasePassword,
        Self::DeleteDatabaseTenant,
        Self::CreateDatabaseBackup,
        Self::DeleteDatabaseBackup,
        Self::RestoreDatabaseBackup,
        Self::RestoreS3DatabaseBackup,
        Self::RestoreDatabasePitr,
        Self::ArchiveDatabaseWal,
        Self::UpdateBackupConfig,
        Self::UpdateAppGitToken,
        Self::DeleteAppGitToken,
        Self::CreateTarget,
        Self::DeleteTarget,
        Self::CreateTargetToken,
        Self::EnrollAgent,
        Self::UpgradeServer,
        Self::CreateAppWebhookSecret,
        Self::DeleteAppWebhookSecret,
        Self::ReceiveAppWebhook,
        Self::CreateGitConnection,
        Self::UpdateGitConnection,
        Self::DeleteGitConnection,
        Self::UpdatePrunePolicy,
        Self::InstallGithubApp,
        Self::InstallAppHook,
        Self::ApproveMcpClient,
        Self::CheckAppUpdate,
        Self::CreateAppScheduledCommand,
        Self::UpdateAppScheduledCommand,
        Self::DeleteAppScheduledCommand,
        Self::RunAppScheduledCommand,
        Self::ApproveDeployment,
        Self::RejectDeployment,
        Self::CancelDeployment,
        Self::UnbanIp,
        Self::UpgradeAppCatalog,
        Self::RotatePlatformKey,
        Self::RegisterPasskey,
        Self::DeletePasskey,
        Self::CreateToken,
        Self::DeleteToken,
        Self::CreateRegistry,
        Self::UpdateRegistry,
        Self::DeleteRegistry,
        Self::UpgradeTarget,
        Self::RestoreConfig,
        Self::CreateDisasterRecoveryBackup,
        Self::RestoreDisasterRecoveryBackup,
        Self::CreateEnvGroup,
        Self::UpdateEnvGroup,
        Self::DeleteEnvGroup,
        Self::CreateProject,
        Self::UpdateProject,
        Self::DeleteProject,
        Self::CreateEnvironment,
        Self::UpdateEnvironment,
        Self::DeleteEnvironment,
        Self::UpdateEnvironmentEnv,
        Self::SyncEnvironmentEnv,
        Self::CreateOutboundWebhook,
        Self::UpdateOutboundWebhook,
        Self::DeleteOutboundWebhook,
        Self::TestOutboundWebhook,
        Self::DrainTarget,
        Self::UndrainTarget,
        Self::UpdateTargetRegion,
        Self::UpdateNodeAlerts,
        Self::VacuumAppLogs,
        Self::QuarantineIp,
        Self::UnquarantineIp,
        Self::UpdateQuarantineConfig,
        Self::UpdateGeoipConfig,
        Self::UpdateGeoipDatabase,
        Self::CreateErrorPageTheme,
        Self::UpdateErrorPageTheme,
        Self::DeleteErrorPageTheme,
        Self::ApplyProxySecurityHeaderProfile,
        Self::TerminateTrafficConnection,
        Self::GenerateAppEnvSecrets,
        Self::UpdateCtConfig,
        Self::CheckCtNow,
        Self::DismissCtAlert,
        Self::UploadAppFile,
        Self::CreateVolumeBackup,
        Self::RestoreVolumeBackup,
        Self::DeleteVolumeBackup,
        Self::ImportVolumeArchive,
        Self::TestDatabaseBackup,
        Self::RunDatabaseBackupNow,
        Self::UpgradeAllTargets,
        Self::RunPrune,
        Self::SkipStoppedApp,
        Self::VacuumPlatformLogs,
        Self::UpdateAnomalyRules,
        Self::RestoreDisasterRecoveryUpload,
        Self::CompleteGitOauth,
        Self::DeleteManyCerts,
        Self::ResetErrorPageThemes,
        Self::UpdateProxyStreamPriority,
        Self::CreateDnsCredential,
        Self::UpdateDnsCredential,
        Self::DeleteDnsCredential,
        Self::RequestAcme,
    ];
}

/// The action names that older releases wrote into the audit log, each with the name that
/// replaced it. An action is named after the API operation that records it; where an old name
/// covered several operations, its old entries move to the first one. The log database rewrites
/// these names when it opens.
pub const RENAMED_ACTIONS: &[(&str, &str)] = &[
    ("add_account", "create_account"),
    ("add_port", "create_port"),
    ("add_proxy", "create_proxy"),
    ("add_access_list", "create_access_list"),
    ("add_cert", "upload_cert"),
    ("add_acme", "create_acme"),
    ("add_dns_credential", "create_dns_credential"),
    ("add_app", "create_app"),
    ("rollback_app", "rollback_deployment"),
    ("exec_app", "open_app_terminal"),
    ("app_file", "delete_app_file"),
    ("view_database_credentials", "get_database_credentials"),
    ("backup_database", "create_database_backup"),
    ("restore_database", "restore_database_backup"),
    ("restore_database_from_s3", "restore_s3_database_backup"),
    ("pitr_restore_database", "restore_database_pitr"),
    ("set_app_git_token", "update_app_git_token"),
    ("add_target", "create_target"),
    ("new_target_token", "create_target_token"),
    ("new_app_webhook_secret", "create_app_webhook_secret"),
    ("app_webhook", "receive_app_webhook"),
    ("add_git_connection", "create_git_connection"),
    ("update_platform_settings", "update_prune_policy"),
    ("connect_git_connection", "install_github_app"),
    ("authorize_mcp_client", "approve_mcp_client"),
    ("add_scheduled_command", "create_app_scheduled_command"),
    ("update_scheduled_command", "update_app_scheduled_command"),
    ("delete_scheduled_command", "delete_app_scheduled_command"),
    ("run_scheduled_command", "run_app_scheduled_command"),
    ("upgrade_app_from_catalog", "upgrade_app_catalog"),
    ("create_api_token", "create_token"),
    ("delete_api_token", "delete_token"),
    ("add_registry", "create_registry"),
    ("add_env_group", "create_env_group"),
    ("add_project", "create_project"),
    ("add_environment", "create_environment"),
    ("add_outbound_webhook", "create_outbound_webhook"),
    ("set_target_region", "update_target_region"),
    ("update_node_alert_rules", "update_node_alerts"),
    ("vacuum_logs", "vacuum_app_logs"),
    ("refresh_app_store", "refresh_store"),
    ("refresh_engines", "refresh_database_engines"),
    ("check_ct_logs", "check_ct_now"),
    ("update_ct_monitor_config", "update_ct_config"),
    ("terminate_connection", "terminate_traffic_connection"),
    ("refresh_geo_ip_database", "update_geoip_database"),
    ("update_geo_ip_config", "update_geoip_config"),
    (
        "apply_security_profile",
        "apply_proxy_security_header_profile",
    ),
    ("add_error_page_theme", "create_error_page_theme"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn name(action: AuditAction) -> String {
        serde_json::to_value(action)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// An old name that is also a current name would rewrite fresh entries on every start, and a
    /// chain (a -> b, b -> c) would leave entries on a name that no longer exists.
    #[test]
    fn no_old_action_name_is_still_in_use() {
        let current: Vec<String> = AuditAction::ALL.iter().map(|&a| name(a)).collect();
        for (old, _) in RENAMED_ACTIONS {
            assert!(
                !current.iter().any(|c| c == old),
                "{old} is still an action name"
            );
        }
    }

    /// Every old entry must land on an action that the log can still read.
    #[test]
    fn every_new_action_name_is_readable() {
        for (old, new) in RENAMED_ACTIONS {
            let parsed = serde_json::from_value::<AuditAction>(serde_json::Value::from(*new));
            assert!(
                parsed.is_ok(),
                "{old} moves to {new}, which is not an action"
            );
        }
    }

    /// The action filter of the audit page lists `ALL`, so an action missing from it cannot be
    /// selected. Every action has a label key in the locale file, so the keys name every action.
    #[test]
    fn all_lists_every_action() {
        let locale: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(include_str!("../locales/en.json")).unwrap_or_default();
        let listed: Vec<String> = AuditAction::ALL.iter().map(|&a| name(a)).collect();
        let missing: Vec<&str> = locale
            .keys()
            .filter_map(|key| key.strip_prefix("audit.action_"))
            .filter(|action| !listed.iter().any(|l| l == action))
            .collect();
        assert!(missing.is_empty(), "actions missing from ALL: {missing:?}");
    }
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

use crate::log::LogFormat;
use clap::{Args, Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use tracing_subscriber::filter::LevelFilter;

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start server
    Start(StartArgs),
    /// Add user
    AddUser(AddUserArgs),
    /// Run the containers of a deployment platform target for a master
    Agent(AgentArgs),
    /// Serve the listeners that a master hands over over IPC
    Worker(WorkerArgs),
    /// Create or restore an encrypted disaster recovery bundle
    DisasterRecovery(DisasterRecoveryArgs),
}

#[derive(Args)]
pub struct DisasterRecoveryArgs {
    #[command(subcommand)]
    pub action: DisasterRecoveryAction,
}

#[derive(Subcommand)]
pub enum DisasterRecoveryAction {
    /// Create a disaster recovery backup bundle of all platform configuration and databases
    Backup(DisasterRecoveryBackupCliArgs),
    /// Restore all platform configuration and databases from a disaster recovery backup bundle
    Restore(DisasterRecoveryRestoreCliArgs),
}

#[derive(Args)]
pub struct DisasterRecoveryBackupCliArgs {
    /// Path where the encrypted .r3bundle file will be written
    #[clap(short, long)]
    pub output: Option<PathBuf>,

    /// Passphrase to encrypt the bundle with AES-256-GCM
    #[clap(short, long, env = "R3V3RS3_DR_PASSPHRASE")]
    pub passphrase: Option<String>,

    /// Config directory containing r3v3rs3 configuration and databases
    #[clap(short, long, env = "R3V3RS3_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    /// Replicate the backup to configured S3 storage
    #[clap(long, default_value_t = false)]
    pub upload_s3: bool,
}

#[derive(Args)]
pub struct DisasterRecoveryRestoreCliArgs {
    /// Path to the encrypted .r3bundle file
    #[clap(short, long)]
    pub file: PathBuf,

    /// Passphrase to decrypt the bundle
    #[clap(short, long, env = "R3V3RS3_DR_PASSPHRASE")]
    pub passphrase: Option<String>,

    /// Target config directory where configuration and databases will be restored
    #[clap(short, long, env = "R3V3RS3_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct AgentArgs {
    /// The agent port of the master as HOST:PORT.
    #[clap(long, value_name = "HOST:PORT", env = "R3V3RS3_AGENT_MASTER")]
    pub master: String,

    /// The enrollment token of the target. Only the first start needs it.
    #[clap(
        long,
        value_name = "TOKEN",
        env = "R3V3RS3_AGENT_TOKEN",
        hide_env_values = true
    )]
    pub token: Option<String>,

    /// The directory of the key and the certificate of the agent.
    #[clap(long, value_name = "DIR", env = "R3V3RS3_AGENT_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// The Docker Engine API that runs the containers of the target.
    #[clap(
        long,
        value_name = "ENDPOINT",
        default_value = "unix:///var/run/docker.sock",
        env = "R3V3RS3_AGENT_DOCKER"
    )]
    pub docker: String,

    #[clap(
        long,
        short,
        value_name = "LEVEL",
        default_value = "info",
        env = "R3V3RS3_AGENT_LOG_LEVEL"
    )]
    pub log_level: LevelFilter,
}

#[derive(Args)]
pub struct WorkerArgs {
    /// The file descriptor of the IPC socket that the master opened.
    #[clap(long, value_name = "FD", env = "R3V3RS3_WORKER_IPC")]
    pub ipc: i32,

    #[clap(long, short, value_name = "DIR", env = "R3V3RS3_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    #[clap(long, short = 'd', value_name = "DIR", env = "R3V3RS3_LOG_DIR")]
    pub log_dir: Option<PathBuf>,

    #[clap(
        long,
        short,
        value_name = "LEVEL",
        default_value = "info",
        env = "R3V3RS3_LOG_LEVEL"
    )]
    pub log_level: LevelFilter,
}

#[derive(Args)]
pub struct StartArgs {
    #[clap(long, value_name = "FILE", env = "R3V3RS3_LOG")]
    pub log: Option<PathBuf>,

    #[clap(long, value_name = "FILE", env = "R3V3RS3_ACCESS_LOG")]
    pub access_log: Option<PathBuf>,

    #[clap(
        long,
        short,
        value_name = "LEVEL",
        default_value = "info",
        env = "R3V3RS3_LOG_LEVEL"
    )]
    pub log_level: LevelFilter,

    #[clap(
        long,
        short,
        value_name = "LEVEL",
        default_value = "info",
        env = "R3V3RS3_ACCESS_LOG_LEVEL"
    )]
    pub access_log_level: LevelFilter,

    #[clap(
        long,
        value_enum,
        value_name = "FORMAT",
        default_value = "text",
        env = "R3V3RS3_LOG_FORMAT"
    )]
    pub log_format: LogFormat,

    #[clap(
        long,
        short,
        value_name = "ADDR",
        default_value = "127.0.0.1:46492",
        env = "R3V3RS3_WEBUI"
    )]
    pub webui: SocketAddr,

    #[clap(long, short, env = "R3V3RS3_NO_WEBUI", conflicts_with = "webui")]
    pub no_webui: bool,

    #[clap(long, short, value_name = "DIR", env = "R3V3RS3_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    #[clap(long, short = 'd', value_name = "DIR", env = "R3V3RS3_LOG_DIR")]
    pub log_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct AddUserArgs {
    pub name: String,

    #[clap(long, short, value_name = "PASSWORD")]
    pub password: Option<String>,

    #[clap(long, short, value_name = "DIR", env = "R3V3RS3_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    #[clap(long)]
    pub totp: bool,

    /// The role of the account: admin, editor or viewer.
    #[clap(long, value_name = "ROLE", default_value = "admin", value_parser = parse_role)]
    pub role: r3v3rs3_api::auth::Role,
}

fn parse_role(value: &str) -> Result<r3v3rs3_api::auth::Role, String> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|_| format!("unknown role {value}, use admin, editor or viewer"))
}

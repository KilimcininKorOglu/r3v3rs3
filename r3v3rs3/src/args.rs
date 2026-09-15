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
    /// Manage the cluster store
    Cluster(ClusterArgs),
}

#[derive(Args)]
pub struct ClusterArgs {
    #[command(subcommand)]
    pub command: ClusterCommand,
}

#[derive(Subcommand)]
pub enum ClusterCommand {
    /// Write a new value encryption key to a file
    Keygen(KeygenArgs),
    /// Encrypt every value of the cluster store with the first key of `cluster.encryption_key_files`
    Rekey(ClusterConfigArgs),
    /// Copy the configuration files into an empty cluster store
    Import(ClusterConfigArgs),
}

#[derive(Args)]
pub struct KeygenArgs {
    /// The key file. It must not exist.
    pub file: PathBuf,
}

#[derive(Args)]
pub struct ClusterConfigArgs {
    #[clap(long, short, value_name = "DIR", env = "R3V3RS3_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,
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

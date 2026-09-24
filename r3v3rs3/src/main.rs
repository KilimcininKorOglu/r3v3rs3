#![forbid(unsafe_code)]

use clap::Parser;
use directories::ProjectDirs;
use r3v3rs3::agent::client::{AgentClient, AgentTiming};
use r3v3rs3::agent::compose::AgentCompose;
use r3v3rs3::agent::executor::Executor;
use r3v3rs3::agent::token::EnrollmentToken;
use r3v3rs3::args::StartArgs;
use r3v3rs3::args::{AgentArgs, ClusterArgs, ClusterCommand, Command};
use r3v3rs3::build::compose::DockerCompose;
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::file::FileStorage;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::server::{Server, ServerChannels};
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::auth::MIN_PASSWORD_LENGTH;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::{self, FilterExt};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = r3v3rs3::args::Cli::parse();

    match args.command {
        Command::Start(args) => start(args).await?,
        Command::AddUser(args) => add_user(args).await?,
        Command::Cluster(args) => cluster(args).await?,
        Command::Agent(args) => agent(args).await?,
    }

    Ok(())
}

/// Enrolls the agent at its first start, then holds its connection to the master until SIGINT.
async fn agent(args: AgentArgs) -> anyhow::Result<()> {
    // A service manager writes the output to a file or the journal, which shows no colors.
    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => ProjectDirs::from("", "", "r3v3rs3")
            .ok_or_else(|| {
                anyhow::anyhow!("failed to get project directories, try setting --data-dir")
            })?
            .data_dir()
            .join("agent"),
    };
    let token = args
        .token
        .as_deref()
        .map(str::parse::<EnrollmentToken>)
        .transpose()?;
    let runtime = Arc::new(r3v3rs3::platform::docker_runtime(&args.docker)?);
    let compose = AgentCompose::new(
        Arc::new(DockerCompose::new(args.docker)),
        data_dir.join("compose"),
    );
    let executor = Executor::new(runtime, compose);
    let client = AgentClient::new(args.master, data_dir, AgentTiming::default(), executor);
    let identity = client.identity(token.as_ref()).await?;
    info!(target = %identity.target, "the agent starts");
    tokio::select! {
        result = client.run(&identity) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c signal");
            Ok(())
        }
    }
}

async fn cluster(args: ClusterArgs) -> anyhow::Result<()> {
    match args.command {
        ClusterCommand::Keygen(args) => {
            let id = r3v3rs3::cluster::key_file::write_new_key_file(&args.file).await?;
            println!(
                "Wrote the encryption key {} to {}.",
                hex::encode(id),
                args.file.display()
            );
            eprintln!(
                "Keep a copy of the key file. The cluster data cannot be decrypted when every key file is lost."
            );
        }
        ClusterCommand::Rekey(args) => {
            let config_dir = get_config_dir(args.config_dir)?;
            let config = FileStorage::new(&config_dir).read_app_config().await?;
            let report = r3v3rs3::cluster::rekey::run(&config.cluster).await?;
            println!(
                "Encrypted {} values with the first key. {} values already used it, {} values are not encrypted and {} values were deleted during the run.",
                report.resealed, report.current, report.plain, report.deleted
            );
        }
        ClusterCommand::Import(args) => {
            let config_dir = get_config_dir(args.config_dir)?;
            let report = r3v3rs3::cluster::import::run(&config_dir).await?;
            println!(
                "Imported the config, {} ports, {} proxies, {} access lists, {} certificates, {} ACME entries and {} accounts.",
                report.ports,
                report.proxies,
                report.access_lists,
                report.certs,
                report.acmes,
                report.accounts
            );
        }
    }
    Ok(())
}

/// Reads `config.toml`. A missing file gives the default config. An invalid file is an error, so
/// a node of a cluster does not start with its local files.
async fn read_local_config(config_dir: &Path) -> anyhow::Result<AppConfig> {
    if !config_dir.join("config.toml").try_exists()? {
        return Ok(AppConfig::default());
    }
    FileStorage::new(config_dir).read_app_config().await
}

/// Starts the log file, the access log file and the log database. The returned guards flush the
/// files, so the caller keeps them until it exits.
async fn init_logging(
    args: &mut StartArgs,
    log_dir: &Path,
) -> anyhow::Result<(Option<WorkerGuard>, Option<WorkerGuard>)> {
    let (log, log_guard) = r3v3rs3::log::create_layer(
        log_dir,
        args.log.take(),
        "r3v3rs3.log",
        args.log_level,
        args.log_format,
    )?;
    let (access_log, access_guard) = r3v3rs3::log::create_layer(
        log_dir,
        args.access_log.take(),
        "access.log",
        args.access_log_level,
        args.log_format,
    )?;
    let db = DatabaseLayer::new(&log_dir.join("log.db"), args.log_level).await?;

    let access_log_filter =
        filter::filter_fn(|metadata| metadata.target().starts_with("r3v3rs3::access_log"));
    let is_span = filter::filter_fn(|metadata| metadata.is_span());
    tracing_subscriber::registry()
        .with(log.with_filter(access_log_filter.clone().not()))
        .with(access_log.with_filter(access_log_filter.or(is_span)))
        .with(db)
        .init();
    Ok((log_guard, access_guard))
}

/// Builds the server. A node of a cluster reads its state from the store and starts the cluster
/// tasks, and every other server reads its state from the config directory.
async fn new_server(
    app_info: AppInfo,
    local: AppConfig,
    config_dir: &Path,
) -> anyhow::Result<(Server, ServerChannels)> {
    if !local.cluster.enabled {
        return Ok(Server::new(app_info, FileStorage::new(config_dir)).await);
    }
    let storage = Arc::new(KvStorage::open(local).await?);
    let (server, channels) = Server::new_shared(app_info, storage.clone()).await;
    r3v3rs3::cluster::spawn_tasks(storage, channels.command.clone());
    Ok((server, channels))
}

async fn start(mut args: StartArgs) -> anyhow::Result<()> {
    let log_dir = get_log_dir(args.log_dir.take())?;
    fs::create_dir_all(&log_dir)?;
    let _guards = init_logging(&mut args, &log_dir).await?;

    let config_dir = get_config_dir(args.config_dir.take())?;
    fs::create_dir_all(&config_dir)?;

    let app_info = new_appinfo(&config_dir, &log_dir);
    let local = read_local_config(&config_dir).await?;

    let (server, channels) = new_server(app_info.clone(), local, &config_dir).await?;
    r3v3rs3::cdn::fetch::spawn_refresh_task(channels.command.clone());
    let server_task = tokio::spawn(server.start());
    let event_send = channels.event.clone();

    serve_admin(app_info, &args, channels).await;

    let _ = event_send.send(r3v3rs3_api::event::ServerEvent::Shutdown);
    server_task.await??;

    Ok(())
}

/// Runs the admin API until it fails or the process gets SIGINT. With `--no-webui` it waits for
/// the signal only.
async fn serve_admin(app_info: AppInfo, args: &StartArgs, channels: ServerChannels) {
    let webui_enabled = !args.no_webui;
    tokio::select! {
        r = r3v3rs3::admin::start_admin(app_info, args.webui, channels.command, channels.callback, channels.event, channels.accounts), if webui_enabled => {
            if let Err(err) = r {
                error!("admin error: {}", err);
            }
        }
        _ =  tokio::signal::ctrl_c() => {
            info!("received ctrl-c signal");
        }
    };
}

/// Writes the account to the store of a cluster node, or to the config directory of every other
/// server.
async fn create_account(
    args: &r3v3rs3::args::AddUserArgs,
    password: &str,
    local: AppConfig,
    config_dir: &Path,
) -> anyhow::Result<r3v3rs3_api::auth::Account> {
    if local.cluster.enabled {
        let storage = KvStorage::open(local).await?;
        return Ok(storage
            .add_account(&args.name, password, args.totp, args.role)
            .await?);
    }
    let files = FileStorage::new(config_dir);
    Ok(files
        .add_account(&args.name, password, args.totp, args.role)
        .await?)
}

async fn add_user(mut args: r3v3rs3::args::AddUserArgs) -> anyhow::Result<()> {
    let config_dir = get_config_dir(args.config_dir.take())?;
    let local = read_local_config(&config_dir).await?;
    let password = match args.password.take() {
        Some(password) => password,
        None => rpassword::prompt_password("password?: ")?,
    };
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        anyhow::bail!("the password needs at least {MIN_PASSWORD_LENGTH} characters");
    }
    let account = create_account(&args, &password, local, &config_dir).await?;
    if let Some(totp) = account.totp {
        println!("\nUse this code to setup your TOTP client:\n{totp}\n");
    }
    Ok(())
}

fn get_config_dir(dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = dir {
        Ok(dir)
    } else {
        let dir = ProjectDirs::from("", "", "r3v3rs3").ok_or_else(|| {
            anyhow::anyhow!("failed to get project directories, try setting --config-dir")
        })?;
        Ok(dir.config_dir().to_owned())
    }
}

fn get_log_dir(dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = dir {
        Ok(dir)
    } else {
        let dir = ProjectDirs::from("", "", "r3v3rs3").ok_or_else(|| {
            anyhow::anyhow!("failed to get project directories, try setting --log-dir")
        })?;
        Ok(dir.data_dir().join("logs"))
    }
}

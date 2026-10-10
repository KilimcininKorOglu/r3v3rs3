#![forbid(unsafe_code)]

use anyhow::Context as _;
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
    // The provider pins the key exchange groups of every TLS config, including the post-quantum
    // X25519MLKEM768 hybrid.
    tokio_rustls::rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install the rustls crypto provider");
    let args = r3v3rs3::args::Cli::parse();
    let exe_args: Vec<String> = std::env::args().skip(1).collect();

    match args.command {
        Command::Start(args) => start(args, exe_args).await?,
        Command::AddUser(args) => add_user(args).await?,
        Command::Cluster(args) => cluster(args).await?,
        Command::Agent(args) => agent(args).await?,
        Command::Worker(args) => worker(args).await?,
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

/// Serves the listeners that a master process hands over over the IPC socket.
async fn worker(args: r3v3rs3::args::WorkerArgs) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let config_dir = get_config_dir(args.config_dir)?;
    let log_dir = get_log_dir(args.log_dir)?;
    r3v3rs3::server::worker::ensure_dirs(&config_dir, &log_dir)?;
    info!("the worker starts");
    r3v3rs3::server::worker::run(args.ipc, &config_dir, &log_dir).await
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
    listeners: Vec<r3v3rs3::server::fdmanifest::ListenerFd>,
) -> anyhow::Result<(Server, ServerChannels)> {
    if !local.cluster.enabled {
        return Ok(Server::new_shared_inheriting(
            app_info,
            Arc::new(FileStorage::new(config_dir)),
            listeners,
        )
        .await);
    }
    let storage = Arc::new(KvStorage::open(local).await?);
    let (server, channels) = if listeners.is_empty() {
        Server::new_shared(app_info, storage.clone()).await
    } else {
        Server::new_shared_inheriting(app_info, storage.clone(), listeners).await
    };
    r3v3rs3::cluster::spawn_tasks(storage, channels.command.clone());
    Ok((server, channels))
}

async fn start(mut args: StartArgs, exe_args: Vec<String>) -> anyhow::Result<()> {
    let (app_info, local, server, channels) = prepare(&mut args).await?;
    r3v3rs3::cdn::fetch::spawn_refresh_task(channels.command.clone());
    spawn_supervisor(&app_info, &local, &server, &channels).await?;
    // The workers serve the ports, so the listeners of this process stay out of the upgrade
    // manifest and this process cannot upgrade again.
    let manifest = if std::env::var_os("R3V3RS3_UPGRADE_IPC_FD").is_none() {
        Some(server.listener_manifest()?)
    } else {
        None
    };
    let server_task = tokio::spawn(server.start());
    let event_send = channels.event.clone();

    serve_admin(app_info, &args, channels, exe_args, manifest).await?;

    let _ = event_send.send(r3v3rs3_api::event::ServerEvent::Shutdown);
    server_task.await??;

    Ok(())
}

/// The logging, the directories, the config and the server of a fresh start.
async fn prepare(
    args: &mut StartArgs,
) -> anyhow::Result<(
    AppInfo,
    r3v3rs3_api::app::AppConfig,
    r3v3rs3::server::Server,
    ServerChannels,
)> {
    let log_dir = get_log_dir(args.log_dir.take())?;
    fs::create_dir_all(&log_dir)?;
    let _guards = init_logging(args, &log_dir).await?;

    let config_dir = get_config_dir(args.config_dir.take())?;
    fs::create_dir_all(&config_dir)?;

    let app_info = new_appinfo(&config_dir, &log_dir);
    let local = read_local_config(&config_dir).await?;
    let listeners = inherit_listeners().await?;

    let (server, channels) =
        new_server(app_info.clone(), local.clone(), &config_dir, listeners).await?;
    Ok((app_info, local, server, channels))
}

/// After a SIGUSR2 the new process inherits the listeners of the previous one.
async fn inherit_listeners() -> anyhow::Result<Vec<r3v3rs3::server::fdmanifest::ListenerFd>> {
    let Ok(fd) = std::env::var("R3V3RS3_UPGRADE_IPC_FD") else {
        return Ok(Vec::new());
    };
    let listeners = r3v3rs3::server::upgrade::child_handover(fd.parse()?).await?;
    info!(
        listeners = listeners.len(),
        "inherited the listeners of the previous process"
    );
    Ok(listeners)
}

/// Runs the worker supervisor in the background when the worker mode is on.
async fn spawn_supervisor(
    app_info: &AppInfo,
    local: &r3v3rs3_api::app::AppConfig,
    server: &r3v3rs3::server::Server,
    channels: &ServerChannels,
) -> anyhow::Result<()> {
    if local.worker_mode != r3v3rs3_api::app::WorkerMode::Workers {
        return Ok(());
    }
    let config_dir = app_info.config_path.clone();
    let manifest = server.listener_manifest()?;
    let supervisor = r3v3rs3::server::supervisor::run(
        channels.event.clone(),
        channels.command.clone(),
        std::env::current_exe()?,
        app_info.clone(),
        read_local_config(&config_dir).await?,
        manifest,
    );
    tokio::spawn(async move {
        if let Err(err) = supervisor.await {
            error!(%err, "the worker supervisor stopped");
        }
    });
    Ok(())
}

/// Runs the admin API until it fails or the process gets SIGINT, or hands the listeners to a
/// new process on SIGUSR2. With `--no-webui` it waits for a signal only.
async fn serve_admin(
    app_info: AppInfo,
    args: &StartArgs,
    channels: ServerChannels,
    exe_args: Vec<String>,
    manifest: Option<Vec<r3v3rs3::server::fdmanifest::ListenerFd>>,
) -> anyhow::Result<()> {
    let webui_enabled = !args.no_webui;
    let mut upgrade = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2())
        .context("failed to install the SIGUSR2 handler")?;
    tokio::select! {
        r = r3v3rs3::admin::start_admin(app_info, args.webui, channels.command, channels.callback, channels.event, channels.accounts), if webui_enabled => {
            if let Err(err) = r {
                error!("admin error: {}", err);
            }
        }
        _ =  tokio::signal::ctrl_c() => {
            info!("received ctrl-c signal");
        }
        _ = upgrade.recv() => {
            info!("received the upgrade signal");
            match manifest {
                Some(manifest) => {
                    if let Err(err) = r3v3rs3::server::upgrade::hand_over(&exe_args, manifest).await {
                        error!(%err, "the upgrade failed, this process keeps serving");
                    }
                }
                None => {
                    // This process already inherited its listeners, so an upgrade at the same
                    // address would fight the process before it.
                    error!("this process started from an upgrade, so it does not upgrade again");
                }
            }
        }
    };
    Ok(())
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

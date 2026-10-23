#![forbid(unsafe_code)]

use anyhow::Context as _;
use clap::Parser;
use directories::ProjectDirs;
use r3v3rs3::agent::client::{AgentClient, AgentTiming};
use r3v3rs3::agent::compose::AgentCompose;
use r3v3rs3::agent::executor::Executor;
use r3v3rs3::agent::token::EnrollmentToken;
use r3v3rs3::args::StartArgs;
use r3v3rs3::args::{AgentArgs, Command};
use r3v3rs3::build::compose::DockerCompose;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::config::sqlite::SqliteStorage;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::server::{Server, ServerChannels};
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::auth::MIN_PASSWORD_LENGTH;
use std::fs;
use std::io::IsTerminal;
use std::net::SocketAddr;
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
        Command::Agent(args) => agent(args).await?,
        Command::Worker(args) => worker(args).await?,
        Command::DisasterRecovery(args) => disaster_recovery_cli(args).await?,
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

/// Reads the server config. Uses SqliteStorage if present or migrates from config.toml.
async fn read_local_config(config_dir: &Path) -> anyhow::Result<AppConfig> {
    SqliteStorage::read_local_config(config_dir).await
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

/// Builds the server that reads its state from the config directory.
async fn new_server(
    app_info: AppInfo,
    _local: AppConfig,
    config_dir: &Path,
    listeners: Vec<r3v3rs3::server::fdmanifest::ListenerFd>,
) -> anyhow::Result<(Server, ServerChannels)> {
    let storage = Arc::new(SqliteStorage::open(config_dir).await?);
    Ok(Server::new_shared_inheriting(app_info, storage, listeners).await)
}

async fn start(mut args: StartArgs, exe_args: Vec<String>) -> anyhow::Result<()> {
    let (app_info, local, server, channels, webui_listener) = prepare(&mut args).await?;
    r3v3rs3::cdn::fetch::spawn_refresh_task(channels.command.clone());
    r3v3rs3::appstore::fetch::spawn_refresh_task(channels.command.clone());
    r3v3rs3::engines::install_embedded();
    r3v3rs3::engines::fetch::spawn_refresh_task(channels.command.clone());
    r3v3rs3::server::self_upgrade::spawn_check_task(
        channels.command.clone(),
        server.update_check_interval(),
    );
    spawn_supervisor(&app_info, &local, &server, &channels).await?;
    // The workers serve the ports, so the worker master keeps the ports out of the manifest of
    // the workers. The listener of the WebUI travels with the ports of a master upgrade, so the
    // new process serves the panel without a bind that the old one holds. A process that
    // started from an upgrade owns its inherited listeners and hands their copies over again.
    let manifest = if local.worker_mode == r3v3rs3_api::app::WorkerMode::Workers {
        None
    } else {
        Some(manifest_with_webui(&server, webui_listener.as_ref())?)
    };
    let server_task = tokio::spawn(server.start());
    let event_send = channels.event.clone();
    // The listeners are bound, so systemd may mark the service active (Type=notify).
    r3v3rs3::server::notify::ready();

    // The upgrade endpoint installs a release and asks this loop for the handover.
    let (upgrade_requests, upgrade_receiver) = tokio::sync::mpsc::channel(1);
    serve_admin(
        app_info,
        channels,
        exe_args,
        manifest,
        webui_listener,
        upgrade_requests,
        upgrade_receiver,
    )
    .await?;

    let _ = event_send.send(r3v3rs3_api::event::ServerEvent::Shutdown);
    server_task.await??;

    Ok(())
}

/// The listeners of an upgrade: the ports of the server plus the listener of the WebUI.
fn manifest_with_webui(
    server: &r3v3rs3::server::Server,
    webui_listener: Option<&tokio::net::TcpListener>,
) -> std::io::Result<Vec<r3v3rs3::server::fdmanifest::ListenerFd>> {
    use std::os::fd::AsFd as _;
    let mut manifest = server.listener_manifest()?;
    let Some(listener) = webui_listener else {
        return Ok(manifest);
    };
    let fd = listener.as_fd().try_clone_to_owned()?;
    manifest.push(r3v3rs3::server::fdmanifest::ListenerFd::new(
        r3v3rs3::server::fdmanifest::ListenerProto::Tcp,
        listener.local_addr()?,
        fd,
    ));
    Ok(manifest)
}

/// The logging, the directories, the config and the server of a fresh start. Also returns the
/// listener of the WebUI: the one that an upgrade inherited, or a fresh bind.
async fn prepare(
    args: &mut StartArgs,
) -> anyhow::Result<(
    AppInfo,
    r3v3rs3_api::app::AppConfig,
    r3v3rs3::server::Server,
    ServerChannels,
    Option<tokio::net::TcpListener>,
)> {
    let log_dir = get_log_dir(args.log_dir.take())?;
    fs::create_dir_all(&log_dir)?;
    let _guards = init_logging(args, &log_dir).await?;

    let config_dir = get_config_dir(args.config_dir.take())?;
    fs::create_dir_all(&config_dir)?;

    let app_info = new_appinfo(&config_dir, &log_dir);
    let local = read_local_config(&config_dir).await?;
    let (listeners, inherited_webui) = inherit_listeners(args.webui).await?;
    let webui_listener = webui_listener(args, inherited_webui).await?;

    let (server, channels) =
        new_server(app_info.clone(), local.clone(), &config_dir, listeners).await?;
    Ok((app_info, local, server, channels, webui_listener))
}

/// The listener of the WebUI: the one that an upgrade inherited, or a fresh bind.
async fn webui_listener(
    args: &StartArgs,
    inherited: Option<tokio::net::TcpListener>,
) -> anyhow::Result<Option<tokio::net::TcpListener>> {
    match inherited {
        Some(listener) => Ok(Some(listener)),
        None if !args.no_webui => Ok(Some(tokio::net::TcpListener::bind(args.webui).await?)),
        None => Ok(None),
    }
}

/// After a SIGUSR2 the new process inherits the listeners of the previous one. The entry that
/// carries the WebUI address becomes the listener of the admin server, the rest are the ports.
async fn inherit_listeners(
    webui: SocketAddr,
) -> anyhow::Result<(
    Vec<r3v3rs3::server::fdmanifest::ListenerFd>,
    Option<tokio::net::TcpListener>,
)> {
    let Ok(fd) = std::env::var("R3V3RS3_UPGRADE_IPC_FD") else {
        return Ok((Vec::new(), None));
    };
    let listeners = r3v3rs3::server::upgrade::child_handover(fd.parse()?).await?;
    info!(
        listeners = listeners.len(),
        "inherited the listeners of the previous process"
    );
    let mut ports = Vec::new();
    let mut webui_listener = None;
    for listener in listeners {
        if listener.addr == webui {
            let r3v3rs3::server::fdmanifest::ListenerFd { fd, .. } = listener;
            let std_listener = std::net::TcpListener::from(fd);
            std_listener.set_nonblocking(true)?;
            webui_listener = Some(tokio::net::TcpListener::from_std(std_listener)?);
        } else {
            ports.push(listener);
        }
    }
    Ok((ports, webui_listener))
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

/// Runs the admin API on the listener that `start` bound, until it fails or the process gets
/// SIGINT, or hands the listeners to a new process. The endpoint that installs a release asks
/// for the handover over `upgrade_receiver`; SIGUSR2 asks for the same handover. Without the
/// listener of the panel it waits for a signal only.
async fn serve_admin(
    app_info: AppInfo,
    channels: ServerChannels,
    exe_args: Vec<String>,
    manifest: Option<Vec<r3v3rs3::server::fdmanifest::ListenerFd>>,
    webui_listener: Option<tokio::net::TcpListener>,
    upgrade_sender: tokio::sync::mpsc::Sender<()>,
    mut upgrade_receiver: tokio::sync::mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    let mut upgrade = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2())
        .context("failed to install the SIGUSR2 handler")?;
    // The signal loop outlives failed upgrades: a failed handover returns to the loop and
    // this process keeps serving.
    let signals = async {
        loop {
            tokio::select! {
                _ = upgrade.recv() => {
                    info!("received the upgrade signal");
                    if try_hand_over(&exe_args, manifest.as_deref()).await {
                        return;
                    }
                }
                _ = upgrade_receiver.recv() => {
                    info!("received the upgrade request of the panel");
                    if try_hand_over(&exe_args, manifest.as_deref()).await {
                        return;
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("received ctrl-c signal");
                    return;
                }
            }
        }
    };
    tokio::pin!(signals);
    match webui_listener {
        Some(listener) => {
            tokio::select! {
                r = r3v3rs3::admin::start_admin_on(app_info, listener, channels.command, channels.callback, channels.event, channels.accounts, upgrade_sender) => {
                    if let Err(err) = r {
                        error!("admin error: {}", err);
                    }
                }
                _ = &mut signals => {}
            }
        }
        None => (&mut signals).await,
    }
    Ok(())
}

/// Hands the listeners to a new process. Returns `false` when the handover failed and this
/// process must keep serving, for example when the process itself started from an upgrade.
async fn try_hand_over(
    exe_args: &[String],
    manifest: Option<&[r3v3rs3::server::fdmanifest::ListenerFd]>,
) -> bool {
    match manifest {
        Some(manifest) => match r3v3rs3::server::upgrade::hand_over(exe_args, manifest).await {
            Ok(()) => true,
            Err(err) => {
                error!(err = ?err, "the upgrade failed, this process keeps serving");
                false
            }
        },
        None => {
            // The workers hold the listeners, so a master upgrade would hand over nothing.
            error!("the workers hold the listeners, so this master does not upgrade");
            false
        }
    }
}

/// Writes the account to the config directory.
async fn create_account(
    args: &r3v3rs3::args::AddUserArgs,
    password: &str,
    _local: AppConfig,
    config_dir: &Path,
) -> anyhow::Result<r3v3rs3_api::auth::Account> {
    let storage = SqliteStorage::open(config_dir).await?;
    Ok(storage
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

async fn disaster_recovery_cli(args: r3v3rs3::args::DisasterRecoveryArgs) -> anyhow::Result<()> {
    match args.action {
        r3v3rs3::args::DisasterRecoveryAction::Backup(cli_args) => {
            cli_backup_action(cli_args).await?;
        }
        r3v3rs3::args::DisasterRecoveryAction::Restore(cli_args) => {
            cli_restore_action(cli_args).await?;
        }
    }
    Ok(())
}

async fn cli_backup_action(
    cli_args: r3v3rs3::args::DisasterRecoveryBackupCliArgs,
) -> anyhow::Result<()> {
    let config_dir = get_config_dir(cli_args.config_dir)?;
    let passphrase = match cli_args.passphrase {
        Some(p) => p,
        None => rpassword::prompt_password("Enter encryption passphrase (or blank for none): ")?,
    };
    let (entry, path) = r3v3rs3::platform::disaster_recovery::run_cli_backup(
        &config_dir,
        cli_args.output.as_deref(),
        &passphrase,
        cli_args.upload_s3,
    )
    .await?;
    println!("Disaster recovery backup created successfully:");
    println!("  ID:          {}", entry.id);
    println!("  Path:        {}", path.display());
    println!("  Size:        {} bytes", entry.size);
    println!("  SHA-256:     {}", entry.sha256);
    println!("  Files Count: {}", entry.files_count);
    Ok(())
}

async fn cli_restore_action(
    cli_args: r3v3rs3::args::DisasterRecoveryRestoreCliArgs,
) -> anyhow::Result<()> {
    let config_dir = get_config_dir(cli_args.config_dir)?;
    let passphrase = match cli_args.passphrase {
        Some(p) => p,
        None => rpassword::prompt_password("Enter decryption passphrase: ")?,
    };
    let restored = r3v3rs3::platform::disaster_recovery::run_cli_restore(
        &cli_args.file,
        &config_dir,
        &passphrase,
    )
    .await?;
    println!("Disaster recovery restore completed successfully!");
    println!("  Files restored: {}", restored);
    println!("  Config dir:     {}", config_dir.display());
    println!("\nYou can now start the server with: r3-server start");
    Ok(())
}

#![forbid(unsafe_code)]

use clap::Parser;
use directories::ProjectDirs;
use r3v3rs3::args::StartArgs;
use r3v3rs3::args::{ClusterArgs, ClusterCommand, Command};
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::file::FileStorage;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::server::Server;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::MIN_PASSWORD_LENGTH;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::filter::{self, FilterExt};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = r3v3rs3::args::Cli::parse();

    match args.command {
        Command::Start(args) => start(args).await?,
        Command::AddUser(args) => add_user(args).await?,
        Command::Cluster(args) => cluster(args).await?,
    }

    Ok(())
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

async fn start(args: StartArgs) -> anyhow::Result<()> {
    let log_dir = get_log_dir(args.log_dir)?;
    fs::create_dir_all(&log_dir)?;

    let (log, _guard) = r3v3rs3::log::create_layer(
        &log_dir,
        args.log,
        "r3v3rs3.log",
        args.log_level,
        args.log_format,
    )?;
    let (access_log, _guard) = r3v3rs3::log::create_layer(
        &log_dir,
        args.access_log,
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

    let config_dir = get_config_dir(args.config_dir)?;
    fs::create_dir_all(&config_dir)?;

    let app_info = new_appinfo(&config_dir, &log_dir);
    let local = read_local_config(&config_dir).await?;

    let (server, channels) = if local.cluster.enabled {
        let storage = Arc::new(KvStorage::open(local).await?);
        let (server, channels) = Server::new_shared(app_info.clone(), storage.clone()).await;
        r3v3rs3::cluster::spawn_tasks(storage, channels.command.clone());
        (server, channels)
    } else {
        Server::new(app_info.clone(), FileStorage::new(&config_dir)).await
    };
    r3v3rs3::cdn::fetch::spawn_refresh_task(channels.command.clone());
    let server_task = tokio::spawn(server.start());
    let event_send = channels.event.clone();

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

    let _ = event_send.send(r3v3rs3_api::event::ServerEvent::Shutdown);
    server_task.await??;

    Ok(())
}

async fn add_user(args: r3v3rs3::args::AddUserArgs) -> anyhow::Result<()> {
    let config_dir = get_config_dir(args.config_dir)?;
    let local = read_local_config(&config_dir).await?;
    let password = if let Some(password) = args.password {
        password
    } else {
        rpassword::prompt_password("password?: ")?
    };
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        anyhow::bail!("the password needs at least {MIN_PASSWORD_LENGTH} characters");
    }
    let account = if local.cluster.enabled {
        let storage = KvStorage::open(local).await?;
        storage
            .add_account(&args.name, &password, args.totp, args.role)
            .await?
    } else {
        let files = FileStorage::new(&config_dir);
        files
            .add_account(&args.name, &password, args.totp, args.role)
            .await?
    };
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

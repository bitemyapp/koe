use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use koe::{
    config::{self, Config},
    ipc,
    protocol::{Action, Request, Response},
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use tokio::{io::BufReader, net::UnixStream};

#[derive(Parser)]
#[command(version, about = "Koe 声 — voice dictation for Aizome and Sekka")]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Configuration file (default: $XDG_CONFIG_HOME/aizome/koe.toml)"
    )]
    config: Option<PathBuf>,
    #[arg(long, global = true, help = "Override the private daemon socket")]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run the session daemon. Recording starts only on request.
    Daemon,
    /// Control dictation; start/stop can be bound to press/release for push-to-talk.
    Record {
        #[arg(value_enum)]
        action: Action,
    },
    /// Print machine-readable daemon status.
    Status,
    /// Stream newline-delimited JSON status for the Aizome shell or other bars.
    Watch {
        #[arg(long, help = "Emit Waybar custom-module JSON")]
        waybar: bool,
    },
    /// Transcribe a mono 16 kHz WAV to stdout, without clipboard changes.
    Transcribe {
        input: PathBuf,
        #[arg(long)]
        model: Option<PathBuf>,
    },
    /// Show, validate, or create configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Download verified local models and manage resident memory.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Check configuration, model, runtime, and desktop dependencies.
    Doctor,
}
#[derive(Subcommand)]
enum ConfigCommand {
    Show,
    Check,
    Init,
}
#[derive(Subcommand)]
enum ModelCommand {
    List,
    Download {
        #[arg(default_value = "base")]
        name: String,
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    Verify {
        name: String,
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    Unload,
}
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("koe: {e:#}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.config.map(Ok).unwrap_or_else(config::config_path)?;
    let socket = || cli.socket.clone().map(Ok).unwrap_or_else(ipc::socket_path);
    match cli.command {
        Command::Daemon => koe::daemon::run(Config::load(&path)?, &socket()?).await?,
        Command::Status => control(&socket()?, Action::Status).await?,
        Command::Record {
            action: Action::Subscribe,
        } => watch(&socket()?, false).await?,
        Command::Record { action } => control(&socket()?, action).await?,
        Command::Watch { waybar } => watch(&socket()?, waybar).await?,
        Command::Transcribe { input, model } => {
            let mut config = Config::load(&path)?;
            if let Some(model) = model {
                config.engine.model = model;
            }
            let samples = koe::audio::read_wav(&input, config.audio.max_seconds)?;
            let engine = koe::engine::Recognizer::new(config.engine);
            let text = engine
                .transcribe(samples, Arc::new(AtomicBool::new(false)))
                .await?;
            println!(
                "{}",
                koe::text::Processor::new(&config.replacements, config.output.append_space)?
                    .apply(&text)
            );
        }
        Command::Config { command } => match command {
            ConfigCommand::Show => print!("{}", toml::to_string_pretty(&Config::load(&path)?)?),
            ConfigCommand::Check => {
                Config::load(&path)?;
                println!("Configuration valid: {}", path.display());
            }
            ConfigCommand::Init => {
                use std::{io::Write, os::unix::fs::OpenOptionsExt};
                std::fs::create_dir_all(path.parent().context("configuration has no parent")?)?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .context(
                        "cannot create configuration (existing files are never overwritten)",
                    )?;
                file.write_all(toml::to_string_pretty(&Config::default())?.as_bytes())?;
                println!("{}", path.display());
            }
        },
        Command::Model { command } => match command {
            ModelCommand::List => {
                let dir = koe::models::directory()?;
                for model in koe::models::catalog().models {
                    let present = dir.join(format!("ggml-{}.bin", model.name)).is_file();
                    println!(
                        "{:<9} {:>6.1} MiB  {}",
                        model.name,
                        model.bytes as f64 / 1048576.0,
                        if present {
                            "present (use model verify to check)"
                        } else {
                            "not installed"
                        }
                    );
                }
            }
            ModelCommand::Download { name, directory } => {
                let dir = directory.map(Ok).unwrap_or_else(koe::models::directory)?;
                println!("{}", koe::models::download(&name, &dir).await?.display());
            }
            ModelCommand::Verify { name, directory } => {
                let catalog = koe::models::catalog();
                let model = catalog
                    .models
                    .iter()
                    .find(|m| m.name == name)
                    .context("unknown model")?;
                let dir = directory.map(Ok).unwrap_or_else(koe::models::directory)?;
                koe::models::verify(&dir.join(format!("ggml-{name}.bin")), model)?;
                println!("{name}: verified");
            }
            ModelCommand::Unload => control(&socket()?, Action::Unload).await?,
        },
        Command::Doctor => {
            let config = Config::load(&path)?;
            let mut missing = false;
            println!("Configuration: {} (valid)", path.display());
            println!(
                "Backend: {:?}; local compiled: {}",
                config.engine.backend,
                cfg!(feature = "local")
            );
            #[cfg(feature = "local")]
            println!(
                "CPU acceleration: {}",
                if cfg!(feature = "openblas") {
                    "OpenBLAS (runtime CPU selection)"
                } else {
                    "built-in Whisper kernels"
                }
            );
            if config.engine.backend == config::Backend::Local {
                let ok = config.engine.model.is_file() && cfg!(feature = "local");
                println!(
                    "Local model: {} ({})",
                    config.engine.model.display(),
                    if ok {
                        "present"
                    } else {
                        "missing model or backend"
                    }
                );
                missing |= !ok;
            }
            let mut programs = vec!["pw-record"];
            if config.output.mode != config::OutputMode::None {
                programs.push("wl-copy");
            }
            if config.output.mode == config::OutputMode::Paste
                && !koe::desktop::uses_aizome(&config.output)
            {
                programs.push("wtype");
            }
            for program in programs {
                let found = std::env::var_os("PATH").is_some_and(|p| {
                    std::env::split_paths(&p).any(|dir| {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::metadata(dir.join(program))
                            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    })
                });
                println!("{program}: {}", if found { "found" } else { "missing" });
                missing |= !found;
            }
            println!(
                "Aizome IPC: {}",
                if koe::desktop::aizome_socket().is_some() {
                    "available"
                } else {
                    "unset"
                }
            );
            match socket() {
                Ok(p) => println!("Daemon socket: {}", p.display()),
                Err(e) => {
                    println!("Runtime: {e}");
                    missing = true;
                }
            }
            ensure!(!missing, "resolve the missing dependencies above");
        }
    }
    Ok(())
}
async fn control(socket: &std::path::Path, action: Action) -> Result<()> {
    let response = ipc::request(socket, &Request::new(action)).await?;
    if action == Action::Last && response.ok {
        println!("{}", response.text.as_deref().unwrap_or_default());
    } else {
        println!("{}", serde_json::to_string(&response)?);
    }
    ensure!(
        response.ok,
        "{}",
        response.error.unwrap_or_else(|| "request failed".into())
    );
    Ok(())
}
async fn watch(socket: &std::path::Path, waybar: bool) -> Result<()> {
    let mut stream = UnixStream::connect(socket)
        .await
        .context("cannot connect to Koe; start the daemon first")?;
    ipc::write_json(&mut stream, &Request::new(Action::Subscribe)).await?;
    let mut reader = BufReader::new(stream);
    while let Some(line) = ipc::read_line(&mut reader).await? {
        let response: Response = serde_json::from_slice(&line)?;
        ensure!(response.ok, "subscription failed");
        if waybar {
            let phase = serde_json::to_value(response.status.phase)?
                .as_str()
                .unwrap()
                .to_owned();
            println!(
                "{}",
                serde_json::json!({"text":if phase == "idle" { "" } else { "Koe" },"class":phase,"alt":phase,"tooltip":format!("Koe: {phase} · {:.1}s", response.status.elapsed_ms as f64 / 1000.0)})
            );
        } else {
            println!("{}", serde_json::to_string(&response)?);
        }
    }
    Ok(())
}

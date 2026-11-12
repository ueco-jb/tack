use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "0.0.0.0:15470")]
        addr: SocketAddr,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        log: Option<PathBuf>,
        #[arg(long = "allow-host")]
        allow_hosts: Vec<String>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Serve {
            addr,
            db,
            log,
            allow_hosts,
        } => {
            let config = tack_server::Config {
                addr,
                db: db.unwrap_or_else(|| xdg_dir("XDG_DATA_HOME", ".local/share").join("tack.db")),
                request_log: log.unwrap_or_else(|| {
                    xdg_dir("XDG_STATE_HOME", ".local/state").join("requests.log")
                }),
                allowed_hosts: allow_hosts,
            };
            let result = tokio::runtime::Runtime::new()
                .map_err(|e| e.to_string())
                .and_then(|rt| {
                    rt.block_on(tack_server::serve(config))
                        .map_err(|e| e.message)
                });
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("tack: {message}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
        .unwrap_or_default()
        .join("tack")
}

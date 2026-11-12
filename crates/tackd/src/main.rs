use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use tack::Client;
use tack_core::api::{AllowClient, ClientList};

#[derive(Parser)]
#[command(
    version,
    about = "Task list server for the home network",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(flatten)]
    serve: Serve,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args)]
struct Serve {
    #[arg(long, default_value = "0.0.0.0:15470")]
    addr: SocketAddr,
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long)]
    log: Option<PathBuf>,
    #[arg(long = "allow-host")]
    allow_hosts: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Allow a client IP; without an IP, list the allowlist")]
    Allow {
        ip: Option<IpAddr>,
        #[arg(long, default_value_t = 15470)]
        port: u16,
    },
    #[command(about = "Remove a client IP; an empty allowlist allows everyone")]
    Deny {
        ip: IpAddr,
        #[arg(long, default_value_t = 15470)]
        port: u16,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => serve(cli.serve),
        Some(Command::Allow { ip, port }) => {
            let client = local(port);
            match ip {
                Some(ip) => client.post("/clients", &AllowClient { ip }),
                None => client.get("/clients", &[]),
            }
            .map(print_clients)
            .map_err(|e| e.message)
        }
        Some(Command::Deny { ip, port }) => local(port)
            .delete(&format!("/clients/{ip}"), &[])
            .map(print_clients)
            .map_err(|e| e.message),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tackd: {message}");
            ExitCode::FAILURE
        }
    }
}

fn serve(args: Serve) -> Result<(), String> {
    let config = tack_server::Config {
        addr: args.addr,
        db: args
            .db
            .unwrap_or_else(|| xdg_dir("XDG_DATA_HOME", ".local/share").join("tack.db")),
        request_log: args
            .log
            .unwrap_or_else(|| xdg_dir("XDG_STATE_HOME", ".local/state").join("requests.log")),
        allowed_hosts: args.allow_hosts,
    };
    tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())?
        .block_on(tack_server::serve(config))
        .map_err(|e| e.message)
}

fn local(port: u16) -> Client {
    Client::new(&format!("http://127.0.0.1:{port}"))
}

fn print_clients(list: ClientList) {
    if list.clients.is_empty() {
        println!("no allowlist, every client is allowed");
    }
    for ip in list.clients {
        println!("{ip}");
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

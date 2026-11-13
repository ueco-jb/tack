use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use tack::{Client, item_path};
use tack_core::api::{CreateItem, Deleted, Health, ItemList, ItemView, PatchItem, StatusList};
use tack_core::model::Category;
use tack_core::{Error, Result};

const DEFAULT_SERVER: &str = "http://localhost:15470";

#[derive(Parser)]
#[command(version, about = "Command line client for tackd")]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Server for this call only, instead of the saved one"
    )]
    url: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Show the saved server, or check and save a new one")]
    Server { url: Option<String> },
    #[command(about = "List items matching a query; no query lists everything not archived")]
    Ls { query: Vec<String> },
    #[command(about = "Show one item with its body")]
    Show {
        #[arg(required = true)]
        item: Vec<String>,
    },
    #[command(about = "Add an item; missing parents are created")]
    Add {
        #[arg(required = true)]
        path: Vec<String>,
        #[arg(short, long)]
        body: Option<String>,
        #[arg(short, long = "tag")]
        tags: Vec<String>,
        #[arg(short, long)]
        due: Option<Date>,
        #[arg(short, long)]
        status: Option<String>,
    },
    #[command(about = "Mark an item done")]
    Done {
        #[arg(required = true)]
        item: Vec<String>,
        #[arg(short, long, help = "Also when items under it are still open")]
        force: bool,
    },
    #[command(about = "Move or rename an item")]
    Mv { item: String, path: String },
    #[command(about = "Change an item")]
    Set {
        #[arg(required = true)]
        item: Vec<String>,
        #[arg(short, long)]
        status: Option<String>,
        #[arg(short, long)]
        body: Option<String>,
        #[arg(short, long = "tag", conflicts_with = "no_tags")]
        tags: Vec<String>,
        #[arg(long)]
        no_tags: bool,
        #[arg(short, long, conflicts_with = "no_due")]
        due: Option<Date>,
        #[arg(long)]
        no_due: bool,
        #[arg(long, conflicts_with = "unarchive")]
        archive: bool,
        #[arg(long)]
        unarchive: bool,
        #[arg(
            short,
            long,
            help = "Mark done also when items under it are still open"
        )]
        force: bool,
    },
    #[command(about = "Delete an item")]
    Rm {
        #[arg(required = true)]
        item: Vec<String>,
        #[arg(short, long, help = "Also delete everything under it")]
        recursive: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.url, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tack: {}", e.message);
            ExitCode::FAILURE
        }
    }
}

fn config_file() -> Result<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("tack").join("server"))
        .ok_or_else(|| Error::internal("neither XDG_CONFIG_HOME nor HOME is set"))
}

fn server(flag: Option<String>) -> Result<String> {
    if let Some(url) = flag {
        return Ok(normalize(&url));
    }
    let saved = std::fs::read_to_string(config_file()?).unwrap_or_default();
    if !saved.trim().is_empty() {
        return Ok(normalize(&saved));
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::invalid("no server set, run tack once in a terminal"));
    }
    eprint!("tack server [{DEFAULT_SERVER}]: ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| Error::internal(format!("cannot read the answer: {e}")))?;
    let answer = answer.trim();
    save_server(if answer.is_empty() {
        DEFAULT_SERVER
    } else {
        answer
    })
}

fn save_server(url: &str) -> Result<String> {
    let url = normalize(url);
    let _: Health = Client::new(&url).get("/health", &[])?;
    let file = config_file()?;
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, format!("{url}\n")));
    written.map_err(|e| Error::internal(format!("cannot write {}: {e}", file.display())))?;
    Ok(url)
}

fn normalize(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let has_port = match authority.rsplit_once(']') {
        Some((_, after)) => after.starts_with(':'),
        None => authority.contains(':'),
    };
    let port = if has_port { "" } else { ":15470" };
    let path = if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    };
    format!("{scheme}://{authority}{port}{path}")
}

fn run(flag: Option<String>, command: Command) -> Result<()> {
    let url = match command {
        Command::Server { url: Some(url) } => {
            println!("{}", save_server(&url)?);
            return Ok(());
        }
        Command::Server { url: None } => {
            println!("{}", server(flag)?);
            return Ok(());
        }
        _ => server(flag)?,
    };
    let client = &Client::new(&url);
    let style = || Style::new(client);
    match command {
        Command::Server { .. } => {}
        Command::Ls { query } => {
            let list: ItemList = client.get("/items", &[("q", &query.join(" "))])?;
            print!("{}", style().lines(&list.items));
        }
        Command::Show { item } => {
            let item: ItemView = client.get(&item_path(&item.join(" ")), &[])?;
            print!("{}", style().detail(&item));
        }
        Command::Add {
            path,
            body,
            tags,
            due,
            status,
        } => {
            let req = CreateItem {
                path: path.join(" "),
                body,
                status,
                tags,
                due,
            };
            let item: ItemView = client.post("/items", &req)?;
            print!("{}", style().lines(&[item]));
        }
        Command::Done { item, force } => {
            let patch = PatchItem {
                status: Some("done".into()),
                force,
                ..PatchItem::default()
            };
            let item: ItemView = client.patch(&item_path(&item.join(" ")), &patch)?;
            print!("{}", style().lines(&[item]));
        }
        Command::Mv { item, path } => {
            let patch = PatchItem {
                path: Some(path),
                ..PatchItem::default()
            };
            let item: ItemView = client.patch(&item_path(&item), &patch)?;
            print!("{}", style().lines(&[item]));
        }
        Command::Set {
            item,
            status,
            body,
            tags,
            no_tags,
            due,
            no_due,
            archive,
            unarchive,
            force,
        } => {
            let patch = PatchItem {
                path: None,
                body,
                status,
                tags: (no_tags || !tags.is_empty()).then_some(tags),
                archived: (archive || unarchive).then_some(archive),
                due: if no_due { Some(None) } else { due.map(Some) },
                force,
            };
            if patch
                == (PatchItem {
                    force,
                    ..PatchItem::default()
                })
            {
                return Err(tack_core::Error::invalid(
                    "nothing to change, see tack set --help",
                ));
            }
            let item: ItemView = client.patch(&item_path(&item.join(" ")), &patch)?;
            print!("{}", style().lines(&[item]));
        }
        Command::Rm { item, recursive } => {
            let recursive = if recursive { "true" } else { "false" };
            let _: Deleted =
                client.delete(&item_path(&item.join(" ")), &[("recursive", recursive)])?;
        }
    }
    Ok(())
}

struct Style {
    categories: Option<HashMap<String, Category>>,
}

impl Style {
    fn new(client: &Client) -> Style {
        let color = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        let categories = color
            .then(|| client.get::<StatusList>("/statuses", &[]).ok())
            .flatten()
            .map(|list| {
                list.statuses
                    .into_iter()
                    .map(|s| (s.name, s.category))
                    .collect()
            });
        Style { categories }
    }

    fn status(&self, name: &str, width: usize) -> String {
        let padded = format!("{name:<width$}");
        let code = match self.categories.as_ref().and_then(|c| c.get(name)) {
            Some(Category::Todo) => "34",
            Some(Category::Active) => "33",
            Some(Category::Done) => "32",
            None => return padded,
        };
        format!("\x1b[{code}m{padded}\x1b[0m")
    }

    fn lines(&self, items: &[ItemView]) -> String {
        let extras: Vec<String> = items.iter().map(extras).collect();
        let status_width = items.iter().map(|i| i.status.len()).max().unwrap_or(0);
        let path_width = items
            .iter()
            .zip(&extras)
            .filter(|(_, e)| !e.is_empty())
            .map(|(i, _)| i.path.len())
            .max()
            .unwrap_or(0);
        let mut out = String::new();
        for (item, extra) in items.iter().zip(extras) {
            out.push_str(&self.status(&item.status, status_width));
            out.push_str("  ");
            if extra.is_empty() {
                out.push_str(&item.path);
            } else {
                out.push_str(&format!("{:<path_width$}  {extra}", item.path));
            }
            out.push('\n');
        }
        out
    }

    fn detail(&self, item: &ItemView) -> String {
        let mut out = format!("{}\n", item.path);
        let mut field = |name: &str, value: String| out.push_str(&format!("{name:<9}{value}\n"));
        field("status", self.status(&item.status, 0));
        if !item.tags.is_empty() {
            field("tags", tags(item));
        }
        if let Some(due) = item.due {
            field("due", due.to_string());
        }
        field("created", local(item.created_at));
        field("updated", local(item.updated_at));
        if item.archived {
            let at = item
                .archived_at
                .map_or_else(|| "by a parent".to_owned(), local);
            field("archived", at);
        }
        if !item.body.is_empty() {
            out.push('\n');
            out.push_str(item.body.trim_end());
            out.push('\n');
        }
        out
    }
}

fn tags(item: &ItemView) -> String {
    item.tags
        .iter()
        .map(|t| format!("+{t}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn extras(item: &ItemView) -> String {
    let mut parts = Vec::new();
    if !item.tags.is_empty() {
        parts.push(tags(item));
    }
    if let Some(due) = item.due {
        parts.push(due.to_string());
    }
    if item.archived {
        parts.push("archived".to_owned());
    }
    parts.join("  ")
}

fn local(at: Timestamp) -> String {
    at.to_zoned(TimeZone::system())
        .strftime("%Y-%m-%d %H:%M")
        .to_string()
}

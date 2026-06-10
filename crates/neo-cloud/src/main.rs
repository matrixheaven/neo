use std::{net::TcpListener, path::PathBuf};

use clap::Parser;
use neo_cloud::{CloudServer, Store};

#[derive(Debug, Parser)]
#[command(name = "neo-cloud", about = "Self-hosted Neo cloud server")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: String,
    #[arg(long, default_value = "~/.neo/cloud.sqlite")]
    database: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let database = expand_user_path(args.database);
    let store = Store::open(database).await?;
    let listener = TcpListener::bind(&args.listen)?;
    println!("neo-cloud listening on http://{}", listener.local_addr()?);
    CloudServer::new(store).serve(listener).await
}

fn expand_user_path(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str().map(str::to_owned) else {
        return path;
    };
    if raw == "~" {
        return home_dir().unwrap_or(path);
    }
    let Some(rest) = raw.strip_prefix("~/").map(str::to_owned) else {
        return path;
    };
    home_dir().map_or(path, |home| home.join(rest))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

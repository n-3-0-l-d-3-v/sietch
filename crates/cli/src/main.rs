//! `vaultc` — command-line access to the Vault's PUT/GET/DELETE/SCAN
//! primitives, and a recovery report for inspecting crash behavior
//! directly instead of trusting it blindly.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use storage::Store;

#[derive(Parser)]
#[command(name = "vaultc", about = "The Impossible Vault CLI", version)]
struct Cli {
    /// Directory holding the vault's segment files.
    #[arg(long, default_value = "./vault-data")]
    dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Put {
        key: String,
        value: String,
    },
    Get {
        key: String,
    },
    Delete {
        key: String,
    },
    Scan {
        prefix: String,
    },
    /// Opens the vault and reports whether recovery had to discard a torn
    /// tail from an unclean shutdown.
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut store =
        Store::open(&cli.dir).with_context(|| format!("opening vault at {}", cli.dir.display()))?;

    match cli.command {
        Command::Put { key, value } => {
            store.put(key, value)?;
            println!("ok");
        }
        Command::Get { key } => match store.get(key.as_bytes()) {
            Some(v) => println!("{}", String::from_utf8_lossy(&v)),
            None => println!("(not found)"),
        },
        Command::Delete { key } => {
            store.delete(key)?;
            println!("ok");
        }
        Command::Scan { prefix } => {
            for (k, v) in store.scan(prefix.as_bytes()) {
                println!(
                    "{} = {}",
                    String::from_utf8_lossy(&k),
                    String::from_utf8_lossy(&v)
                );
            }
        }
        Command::Status => {
            println!(
                "recovered_from_torn_tail: {}",
                store.recovered_from_torn_tail
            );
        }
    }
    Ok(())
}

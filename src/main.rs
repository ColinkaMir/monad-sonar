// monad-sonar — read the Monad validator peer set without running a node.
mod harness;
// Copyright (C) 2026 ProofLine. Licensed under GPL-3.0 (built on category-labs/monad-bft).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Which Monad network to discover peers on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Network {
    Testnet,
    Mainnet,
}

impl Network {
    /// Chain id per network (testnet=10143, mainnet=143).
    fn chain_id(self) -> u64 {
        match self {
            Network::Testnet => 10143,
            Network::Mainnet => 143,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "monad-sonar", version, about = "Read the Monad validator peer set without running a node.")]
struct Cli {
    /// Network to discover on.
    #[arg(long, value_enum, default_value_t = Network::Testnet, global = true)]
    network: Network,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Discover the active-set peer records and print/write them.
    Peers {
        /// Path to the node config TOML (network / peer_discovery / bootstrap).
        #[arg(long)]
        config: PathBuf,
        /// Write JSON to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Keep running and refresh the peer set on this interval (seconds); one-shot if unset.
        #[arg(long)]
        watch: Option<u64>,
        /// How long to run discovery before dumping (seconds).
        #[arg(long, default_value_t = 45)]
        run_secs: u64,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Peers { config, out, watch, run_secs } => {
            tracing::info!(network = ?cli.network, chain_id = cli.network.chain_id(), "monad-sonar");
            harness::run_peers(&config, out, watch, run_secs).await?;
        }
    }
    Ok(())
}

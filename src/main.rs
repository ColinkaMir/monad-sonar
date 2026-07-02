// monad-sonar — read the Monad validator peer set without running a node.
mod harness;
mod rpc;
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

    /// Default public JSON-RPC per network (used to read the active set node-independently).
    fn default_rpc(self) -> &'static str {
        match self {
            Network::Testnet => "https://testnet-rpc.monad.xyz",
            Network::Mainnet => "https://rpc.monad.xyz",
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
        /// JSON-RPC endpoint used to read the active validator set + epoch (defaults per --network).
        /// If a `validators.toml` sits next to --config it is used instead (offline fallback).
        #[arg(long)]
        rpc: Option<String>,
        /// Public IPv4 this crawler advertises in its own name record. It MUST match the source IP
        /// of our packets or peers reject us (auth-UDP proves IP ownership) and discovery returns
        /// nothing. Auto-detected if unset; override here when auto-detect is wrong (NAT, multi-homed).
        #[arg(long)]
        public_ip: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Peers { config, out, watch, run_secs, rpc, public_ip } => {
            let rpc_url = rpc.unwrap_or_else(|| cli.network.default_rpc().to_string());
            tracing::info!(network = ?cli.network, chain_id = cli.network.chain_id(), %rpc_url, "monad-sonar");
            harness::run_peers(&config, out, watch, run_secs, &rpc_url, public_ip).await?;
        }
    }
    Ok(())
}

// Discovery harness: a stripped port of monad-node's build_raptorcast_router
// (monad-node/src/main.rs) that stands up ONLY the networking + peer-discovery stack.
// It runs the MultiRouter (which self-persists discovered peers to persisted_peers_path),
// drops the consensus/execution wiring, and drops the self-name-record-sig assert (we
// generate a fresh identity and sign our own record).
//
// Copyright (C) 2026 ProofLine. GPL-3.0 (built on category-labs/monad-bft).

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::NonZeroU16,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use monad_crypto::certificate_signature::CertificateSignaturePubKey;
use monad_dataplane::{DataplaneBuilder, TcpSocketId, UdpSocketId};
use monad_node_config::{
    ExecutionProtocolType, NodeConfig, SignatureCollectionType, SignatureType,
};
use monad_peer_discovery::{
    discovery::{PeerDiscovery, PeerDiscoveryBuilder},
    MonadNameRecord, NameRecord,
};
use monad_peer_score::{ema, StdClock};
use monad_raptorcast::{
    auth::WireAuthProtocol,
    config::{RaptorCastConfig, RaptorCastConfigPrimary},
};
use monad_router_multi::MultiRouter;
use monad_types::{Epoch, NodeId, Round};
use monad_validator::{
    proposer_schedule::{BoxedProposerSchedule, ElectedProposerSchedule},
    weighted_round_robin::WeightedRoundRobin,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

type ST = SignatureType;
type SCT = SignatureCollectionType;
type EPT = ExecutionProtocolType;
type MonadMsg = monad_state::MonadMessage<ST, SCT, EPT>;
type VerifiedMonadMsg = monad_state::VerifiedMonadMessage<ST, SCT, EPT>;
type ScoreReader = ema::ScoreReader<NodeId<CertificateSignaturePubKey<ST>>, StdClock>;

/// Build the discovery-only router. `node_config` supplies network/peer-discovery/bootstrap
/// config; `identity` is a freshly generated keypair; `epoch_validators` is the target set.
#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_discovery_router(
    node_config: NodeConfig<ST>,
    identity: monad_secp::KeyPair,
    epoch_validator_map: BTreeMap<Epoch, BTreeSet<NodeId<CertificateSignaturePubKey<ST>>>>,
    current_epoch: Epoch,
    current_round: Round,
    advertised_ip: Option<Ipv4Addr>,
    persisted_peers_path: PathBuf,
) -> MultiRouter<ST, MonadMsg, VerifiedMonadMsg, monad_executor_glue::MonadEvent<ST, SCT, EPT>, PeerDiscovery<ST>, WireAuthProtocol, ScoreReader> {
    let peer_discovery_config = node_config.peer_discovery.clone();
    let bootstrap_nodes = node_config.bootstrap.clone();
    let network_config = node_config.network.clone();

    let leader_election: WeightedRoundRobin<_> = WeightedRoundRobin::default();
    let proposer_schedule: BoxedProposerSchedule<CertificateSignaturePubKey<ST>> =
        Box::new(ElectedProposerSchedule::new(leader_election));
    let (_score_provider, score_reader) =
        ema::create::<NodeId<CertificateSignaturePubKey<ST>>, StdClock>(
            node_config.txpool_peer_score.clone(),
            StdClock,
        );

    let self_udp_port = peer_discovery_config.udp_port();
    let authenticated_bind_address = SocketAddr::new(
        IpAddr::V4(network_config.bind_address_host),
        network_config.authenticated_bind_address_port,
    );
    // Discovery-crawler override: do NOT stand up a non-authenticated UDP socket. With one
    // present, write_to_name_record takes the NonAuthenticatedFallback path for the FIRST send to
    // each peer (socket.rs:158) — it sprays the discovery ping UNAUTHENTICATED to the peer's
    // non-auth udp port (which live testnet peers don't answer) while the auth session establishes
    // empty (buffered_messages=0). Forcing None makes every ping go over the authenticated session.
    let non_authenticated_bind_address: Option<SocketAddr> = None;
    let tcp_bind_address = SocketAddr::new(
        IpAddr::V4(network_config.bind_address_host),
        network_config.bind_address_tcp_port.unwrap_or(0),
    );

    let self_id = NodeId::new(identity.pubkey());
    let self_tcp_port = peer_discovery_config.tcp_port();
    // The advertised IP MUST equal our packet source IP: auth-UDP proves IP ownership, so peers
    // silently drop us (0 pongs) if the name record claims an address we don't send from.
    let self_ip = advertised_ip
        .unwrap_or_else(|| peer_discovery_config.ip().expect("self endpoint must be an IP"));

    let self_record = NameRecord::new_with_ports(
        self_ip,
        self_tcp_port.get(),
        self_udp_port.map(NonZeroU16::get),
        peer_discovery_config.self_auth_port.get(),
        peer_discovery_config.self_direct_udp_port.map(NonZeroU16::get),
        peer_discovery_config.self_record_seq_num,
    );
    let self_record = MonadNameRecord::new(self_record, &identity);

    let mut dp_builder = DataplaneBuilder::new(network_config.max_mbps.into())
        .with_udp_multishot(network_config.enable_udp_multishot);
    let mut udp_sockets: Vec<(UdpSocketId, SocketAddr)> =
        vec![(UdpSocketId::AuthenticatedRaptorcast, authenticated_bind_address)];
    if let Some(address) = non_authenticated_bind_address {
        udp_sockets.push((UdpSocketId::Raptorcast, address));
    }
    dp_builder = dp_builder
        .with_udp_sockets(udp_sockets)
        .with_tcp_sockets([(TcpSocketId::Raptorcast, tcp_bind_address)]);

    // bootstrap peers from config
    let bootstrap_peers: BTreeMap<_, _> = bootstrap_nodes
        .peers
        .iter()
        .filter_map(|peer| {
            let node_id = NodeId::new(peer.secp256k1_pubkey);
            if node_id == self_id {
                return None;
            }
            match MonadNameRecord::try_from(peer) {
                Ok(rec) => Some((node_id, rec)),
                Err(_) => None,
            }
        })
        .collect();

    let pinned_full_nodes: BTreeSet<_> = bootstrap_peers.keys().cloned().collect();

    let peer_discovery_builder = PeerDiscoveryBuilder {
        self_id,
        self_record,
        current_round,
        current_epoch,
        epoch_validators: epoch_validator_map.clone(),
        pinned_full_nodes,
        prioritized_full_nodes: BTreeSet::new(),
        bootstrap_peers,
        refresh_period: Duration::from_secs(peer_discovery_config.refresh_period),
        request_timeout: Duration::from_secs(peer_discovery_config.request_timeout),
        unresponsive_prune_threshold: peer_discovery_config.unresponsive_prune_threshold,
        last_participation_prune_threshold: peer_discovery_config.last_participation_prune_threshold,
        min_num_peers: peer_discovery_config.min_num_peers,
        max_num_peers: peer_discovery_config.max_num_peers,
        max_group_size: node_config.fullnode_raptorcast.max_group_size,
        enable_publisher: node_config.fullnode_raptorcast.enable_publisher,
        enable_client: node_config.fullnode_raptorcast.enable_client,
        rng: ChaCha8Rng::from_entropy(),
        persisted_peers_path,
        ping_rate_limit_per_second: peer_discovery_config.ping_rate_limit_per_second,
    };

    let shared_key = Arc::new(identity);
    let wireauth_config = monad_wireauth::Config::default();
    let auth_protocol = WireAuthProtocol::new(
        &monad_raptorcast::auth::metrics::UDP_METRICS,
        wireauth_config,
        shared_key.clone(),
    );

    MultiRouter::new(
        self_id,
        RaptorCastConfig {
            shared_key,
            mtu: network_config.mtu,
            udp_message_max_age_ms: network_config.udp_message_max_age_ms,
            sig_verification_rate_limit: network_config.signature_verifications_per_second,
            primary_instance: RaptorCastConfigPrimary {
                raptor10_redundancy: 2.5f32,
                fullnode_dedicated: Vec::new(),
            },
            secondary_instance: node_config.fullnode_raptorcast.clone(),
            deterministic_protocol_rollout: node_config.deterministic_raptorcast_rollout,
        },
        dp_builder,
        peer_discovery_builder,
        current_epoch,
        epoch_validator_map,
        auth_protocol,
        None,
        score_reader,
        proposer_schedule,
    )
}

use std::path::Path;
use futures::StreamExt;
use monad_crypto::certificate_signature::PubKey;
use monad_node_config::MonadNodeConfig;
use monad_secp::KeyPair;

/// Parse a validators.toml-format file (the node emits one per epoch:
/// `[[validator_sets]]` with `epoch` + `[[validator_sets.validators]]` with `node_id`) into the
/// epoch -> {NodeId} map peer-discovery uses as PeerLookup targets. Keeps only `current_epoch`
/// (and any later sets present), which is what discovery actively looks up.
type ValidatorMap = BTreeMap<Epoch, BTreeSet<NodeId<CertificateSignaturePubKey<ST>>>>;

fn node_id_from_hex(secp_hex: &str) -> Result<NodeId<CertificateSignaturePubKey<ST>>, Box<dyn std::error::Error>> {
    let bytes = hex::decode(secp_hex.trim_start_matches("0x"))?;
    let pk = <CertificateSignaturePubKey<ST> as PubKey>::from_bytes(&bytes)
        .map_err(|e| format!("bad validator secp {secp_hex}: {e:?}"))?;
    Ok(NodeId::new(pk))
}

/// OFFLINE fallback: read the active set from a validators.toml snapshot ([[validator_sets]] with
/// `epoch` + [[validator_sets.validators]] with `node_id`). Returns the map plus the newest epoch
/// found (used as current_epoch).
fn load_epoch_validators_file(
    path: &Path,
) -> Result<(Epoch, ValidatorMap), Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    struct File {
        validator_sets: Vec<Set>,
    }
    #[derive(serde::Deserialize)]
    struct Set {
        epoch: u64,
        validators: Vec<Val>,
    }
    #[derive(serde::Deserialize)]
    struct Val {
        node_id: String,
    }

    let file: File = toml::from_str(&std::fs::read_to_string(path)?)?;
    let mut map: ValidatorMap = BTreeMap::new();
    let mut newest = 0u64;
    for set in file.validator_sets {
        newest = newest.max(set.epoch);
        let mut ids = BTreeSet::new();
        for v in set.validators {
            ids.insert(node_id_from_hex(&v.node_id)?);
        }
        map.insert(Epoch(set.epoch), ids);
    }
    Ok((Epoch(newest), map))
}

/// PRIMARY (node-independent): read the current epoch, round proxy, and the active consensus set's
/// secp node ids straight from the staking precompile over public RPC. One getValidator call per
/// validator, so this takes a few seconds for a full set.
fn fetch_epoch_validators_rpc(
    rpc_url: &str,
) -> Result<(Epoch, Round, ValidatorMap), Box<dyn std::error::Error>> {
    let rpc = crate::rpc::Rpc::new(rpc_url);
    let epoch = rpc.current_epoch()?;
    let round = rpc.round_proxy()?;
    let ids = rpc.consensus_validator_ids()?;
    tracing::info!(epoch, round, validators = ids.len(), "monad-sonar: RPC active set, fetching secp keys");
    let mut set = BTreeSet::new();
    for id in &ids {
        match rpc.validator_secp(*id) {
            Ok(bytes) => {
                set.insert(node_id_from_hex(&hex::encode(bytes))?);
            }
            Err(e) => tracing::warn!(id, error = %e, "skipping validator (secp fetch failed)"),
        }
    }
    let mut map: ValidatorMap = BTreeMap::new();
    map.insert(Epoch(epoch), set);
    Ok((Epoch(epoch), Round(round), map))
}

/// Load a persistent identity from `path`, or create one (32-byte secp secret, 0600) on first use.
/// Reusing one identity across runs keeps peers from seeing a flood of fresh identities from our IP
/// (which trips their anti-DoS and gets us throttled to zero pongs).
fn load_or_create_identity(path: &Path) -> Result<KeyPair, Box<dyn std::error::Error>> {
    if path.exists() {
        let mut bytes = std::fs::read(path)?;
        if bytes.len() != 32 {
            return Err(format!("identity {path:?}: expected 32 bytes, got {}", bytes.len()).into());
        }
        let kp = KeyPair::from_bytes(&mut bytes)?;
        tracing::info!(?path, "monad-sonar: loaded persistent identity");
        Ok(kp)
    } else {
        let mut secret: [u8; 32] = rand::random();
        std::fs::write(path, secret)?; // write before from_bytes (which may consume the buffer)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        let kp = KeyPair::from_bytes(&mut secret)?;
        tracing::info!(?path, "monad-sonar: created persistent identity");
        Ok(kp)
    }
}

/// Best-effort public-IP discovery via a plain HTTP echo service. Returns None on any failure
/// (offline, service down) — the caller then falls back to the config's advertised address.
fn detect_public_ip() -> Option<Ipv4Addr> {
    let ip = ureq::get("https://api.ipify.org")
        .call()
        .ok()?
        .into_string()
        .ok()?;
    ip.trim().parse().ok()
}

/// Run discovery: load config, generate a throwaway identity, drive the router's stream so
/// peer-discovery runs, then read the peers.toml the router persisted.
pub async fn run_peers(
    config_path: &Path,
    out: Option<PathBuf>,
    _watch: Option<u64>,
    run_secs: u64,
    rpc_url: &str,
    public_ip: Option<String>,
    identity_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let config: MonadNodeConfig = toml::from_str(&std::fs::read_to_string(config_path)?)?;
    let identity = load_or_create_identity(identity_path)?;

    // Resolve the IP we advertise: explicit flag > auto-detected public IP > whatever the config
    // says. It must match our real source IP or peers reject us (auth-UDP IP-ownership check).
    let advertised_ip: Option<Ipv4Addr> = match public_ip {
        Some(s) => Some(s.parse().map_err(|e| format!("bad --public-ip {s}: {e}"))?),
        None => detect_public_ip(),
    };
    if let Some(ip) = advertised_ip {
        tracing::info!(%ip, "monad-sonar: advertising this public IP (must match our source IP)");
    }

    let persisted = std::env::temp_dir().join("monad-sonar-peers.toml");
    let _ = std::fs::remove_file(&persisted);

    // PeerLookup targets = the active validator set; discovery also needs the current epoch (round
    // is not load-bearing for discovery). Source of truth: the live chain via public RPC
    // (node-independent). Offline fallback: a `validators.toml` snapshot next to --config.
    let validators_file = config_path.with_file_name("validators.toml");
    let (current_epoch, current_round, epoch_validator_map) = if validators_file.exists() {
        let (epoch, map) = load_epoch_validators_file(&validators_file)?;
        tracing::info!(?validators_file, epoch = epoch.0, "monad-sonar: active set from local snapshot (offline)");
        (epoch, Round(epoch.0), map)
    } else {
        fetch_epoch_validators_rpc(rpc_url)?
    };
    tracing::info!(
        epoch = current_epoch.0,
        validators = epoch_validator_map.get(&current_epoch).map(|s| s.len()).unwrap_or(0),
        "monad-sonar: loaded active validator set"
    );

    tracing::info!(?persisted, run_secs, "monad-sonar: starting discovery");
    let router = build_discovery_router(
        config,
        identity,
        epoch_validator_map,
        current_epoch,
        current_round,
        advertised_ip,
        persisted.clone(),
    );
    let mut router = Box::pin(router);

    // The router's persisted file mirrors its ACTIVE routing table (bounded + pruned), so it holds
    // only a live working set at any instant, not the cumulative discovery. A crawler wants the
    // UNION of every name record seen over the run, so we poll+merge the file as discovery churns.
    #[derive(serde::Deserialize)]
    struct Persisted {
        #[serde(default)]
        peers: Vec<PersistedPeer>,
    }
    #[derive(serde::Deserialize)]
    struct PersistedPeer {
        address: String,
        tcp_port: u16,
        auth_port: u16,
        record_seq_num: u64,
        secp256k1_pubkey: String,
    }
    let mut seen: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let merge = |seen: &mut BTreeMap<String, serde_json::Value>| {
        let text = std::fs::read_to_string(&persisted).unwrap_or_default();
        if let Ok(parsed) = toml::from_str::<Persisted>(&text) {
            for p in parsed.peers {
                // keep the highest record_seq_num per peer (freshest name record)
                let keep = seen
                    .get(&p.secp256k1_pubkey)
                    .and_then(|v| v.get("seq").and_then(|s| s.as_u64()))
                    .is_none_or(|prev| p.record_seq_num >= prev);
                if keep {
                    seen.insert(
                        p.secp256k1_pubkey.clone(),
                        serde_json::json!({
                            "secp": p.secp256k1_pubkey,
                            "ip": p.address,
                            "port": p.tcp_port,
                            "authPort": p.auth_port,
                            "seq": p.record_seq_num,
                        }),
                    );
                }
            }
        }
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(run_secs);
    let mut poll = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = poll.tick() => merge(&mut seen),
            ev = router.next() => { if ev.is_none() { break; } }
        }
    }
    merge(&mut seen); // final sweep

    let records: Vec<serde_json::Value> = seen.into_values().collect();
    let json = serde_json::to_string_pretty(&records)?;

    match out {
        Some(path) => {
            std::fs::write(&path, &json)?;
            println!(
                "monad-sonar: discovered {} peers -> {}",
                records.len(),
                path.display()
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}

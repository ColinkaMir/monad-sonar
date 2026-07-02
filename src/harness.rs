// Discovery harness: a stripped port of monad-node's build_raptorcast_router
// (monad-node/src/main.rs) that stands up ONLY the networking + peer-discovery stack.
// It runs the MultiRouter (which self-persists discovered peers to persisted_peers_path),
// drops the consensus/execution wiring, and drops the self-name-record-sig assert (we
// generate a fresh identity and sign our own record).
//
// Copyright (C) 2026 ProofLine. GPL-3.0 (built on category-labs/monad-bft).

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
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
    let self_ip = peer_discovery_config.ip().expect("self endpoint must be an IP");

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
fn load_epoch_validators(
    path: &Path,
    current_epoch: Epoch,
) -> Result<BTreeMap<Epoch, BTreeSet<NodeId<CertificateSignaturePubKey<ST>>>>, Box<dyn std::error::Error>>
{
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

    let mut map: BTreeMap<Epoch, BTreeSet<NodeId<CertificateSignaturePubKey<ST>>>> = BTreeMap::new();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?path, ?e, "no validators file; PeerLookup disabled (bootstrap only)");
            return Ok(map);
        }
    };
    let file: File = toml::from_str(&text)?;
    for set in file.validator_sets {
        if set.epoch < current_epoch.0 {
            continue;
        }
        let mut ids = BTreeSet::new();
        for v in set.validators {
            let bytes = hex::decode(v.node_id.trim_start_matches("0x"))?;
            let pk = <CertificateSignaturePubKey<ST> as PubKey>::from_bytes(&bytes)
                .map_err(|e| format!("bad validator node_id {}: {e:?}", v.node_id))?;
            ids.insert(NodeId::new(pk));
        }
        map.insert(Epoch(set.epoch), ids);
    }
    Ok(map)
}

/// Run discovery: load config, generate a throwaway identity, drive the router's stream so
/// peer-discovery runs, then read the peers.toml the router persisted.
pub async fn run_peers(
    config_path: &Path,
    out: Option<PathBuf>,
    _watch: Option<u64>,
    run_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let config: MonadNodeConfig = toml::from_str(&std::fs::read_to_string(config_path)?)?;
    let mut secret: [u8; 32] = rand::random();
    let identity = KeyPair::from_bytes(&mut secret)?;

    let persisted = std::env::temp_dir().join("monad-sonar-peers.toml");
    let _ = std::fs::remove_file(&persisted);

    // TODO(product): epoch/round are hardcoded placeholders (a recent testnet snapshot) and WILL
    // go stale as the epoch advances. Source them from a reference RPC (consensus/getValidator).
    let current_epoch = Epoch(839);
    let current_round = Round(42316863);

    // PeerLookup targets = the active validator set. Load NodeIds from a validators.toml-format
    // file (sibling of the node config, or supplied out-of-band). Without these the crawler can
    // only maintain bootstrap peers; with them it issues targeted PeerLookups and expands.
    let validators_path = config_path.with_file_name("validators.toml");
    let epoch_validator_map = load_epoch_validators(&validators_path, current_epoch)?;
    tracing::info!(
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

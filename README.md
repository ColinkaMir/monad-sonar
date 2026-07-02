# monad-sonar

**Read the Monad validator peer set without running a node.**

`monad-sonar` speaks Monad's peer-discovery protocol directly, so any builder can fetch
validator **name records** (endpoints) from a laptop, a small VPS, or CI — never on a host
that runs a Monad node.

> Status: **working v1 (testnet).** Authenticated discovery + peer-lookup run live and export
> JSON; a 100s testnet pass discovers ~195 peer name records. Mainnet bootstrap + hosted feeds next.

## Why

Reading the peer set normally means running a full node: a dedicated NVMe TrieDB device,
hundreds of GB of state sync, a heavy box, and third-party code sitting on your validator
host. `monad-sonar` does **only discovery** — light, isolated, safe.

- **No node required** — no raw TrieDB device, no state sync, no heavy box.
- **Off the validator** — runs anywhere; never on a signing host (in line with the
  Foundation's guidance to keep third-party code off the validator host).
- **Lightweight & fast** — starts in seconds, tiny footprint.
- **Protocol-faithful** — speaks the real, documented Monad discovery protocol
  (reusing the official `monad-bft` networking crates), not a scrape or a hack.
- **Composable** — a CLI and a library; a building block for explorers, dashboards,
  monitors, and observed-data layers.
- **Standard output** — the same name records the protocol defines, as JSON.

## Build

```
cargo build --release        # Cargo fetches category-labs/monad-bft (pinned) and its submodules
```

No local checkout of `monad-bft` is required — the dependency is a pinned git rev. (Only the
networking/discovery crates compile; the C++ execution engine is never built.)

> **Heads-up on the first build:** because the crates live in the large `monad-bft` workspace,
> Cargo fetches that whole repository and its submodules — on the order of **~4–5 GB** the first
> time (cached afterwards). None of the heavy C++ `monad-execution` submodule is compiled; it is
> only pulled during dependency resolution. A leaner packaging (just the networking crates) is on
> the roadmap.

## Usage

`monad-sonar` needs two inputs next to each other:

- a **node-style config** (`--config`) describing the bind ports and a handful of bootstrap peers
  (see `configs/testnet.toml`), and
- a **`validators.toml`** sibling of that config listing the current active-set node ids — the
  PeerLookup targets. A snapshot is provided; copy it (or, later, fetch it from RPC):

```
cp configs/validators.example.toml configs/validators.toml

# discover the testnet peer set for ~100s and write JSON
monad-sonar --network testnet peers \
  --config configs/testnet.toml --out peers.json --run-secs 100
```

Both **testnet** and **mainnet** are first-class (`--network`); a mainnet bootstrap seed is on the
roadmap.

### Output

```json
[
  { "secp": "0x0203a26b...", "ip": "149.50.110.123", "port": 8000, "authPort": 8001, "seq": 2 }
]
```

A 100s testnet pass typically discovers ~200 peer name records.

## How it works

`monad-sonar` implements the documented discovery protocol (ping/pong + peer-lookup over an
authenticated UDP transport, wrapped in RaptorCast), reusing the official
[`category-labs/monad-bft`](https://github.com/category-labs/monad-bft) crates for the wire
and cryptography. It bootstraps from a few known peers, discovers the active validator set's
name records, and exports them.

## Hosted feeds

Beyond self-hosting the CLI, ProofLine runs **public, continuously-updating feeds** you can
pull from directly (testnet and mainnet), hosted separately from any Monad node host. Links
to follow.

## Roadmap

- **v1** — the peer set (name records) for testnet and mainnet.
- Next — provider/ASN/region enrichment, liveness, latency from the vantage, diff/history,
  a hosted API.

## License

GPL-3.0. `monad-sonar` builds on `category-labs/monad-bft` (GPL-3.0). Credit: Category Labs /
Monad for the protocol and the crates.

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
cargo build --release        # light: only the ~15 networking crates + crates.io deps
```

The handful of `category-labs/monad-bft` crates this needs (plus the `category-labs/monoio` fork)
are **vendored** under `vendor/` (~7 MB), so building does **not** clone the large `monad-bft`
monorepo or its submodules — no ~4–5 GB pull of the C++ `monad-execution` engine and ethereum test
vectors we never compile. Everything else comes from crates.io as usual. See `.cargo/config.toml`
for the source mapping; to build against upstream instead, remove that file.

## Usage

All it needs is a **node-style config** (`--config`) describing the bind ports and a handful of
bootstrap peers (see `configs/testnet.toml`):

```
# discover the testnet peer set for ~100s and write JSON
monad-sonar --network testnet peers \
  --config configs/testnet.toml --out peers.json --run-secs 100
```

The active validator set (the PeerLookup targets) and the current epoch are read **live from the
public JSON-RPC** — so no node and no local snapshot are required. Override with `--rpc <url>`; for
offline use, drop a `validators.toml` next to `--config` (see `configs/validators.example.toml`) and
that snapshot is used instead.

The crawler **auto-detects its public IP** and advertises it in its own name record. This IP must
match the source IP of its packets or peers reject it (auth-UDP proves IP ownership) and discovery
returns nothing — so behind NAT / on a multi-homed host, set it explicitly with `--public-ip <ip>`.

Both **testnet** and **mainnet** are first-class (`--network`), each with a ready config
(`configs/testnet.toml`, `configs/mainnet.toml`):

```
# mainnet
monad-sonar --network mainnet peers \
  --config configs/mainnet.toml --out mainnet-peers.json --run-secs 100
```

A ~70s mainnet pass discovers ~120 peer name records from the 8 seed peers.

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

Beyond self-hosting the CLI, Prooflines runs **public, continuously-updating feeds** you can
pull from directly (testnet and mainnet), hosted separately from any Monad node host. Links
to follow.

## Roadmap

- **v1** — the peer set (name records) for testnet and mainnet.
- Next — provider/ASN/region enrichment, liveness, latency from the vantage, diff/history,
  a hosted API.

## License

GPL-3.0. `monad-sonar` builds on `category-labs/monad-bft` (GPL-3.0). Credit: Category Labs /
Monad for the protocol and the crates.

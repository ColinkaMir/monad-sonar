# Contributing to monad-sonar

`monad-sonar` reads the Monad validator peer set by speaking the discovery
protocol directly, off any node. It is the engine behind the hosted feeds at
[prooflines.org/monad/sonar](https://prooflines.org/monad/sonar/).

## Building and testing

```bash
cargo build --release     # vendored monad-bft crates, no 4.5 GB monorepo clone
cargo test --release      # unit tests (protocol parsing); required before a PR
```

The `category-labs/monad-bft` crates this depends on are vendored under
`vendor/` (~6 MB), so a clone builds without pulling the monorepo or its C++
submodules. crates.io dependencies fetch normally. See `.cargo/config.toml`.

## Running a real discovery pass

```bash
cargo run --release -- --network testnet peers \
  --config configs/testnet.toml --out peers.json --run-secs 100
```

Needs only a node-style config (bind ports + a few bootstrap peers, see
`configs/testnet.toml`). Never run it on a host that also runs a Monad node,
that is the whole point: discovery stays off the validator.

## What a change needs

- `cargo build --release` and `cargo test --release` must pass before a PR.
- New protocol-facing logic needs a unit test. Parsing and record handling are
  pure and testable without the network (see the tests in `src/harness.rs`);
  keep new logic that way where you can.
- Keep the crate faithful to the real discovery protocol via the official
  `monad-bft` crates, not a scrape or a reimplementation.
- The identity secret (`configs/*identity*.key`) is gitignored and MUST stay
  out of the repo.
- One change per PR.

## Good places to start

- Additional output formats (CSV, prometheus text) alongside the JSON export.
- Multi-vantage RTT annotation on discovered records.
- A `--once` mode that exits after the first full sweep instead of `--run-secs`.
- Richer name-record fields in the JSON (ports, seq, auth status) behind a flag.

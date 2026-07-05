# tools/ — sonar app-level RTT

`sonar-rtt.sh` + `sonar_rtt.py` turn a monad-sonar discovery crawl into per-validator round-trip
latency measured from the auth-UDP **ping/pong** the crawler already exchanges — so validators that
answer peer discovery but firewall ICMP still get an RTT (which a plain ICMP map would miss).

Method: run sonar with `RUST_LOG=monad_peer_discovery=debug`, match each pong to the most recent
preceding ping for that secp. sonar's ping timestamp is the command-emit time (ahead of the send
queue), so raw values are inflated by a near-constant factor; given an ICMP reference the parser
self-calibrates (ratio = median(sonar/ICMP) over validators reachable by both this run; observed
~2.07, IQR ~4%), yielding ICMP-comparable ms. Vantage = wherever it runs.

#!/usr/bin/env python3
"""monad-sonar app-level RTT: parse a debug crawl log (RUST_LOG=monad_peer_discovery=debug)
into per-validator round-trip latency from the SAME vantage sonar ran on, by matching each
pong to the most recent preceding ping for that secp.

sonar's ping timestamp is the command-emit time (before the dataplane send queue), so the raw
value is inflated by a near-constant factor. If an ICMP reference (ip->rtt) + secp->ip map is
supplied, we self-calibrate: ratio = median(sonar/ICMP over dual-reachable this run) and emit a
calibrated, ICMP-comparable RTT. The win: validators that answer auth-UDP but BLOCK ICMP get an
RTT they'd otherwise be missing on the geo map.

Usage: sonar_rtt.py LOG OUT [ICMP_AVERAGED_JSON VALIDATORS_WITH_IPS_JSON]
"""
import sys, re, json, statistics
from datetime import datetime

log, out = sys.argv[1], sys.argv[2]
icmp_f = sys.argv[3] if len(sys.argv) > 3 else None
vip_f  = sys.argv[4] if len(sys.argv) > 4 else None

ts_re   = re.compile(r'^(\d{4}-\d{2}-\d{2}T[\d:.]+Z)')
ping_re = re.compile(r'sending ping request to=([0-9a-f]{66})')
pong_re = re.compile(r'handling pong response from=([0-9a-f]{66})')
t = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()

last, rtts = {}, {}
for line in open(log, errors="ignore"):
    m = ts_re.match(line)
    if not m: continue
    now = t(m.group(1))
    p = ping_re.search(line)
    if p: last[p.group(1)] = now; continue
    q = pong_re.search(line)
    if q and q.group(1) in last:
        ms = (now - last.pop(q.group(1))) * 1000.0
        if 0 < ms <= 3000: rtts.setdefault(q.group(1), []).append(ms)

raw = {"0x"+s: round(statistics.median(v), 1) for s, v in rtts.items() if v}

ratio, calibrated = None, {}
if icmp_f and vip_f:
    vip = json.load(open(vip_f)); vrecs = vip if isinstance(vip, list) else vip.get("validators", [])
    s2ip = {r["node_id"].lower(): r["ip"] for r in vrecs if isinstance(r, dict) and r.get("node_id") and r.get("ip")}
    ip2 = {ip: rec.get("avg_ms") for ip, rec in json.load(open(icmp_f))["measurements"].items()
           if isinstance(rec, dict) and rec.get("avg_ms")}
    dual = [(v, ip2[s2ip[k.lower()]]) for k, v in raw.items()
            if s2ip.get(k.lower()) in ip2 and ip2.get(s2ip.get(k.lower(), ""), 0) > 0]
    if len(dual) >= 8:
        ratio = round(statistics.median(s / i for s, i in dual), 3)
        calibrated = {k: round(v / ratio, 1) for k, v in raw.items()}

json.dump({"source": "monad-sonar auth-UDP ping/pong roundtrip (app-level)",
           "calibration_ratio_sonar_over_icmp": ratio,
           "count": len(raw), "rtt_ms_raw": raw,
           "rtt_ms": calibrated or raw}, open(out, "w"), indent=2)
v = sorted((calibrated or raw).values())
print(f"sonar RTT: {len(raw)} validators | calibration ratio={ratio} | "
      f"{'calibrated' if ratio else 'raw'} p50={v[len(v)//2]:.0f}ms" if v else "no RTT")

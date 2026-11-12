#!/usr/bin/env python3
"""Daily network report for monad-sonar: one readable page a day, built from the hosted feeds.

Reads the public feeds this repository produces (prooflines.org/monad/sonar/) and the mainnet
geo/stake view built on them (prooflines.org/monad/geo-latency/), compares with the previous
day, and writes:

  data/daily/<date>.json       machine-readable snapshot (same shape as before, plus deltas)
  data/latest.json             copy of today's snapshot
  reports/<yyyy-mm>/<date>.md  the day's report
  NETWORK.md                   the last 14 days at a glance

It refuses to write anything when the feeds are stale or empty: a missing day says more than a
day of numbers nobody measured. Exit code 2 means "skipped, see stderr".

  python3 tools/daily_report.py              # today (UTC)
  python3 tools/daily_report.py --date 2026-11-13
"""
import argparse
import datetime as dt
import glob
import json
import os
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SONAR = "https://prooflines.org/monad/sonar"
GEO = "https://prooflines.org/monad/geo-latency/geo-latency-data-mainnet.json"
MAX_FEED_AGE_H = 3
UA = {"User-Agent": "monad-sonar-daily-report (+https://github.com/ColinkaMir/monad-sonar)"}


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=30) as r:
        return json.load(r)


def ts(s):
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def collect(now):
    index = get(f"{SONAR}/index.json")
    problems, peers = [], {}
    for net in ("mainnet", "testnet"):
        f = index["feeds"].get(net) or {}
        if not f.get("peers"):
            problems.append(f"{net} feed is empty")
        elif not f.get("updated_at") or (now - ts(f["updated_at"])).total_seconds() > MAX_FEED_AGE_H * 3600:
            problems.append(f"{net} feed last updated {f.get('updated_at')}")
        peers[net] = f.get("peers")
    union = {net: (index["feeds"].get(f"{net}_union") or {}).get("peers") for net in ("mainnet", "testnet")}
    geo = get(GEO)
    geo_age = now - ts(geo["generated_at_utc"].replace(" UTC", "+00:00").replace(" ", "T"))
    if geo_age.total_seconds() > MAX_FEED_AGE_H * 3600:
        problems.append(f"geo view generated {geo['generated_at_utc']}")
    return peers, union, geo, problems


def pct(x):
    return f"{x:.1f}%"


def delta(now, then, fmt=lambda v: f"{v:+d}", unit=""):
    if now is None or then is None:
        return "–"
    d = now - then
    if d == 0:
        return "0"
    return fmt(d) + unit


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--date")
    args = ap.parse_args()
    now = dt.datetime.now(dt.timezone.utc)
    date = args.date or now.date().isoformat()

    peers, union, geo, problems = collect(now)
    if problems:
        print("skipped: " + "; ".join(problems), file=sys.stderr)
        return 2

    cont = {c["name"]: c for c in geo["continents"]}
    top = geo["providers"][:3]
    snap = {
        "date": date, "source": "monad-sonar off-node discovery",
        "testnet": {"peers_discovered": peers["testnet"], "peers_union": union["testnet"]},
        "mainnet": {"peers_discovered": peers["mainnet"], "peers_union": union["mainnet"],
                    "epoch": geo.get("epoch"), "geo_coverage": geo.get("coverage"),
                    "continents": geo.get("continents"), "headline": geo.get("headline"),
                    "top_providers": [{"name": p["name"], "stake_pct": p["stake_pct"]} for p in top]},
    }

    prev_files = sorted(f for f in glob.glob(os.path.join(ROOT, "data/daily/*.json"))
                        if os.path.basename(f)[:10] < date)
    prev = json.load(open(prev_files[-1])) if prev_files else None
    pm = (prev or {}).get("mainnet", {})
    pt = (prev or {}).get("testnet", {})
    pcont = {c["name"]: c for c in pm.get("continents") or []}
    vals = geo["coverage"]["total_validators"]
    pvals = (pm.get("geo_coverage") or {}).get("total_validators")
    since = f"vs {prev['date']}" if prev else "first report"

    rows = [
        ("Mainnet peers discovered", peers["mainnet"], delta(peers["mainnet"], pm.get("peers_discovered"))),
        ("Testnet peers discovered", peers["testnet"], delta(peers["testnet"], pt.get("peers_discovered"))),
        ("Mainnet validators located", vals, delta(vals, pvals)),
    ]
    for name in ("EU", "NA", "APAC", "Other"):
        if name in cont:
            n, p = cont[name]["stake_pct"], (pcont.get(name) or {}).get("stake_pct")
            rows.append((f"{name} share of mainnet stake", pct(n),
                         "–" if p is None else f"{n - p:+.2f} pp"))
    h, ph = geo["headline"], pm.get("headline") or {}
    rows.append(("Top-4 hosting providers, stake", pct(h["top4_provider_stake_pct"]),
                 "–" if "top4_provider_stake_pct" not in ph else
                 f"{h['top4_provider_stake_pct'] - ph['top4_provider_stake_pct']:+.2f} pp"))

    table = "\n".join(f"| {a} | {b} | {c} |" for a, b, c in rows)
    report = f"""# Monad network, {date}

Off-node view from [monad-sonar](https://github.com/ColinkaMir/monad-sonar): peers read over the
discovery protocol, no node involved. Mainnet epoch {geo.get('epoch')}. Change column: {since}.

| | Today | Change |
|---|---|---|
{table}

Largest hosting providers by mainnet stake: {"; ".join(f"{p['name']} {pct(p['stake_pct'])}" for p in top)}.

Data: [`data/daily/{date}.json`](../../data/daily/{date}.json) · live feeds at
[prooflines.org/monad/sonar](https://prooflines.org/monad/sonar/) · stake geography and latency at
[prooflines.org/monad/geo-latency](https://prooflines.org/monad/geo-latency/). Stake shares come
from the mainnet active set; locations from GeoIP of the addresses peers advertise.
"""

    os.makedirs(os.path.join(ROOT, "data/daily"), exist_ok=True)
    os.makedirs(os.path.join(ROOT, f"reports/{date[:7]}"), exist_ok=True)
    for path, obj in ((f"data/daily/{date}.json", snap), ("data/latest.json", snap)):
        json.dump(obj, open(os.path.join(ROOT, path), "w"), indent=2, ensure_ascii=False)
    open(os.path.join(ROOT, f"reports/{date[:7]}/{date}.md"), "w").write(report)

    # NETWORK.md: the last 14 days that have a snapshot.
    days = sorted(glob.glob(os.path.join(ROOT, "data/daily/*.json")))[-14:]
    lines = []
    for f in reversed(days):
        d = json.load(open(f))
        m = d.get("mainnet", {})
        c = {x["name"]: x for x in m.get("continents") or []}
        rep = f"reports/{d['date'][:7]}/{d['date']}.md"
        link = f"[{d['date']}]({rep})" if os.path.exists(os.path.join(ROOT, rep)) else d["date"]
        lines.append(f"| {link} | {m.get('peers_discovered', '–')} | "
                     f"{d.get('testnet', {}).get('peers_discovered', '–')} | "
                     f"{(m.get('geo_coverage') or {}).get('total_validators', '–')} | "
                     f"{pct(c['EU']['stake_pct']) if 'EU' in c else '–'} | "
                     f"{pct(m['headline']['top4_provider_stake_pct']) if m.get('headline') else '–'} |")
    open(os.path.join(ROOT, "NETWORK.md"), "w").write(
        "# Monad network, daily\n\nOne line a day from monad-sonar's hosted feeds; each date links to "
        "that day's report. Generated by `tools/daily_report.py`.\n\n"
        "| Date | Mainnet peers | Testnet peers | Validators located | EU stake | Top-4 providers |\n"
        "|---|---|---|---|---|---|\n" + "\n".join(lines) + "\n")

    eu = cont.get("EU", {}).get("stake_pct")
    print(f"network: {date} · mainnet {peers['mainnet']} peers "
          f"({delta(peers['mainnet'], pm.get('peers_discovered'))}) · {vals} validators"
          + (f" · EU {pct(eu)} stake" if eu is not None else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Continuous sonar UNION daemon (network-generic): rolling union of discovered
name records over a time window. A single crawl misses peers that don't gossip
a discoverable record at that instant; unioning over time catches them and
smooths pass-to-pass variance. Publishes <network>-peers-union.json.

Parameterized entirely via env:
  NETWORK   testnet|mainnet            (required)
  CONFIG    path to the union toml     (default configs/<network>-union.toml)
  IDENTITY  path to a stable keypair   (default configs/<network>-union-identity.key)
  RUN_SECS  crawl window, default 600
  SLEEP_SECS pause between crawls, default 300
  WINDOW_H  age-out horizon hours, default 24
"""
import json, os, subprocess, time, tempfile, pathlib

SONAR    = "/home/solana/monad-sonar"
BIN      = f"{SONAR}/target/release/monad-sonar"
NETWORK  = os.environ["NETWORK"]
CONFIG   = os.getenv("CONFIG", f"{SONAR}/configs/{NETWORK}-union.toml")
IDENTITY = os.getenv("IDENTITY", f"{SONAR}/configs/{NETWORK}-union-identity.key")
WINDOW_SECS = int(os.getenv("RUN_SECS", "600"))
SLEEP_SECS  = int(os.getenv("SLEEP_SECS", "300"))
WINDOW_H    = float(os.getenv("WINDOW_H", "24"))
STORE = f"/home/solana/sonar-union/{NETWORK}-store.json"
OUT   = f"/home/solana/sonar-union/{NETWORK}-peers-union.json"
WEB   = f"/var/www/proofline-public/monad/sonar/{NETWORK}-peers-union.json"
LOG   = f"/home/solana/.sonar-{NETWORK}-daemon.log"

# legacy testnet paths (pre-generic daemon) so history carries over
if NETWORK == "testnet":
    CONFIG = os.getenv("CONFIG", f"{SONAR}/configs/testnet-union.toml")
    IDENTITY = os.getenv("IDENTITY", f"{SONAR}/configs/monad-sonar-union-identity.key")


def log(m):
    with open(LOG, "a") as f:
        f.write("%s %s\n" % (time.strftime("%F %T", time.gmtime()), m))


def load_store():
    try:
        return json.load(open(STORE))
    except Exception:
        return {}


def crawl():
    tmp = tempfile.mktemp(suffix=".json")
    try:
        subprocess.run([BIN, "peers", "--network", NETWORK, "--config", CONFIG,
                        "--identity", IDENTITY, "--run-secs", str(WINDOW_SECS), "--out", tmp],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=WINDOW_SECS + 120, check=False)
        return json.load(open(tmp))
    except Exception as e:
        log("crawl failed: %s" % e)
        return []
    finally:
        try:
            os.unlink(tmp)
        except OSError:
            pass


def publish(store):
    now = time.time()
    peers = [{"secp": k, "ip": v["ip"], "port": v.get("port"), "authPort": v.get("authPort"), "seq": v.get("seq", 0)}
             for k, v in store.items() if v.get("ip") and now - v["last_seen"] <= WINDOW_H * 3600]
    pathlib.Path(os.path.dirname(OUT)).mkdir(parents=True, exist_ok=True)
    pathlib.Path(OUT).write_text(json.dumps(peers, indent=0))
    r = subprocess.run(["sudo", "cp", OUT, WEB], capture_output=True, text=True)
    subprocess.run(["sudo", "chmod", "644", WEB], capture_output=True)
    log("published %d live peers (store=%d)%s" % (len(peers), len(store),
        "" if r.returncode == 0 else " | publish FAIL: " + r.stderr.strip()[:80]))


def main():
    store = load_store()
    log("%s union daemon start: window=%ss sleep=%ss age-out=%sh store=%d" %
        (NETWORK, WINDOW_SECS, SLEEP_SECS, WINDOW_H, len(store)))
    while True:
        peers = crawl()
        now = time.time()
        added = 0
        for p in peers:
            secp = (p.get("secp") or "").lower()
            ip = p.get("ip")
            if not (secp and ip):
                continue
            seq = p.get("seq", 0)
            cur = store.get(secp)
            if cur is None:
                added += 1
            if cur is None or seq >= cur.get("seq", 0):
                store[secp] = {"ip": ip, "port": p.get("port"), "authPort": p.get("authPort"),
                               "seq": seq, "last_seen": now}
            else:
                store[secp]["last_seen"] = now
        pathlib.Path(os.path.dirname(STORE)).mkdir(parents=True, exist_ok=True)
        json.dump(store, open(STORE, "w"))
        log("window: crawl=%d new=%d union=%d" % (len(peers), added, len(store)))
        publish(store)
        time.sleep(SLEEP_SECS)


if __name__ == "__main__":
    main()

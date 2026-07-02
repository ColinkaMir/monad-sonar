// Node-independent chain reads: fetch the current epoch and the active validator set (their
// secp consensus keys) straight from the Monad staking precompile over a public JSON-RPC, so the
// crawler needs no local node files. All calls are plain `eth_call`s against the precompile at
// 0x…1000. Run once at startup (blocking) before the async discovery loop begins.
//
// Copyright (C) 2026 ProofLine. GPL-3.0 (built on category-labs/monad-bft).

use serde_json::json;

/// Staking precompile address (same on testnet and mainnet).
pub const STAKING_PRECOMPILE: &str = "0x0000000000000000000000000000000000001000";

// Selectors (first 4 bytes of calldata).
const SEL_GET_EPOCH: &str = "0x757991a8"; // getEpoch() -> (epoch, _)
const SEL_GET_CONSENSUS_SET: &str = "0xfb29b729"; // paginated -> [is_done, next_index, offset, len, ids…]
const SEL_GET_VALIDATOR: &str = "0x2b6d639a"; // getValidator(id) -> (…, secp bytes, bls bytes)

pub struct Rpc {
    url: String,
    contract: String,
}

type R<T> = Result<T, Box<dyn std::error::Error>>;

impl Rpc {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), contract: STAKING_PRECOMPILE.to_string() }
    }

    /// One JSON-RPC `eth_call`, returning the hex result string (no 0x-strip).
    fn eth_call(&self, data: &str) -> R<String> {
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "eth_call",
            "params": [{ "to": self.contract, "data": data }, "latest"],
        });
        let resp: serde_json::Value = ureq::post(&self.url)
            .set("content-type", "application/json")
            .send_json(body)?
            .into_json()?;
        if let Some(err) = resp.get("error") {
            return Err(format!("rpc error: {err}").into());
        }
        Ok(resp
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or("rpc: no result")?
            .to_string())
    }

    fn eth_call_raw(&self, method: &str, params: serde_json::Value) -> R<String> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp: serde_json::Value = ureq::post(&self.url)
            .set("content-type", "application/json")
            .send_json(body)?
            .into_json()?;
        Ok(resp.get("result").and_then(|r| r.as_str()).ok_or("rpc: no result")?.to_string())
    }

    /// Current consensus epoch (word 0 of getEpoch()).
    pub fn current_epoch(&self) -> R<u64> {
        let hex = self.eth_call(SEL_GET_EPOCH)?;
        Ok(word_u64(strip(&hex), 0))
    }

    /// Latest block number — used as a stand-in for the consensus round (discovery does not depend
    /// on an exact round; the real round runs slightly ahead of the block height).
    pub fn round_proxy(&self) -> R<u64> {
        let hex = self.eth_call_raw("eth_blockNumber", json!([]))?;
        Ok(u64::from_str_radix(hex.trim_start_matches("0x"), 16)?)
    }

    /// Validator ids of the current consensus set (follows the precompile's pagination).
    pub fn consensus_validator_ids(&self) -> R<Vec<u64>> {
        let mut ids = Vec::new();
        let mut next_index: u64 = 0;
        loop {
            let data = format!("{SEL_GET_CONSENSUS_SET}{next_index:064x}");
            let raw = self.eth_call(&data)?;
            let body = strip(&raw);
            let words: Vec<&str> = body.as_bytes().chunks(64).filter_map(|c| std::str::from_utf8(c).ok()).collect();
            if words.len() < 4 {
                break;
            }
            let is_done = word(&words, 0);
            next_index = word(&words, 1);
            let arr_off = (word(&words, 2) / 32) as usize;
            if arr_off >= words.len() {
                break;
            }
            let arr_len = word(&words, arr_off) as usize;
            for i in 0..arr_len {
                if arr_off + 1 + i < words.len() {
                    ids.push(word(&words, arr_off + 1 + i));
                }
            }
            if is_done != 0 {
                break;
            }
        }
        Ok(ids)
    }

    /// Compressed secp256k1 consensus key (node id) of a validator. getValidator's first dynamic
    /// `bytes` return field (offset at word 10) is the secp key; the second is the BLS cert key.
    pub fn validator_secp(&self, id: u64) -> R<[u8; 33]> {
        let data = format!("{SEL_GET_VALIDATOR}{id:064x}");
        let raw = self.eth_call(&data)?;
        let body = strip(&raw);
        let words: Vec<&str> = body.as_bytes().chunks(64).filter_map(|c| std::str::from_utf8(c).ok()).collect();
        if words.len() < 13 {
            return Err(format!("getValidator({id}): short return ({} words)", words.len()).into());
        }
        let off = (word(&words, 10) / 32) as usize; // -> word index of the secp bytes field
        let len = word(&words, off) as usize;
        if len != 33 {
            return Err(format!("getValidator({id}): secp len {len} != 33").into());
        }
        // bytes live in the words immediately after the length word
        let bytes_hex: String = words[off + 1..].concat();
        let raw = hex::decode(&bytes_hex[..66])?; // 33 bytes = 66 hex chars
        let mut out = [0u8; 33];
        out.copy_from_slice(&raw);
        Ok(out)
    }
}

fn strip(h: &str) -> &str {
    h.strip_prefix("0x").unwrap_or(h)
}

fn word(words: &[&str], i: usize) -> u64 {
    words.get(i).map(|w| word_u64_str(w)).unwrap_or(0)
}

fn word_u64(body: &str, i: usize) -> u64 {
    let start = i * 64;
    body.get(start..start + 64).map(word_u64_str).unwrap_or(0)
}

/// A 32-byte ABI word holds values that fit in u64 in its low 8 bytes.
fn word_u64_str(w: &str) -> u64 {
    u64::from_str_radix(&w[48..64], 16).unwrap_or(0)
}

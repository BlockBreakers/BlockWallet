use std::str::FromStr;

use bdk_wallet::bitcoin::consensus::encode::serialize_hex;
use bdk_wallet::bitcoin::hashes::Hash;
use bdk_wallet::bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bdk_wallet::bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bdk_wallet::bitcoin::{
    absolute::LockTime, ecdsa, transaction::Version, Amount, OutPoint, PrivateKey, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness, WitnessProgram, WitnessVersion,
};
use bech32::{segwit, Hrp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::configuration::block_error;
use crate::configuration::http;
use crate::currencies::fees::{check_fee_is_sane, clamp_fee_rate};
use crate::currencies::ltc::double_sha256;

const DUST_LIMIT_SATS: u64 = 1_000;
const WIF_MAINNET: u8 = 0xB0;
const WIF_TESTNET: u8 = 0xEF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LtcNetwork {
    Mainnet,
    Testnet,
}

pub fn parse_network(name: &str) -> LtcNetwork {
    match name.trim().to_ascii_lowercase().as_str() {
        "testnet" | "test" => LtcNetwork::Testnet,
        _ => LtcNetwork::Mainnet,
    }
}

pub fn network_name(network: LtcNetwork) -> &'static str {
    match network {
        LtcNetwork::Testnet => "testnet",
        LtcNetwork::Mainnet => "litecoin",
    }
}

pub fn default_esplora_url(network: LtcNetwork) -> &'static str {
    match network {
        LtcNetwork::Mainnet => "https://litecoinspace.org/api",
        LtcNetwork::Testnet => "https://litecoinspace.org/testnet/api",
    }
}

pub fn resolve_node(ltc_node: &str, network: LtcNetwork) -> String {
    let node = ltc_node.trim();
    if !node.is_empty() {
        return node.trim_end_matches('/').to_string();
    }
    default_esplora_url(network).to_string()
}

fn hrp_for(network: LtcNetwork) -> &'static str {
    match network {
        LtcNetwork::Mainnet => "ltc",
        LtcNetwork::Testnet => "tltc",
    }
}

pub fn encode_address(pubkey_hash20: &[u8; 20], network: LtcNetwork) -> Result<String, block_error::Error> {
    let hrp = Hrp::parse(hrp_for(network)).map_err(|e| block_error::Error::new(format!("invalid hrp: {e}")))?;
    segwit::encode(hrp, segwit::VERSION_0, pubkey_hash20)
        .map_err(|e| block_error::Error::new(format!("bech32 encode failed: {e}")))
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedAddress {
    pub network: LtcNetwork,
    pub program: Vec<u8>,
}

pub fn decode_address(address: &str) -> Result<DecodedAddress, block_error::Error> {
    let (hrp, version, program) = segwit::decode(address.trim())
        .map_err(|e| block_error::Error::new(format!("invalid litecoin address: {e}")))?;
    let network = match hrp.as_str() {
        "ltc" => LtcNetwork::Mainnet,
        "tltc" => LtcNetwork::Testnet,
        other => return Err(block_error::Error::new(format!("unrecognized litecoin address prefix: {other}"))),
    };
    if version != segwit::VERSION_0 || program.len() != 20 {
        return Err(block_error::Error::new(
            "only native SegWit (P2WPKH) litecoin addresses are supported".into(),
        ));
    }
    Ok(DecodedAddress { network, program })
}

pub fn validate_address(address: &str, network: LtcNetwork) -> Result<DecodedAddress, block_error::Error> {
    let decoded = decode_address(address)?;
    if decoded.network != network {
        return Err(block_error::Error::new(format!(
            "address is not valid on {}",
            network_name(network)
        )));
    }
    Ok(decoded)
}

fn witness_program_script(decoded: &DecodedAddress) -> Result<ScriptBuf, block_error::Error> {
    let program = WitnessProgram::new(WitnessVersion::V0, &decoded.program)
        .map_err(|e| block_error::Error::new(format!("invalid witness program: {e:?}")))?;
    Ok(ScriptBuf::new_witness_program(&program))
}

/// Hand-rolled base58check WIF encoding with Litecoin's version byte (`bitcoin::PrivateKey::to_wif`
/// only knows Bitcoin's). Reuses `sha2` (double-SHA256 checksum) and `bs58` (base58 alphabet),
/// both already dependencies from the Solana work — no new crate needed for this half of it.
pub fn encode_wif(secret: &[u8; 32], network: LtcNetwork) -> String {
    let version = match network {
        LtcNetwork::Mainnet => WIF_MAINNET,
        LtcNetwork::Testnet => WIF_TESTNET,
    };
    let mut payload = Vec::with_capacity(38);
    payload.push(version);
    payload.extend_from_slice(secret);
    payload.push(0x01); // compressed-pubkey marker
    let checksum = double_sha256(&payload);
    payload.extend_from_slice(&checksum[..4]);
    bs58::encode(payload).into_string()
}

pub fn decode_wif(wif: &str) -> Result<([u8; 32], LtcNetwork), block_error::Error> {
    let bytes = bs58::decode(wif.trim())
        .into_vec()
        .map_err(|e| block_error::Error::new(format!("invalid WIF: {e}")))?;
    if bytes.len() != 38 {
        return Err(block_error::Error::new("invalid WIF length".into()));
    }
    let (payload, checksum) = bytes.split_at(34);
    let expected = double_sha256(payload);
    if &expected[..4] != checksum {
        return Err(block_error::Error::new("invalid WIF checksum".into()));
    }
    let network = match payload[0] {
        WIF_MAINNET => LtcNetwork::Mainnet,
        WIF_TESTNET => LtcNetwork::Testnet,
        _ => return Err(block_error::Error::new("not a litecoin WIF (unrecognized version byte)".into())),
    };
    if payload[33] != 0x01 {
        return Err(block_error::Error::new("only compressed-key WIF is supported".into()));
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&payload[1..33]);
    Ok((secret, network))
}

pub fn ltc_to_sats(input: &str) -> Result<u64, block_error::Error> {
    let s = crate::currencies::amount::normalize_decimal_input(input)?;
    if let Some((whole, frac)) = s.split_once('.') {
        if frac.len() > 8 {
            return Err(block_error::Error::new("amount has more than 8 decimal places".to_string()));
        }
        let whole_sats: u64 = if whole.is_empty() {
            0
        } else {
            whole
                .parse::<u64>()
                .map_err(|_| block_error::Error::new("amount must be a number".to_string()))?
                .checked_mul(100_000_000)
                .ok_or_else(|| block_error::Error::new("amount is too large".to_string()))?
        };
        let mut frac_s = frac.to_string();
        while frac_s.len() < 8 {
            frac_s.push('0');
        }
        let frac_sats: u64 = frac_s
            .parse()
            .map_err(|_| block_error::Error::new("amount must be a number".to_string()))?;
        whole_sats
            .checked_add(frac_sats)
            .ok_or_else(|| block_error::Error::new("amount is too large".to_string()))
    } else {
        let ltc: u64 = s
            .parse()
            .map_err(|_| block_error::Error::new("amount must be a number".to_string()))?;
        ltc.checked_mul(100_000_000)
            .ok_or_else(|| block_error::Error::new("amount is too large".to_string()))
    }
}

pub fn format_ltc(sats: u64) -> String {
    let whole = sats / 100_000_000;
    let frac = sats % 100_000_000;
    format!("{whole}.{frac:08}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeeTiers {
    pub low: f32,
    pub medium: f32,
    pub high: f32,
}

impl Default for FeeTiers {
    fn default() -> Self {
        Self { low: 1.0, medium: 2.0, high: 5.0 }
    }
}

pub fn fee_rate_from_tier(tiers: &FeeTiers, label: &str) -> f32 {
    let defaults = FeeTiers::default();
    let (rate, fallback) = match label.to_ascii_lowercase().as_str() {
        "low" => (tiers.low, defaults.low),
        "high" => (tiers.high, defaults.high),
        _ => (tiers.medium, defaults.medium),
    };
    clamp_fee_rate(rate, fallback)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LtcHistoryItem {
    pub txid: String,
    pub amount_sats: i64,
    pub confirmations: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LtcSyncState {
    pub confirmed_sats: u64,
    pub pending_sats: u64,
    pub receive_address: String,
    pub history: Vec<LtcHistoryItem>,
    pub offline: bool,
}

impl LtcSyncState {
    pub fn balance_display(&self) -> String {
        if self.offline {
            return format!("{} LTC (offline)", format_ltc(self.confirmed_sats));
        }
        if self.pending_sats == 0 {
            format!("{} LTC", format_ltc(self.confirmed_sats))
        } else {
            format!(
                "{} LTC (+{} pending)",
                format_ltc(self.confirmed_sats),
                format_ltc(self.pending_sats)
            )
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectedUtxo {
    pub txid: String,
    pub vout: u32,
    pub value_sats: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSend {
    pub from: String,
    pub to: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub fee_rate_sat_vb: f32,
    pub total_sats: u64,
    pub change_sats: u64,
    pub utxos: Vec<SelectedUtxo>,
    /// THORChain routing instruction, carried as an `OP_RETURN` output.
    ///
    /// `None` for an ordinary send. When present this is what the network actually obeys,
    /// so it is fee-accounted here and re-checked in `swap::safety` before signing.
    pub memo: Option<String>,
}

impl PreparedSend {
    pub fn summary(&self) -> String {
        format!(
            "To: {}\nAmount: {} LTC\nFee: {} LTC ({:.1} sat/vB)\nTotal: {} LTC",
            self.to,
            format_ltc(self.amount_sats),
            format_ltc(self.fee_sats),
            self.fee_rate_sat_vb,
            format_ltc(self.total_sats)
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CandidateUtxo {
    pub txid: String,
    pub vout: u32,
    pub value_sats: u64,
}

/// Simple, safe, non-optimal coin selection: largest UTXOs first until the target is covered.
/// Deliberately not fee-optimal (no branch-and-bound, no privacy-preserving selection) — a
/// first pass, same spirit as this codebase's Low/Medium/High fee-tier simplicity elsewhere.
pub(crate) fn select_utxos_largest_first(
    utxos: &[CandidateUtxo],
    target_sats: u64,
) -> Option<(Vec<CandidateUtxo>, u64)> {
    let mut sorted: Vec<&CandidateUtxo> = utxos.iter().collect();
    sorted.sort_by(|a, b| b.value_sats.cmp(&a.value_sats));
    let mut selected = Vec::new();
    let mut total = 0u64;
    for u in sorted {
        selected.push(u.clone());
        total += u.value_sats;
        if total >= target_sats {
            return Some((selected, total));
        }
    }
    None
}

fn get_json(url: &str) -> Result<Value, block_error::Error> {
    let text = http::get_text(url)?;
    serde_json::from_str(&text).map_err(|e| block_error::Error::new(format!("invalid response from esplora: {e}")))
}

fn stats_balance(stats: &Value) -> Option<u64> {
    let funded = stats.get("funded_txo_sum")?.as_u64()?;
    let spent = stats.get("spent_txo_sum")?.as_u64()?;
    Some(funded.saturating_sub(spent))
}

fn get_balance_stats(base: &str, address: &str) -> Result<(u64, u64), block_error::Error> {
    let json = get_json(&format!("{base}/address/{address}"))?;
    let confirmed = json.get("chain_stats").and_then(stats_balance).unwrap_or(0);
    // Best-effort: this only reflects unconfirmed *incoming* value (saturating_sub clamps a
    // net-outgoing mempool state to 0 rather than showing negative pending). The actual send
    // path below queries UTXOs directly and isn't affected by this display-only simplification.
    let pending = json.get("mempool_stats").and_then(stats_balance).unwrap_or(0);
    Ok((confirmed, pending))
}

fn get_utxos(base: &str, address: &str) -> Result<Vec<CandidateUtxo>, block_error::Error> {
    let json = get_json(&format!("{base}/address/{address}/utxo"))?;
    let arr = json.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for item in arr {
        let txid = item.get("txid").and_then(Value::as_str).unwrap_or_default().to_string();
        if txid.is_empty() {
            continue;
        }
        let vout = item.get("vout").and_then(Value::as_u64).unwrap_or(0) as u32;
        let value_sats = item.get("value").and_then(Value::as_u64).unwrap_or(0);
        out.push(CandidateUtxo { txid, vout, value_sats });
    }
    Ok(out)
}

fn get_tip_height(base: &str) -> u64 {
    http::get_text(&format!("{base}/blocks/tip/height"))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn get_history(base: &str, address: &str, tip_height: u64) -> Vec<LtcHistoryItem> {
    let Ok(json) = get_json(&format!("{base}/address/{address}/txs")) else {
        return Vec::new();
    };
    let Some(arr) = json.as_array() else { return Vec::new() };
    let mut items = Vec::new();
    for tx in arr {
        let txid = tx.get("txid").and_then(Value::as_str).unwrap_or_default().to_string();
        if txid.is_empty() {
            continue;
        }
        let mut received: i64 = 0;
        let mut sent: i64 = 0;
        if let Some(vout) = tx.get("vout").and_then(Value::as_array) {
            for out in vout {
                if out.get("scriptpubkey_address").and_then(Value::as_str) == Some(address) {
                    received += out.get("value").and_then(Value::as_i64).unwrap_or(0);
                }
            }
        }
        if let Some(vin) = tx.get("vin").and_then(Value::as_array) {
            for inp in vin {
                let prevout_addr = inp
                    .get("prevout")
                    .and_then(|p| p.get("scriptpubkey_address"))
                    .and_then(Value::as_str);
                if prevout_addr == Some(address) {
                    sent += inp
                        .get("prevout")
                        .and_then(|p| p.get("value"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                }
            }
        }
        let confirmed = tx
            .get("status")
            .and_then(|s| s.get("confirmed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let confirmations = if confirmed {
            let block_height = tx
                .get("status")
                .and_then(|s| s.get("block_height"))
                .and_then(Value::as_u64);
            block_height
                .map(|h| tip_height.saturating_sub(h).saturating_add(1) as u32)
                .unwrap_or(1)
        } else {
            0
        };
        items.push(LtcHistoryItem { txid, amount_sats: received - sent, confirmations });
    }
    items
}

fn fetch_fee_tiers(base: &str) -> FeeTiers {
    let defaults = FeeTiers::default();
    // Every rate is clamped on the way in, so an absurd or malformed estimate from the node
    // is neutralised here rather than being carried into transaction building.
    let bound = |low: f32, medium: f32, high: f32| FeeTiers {
        low: clamp_fee_rate(low, defaults.low),
        medium: clamp_fee_rate(medium, defaults.medium),
        high: clamp_fee_rate(high, defaults.high),
    };
    if let Ok(map) = get_json(&format!("{base}/fee-estimates")) {
        if let Some(obj) = map.as_object() {
            let get = |k: &str| obj.get(k).and_then(Value::as_f64).map(|v| v as f32);
            if let (Some(high), Some(medium), Some(low)) = (get("1"), get("3"), get("6")) {
                return bound(low, medium, high);
            }
        }
    }
    // litecoinspace.org didn't serve the classic esplora /fee-estimates during this session's
    // spot check, but does serve the mempool.space-style extended API — fall back to it.
    if let Ok(rec) = get_json(&format!("{base}/v1/fees/recommended")) {
        let get = |k: &str| rec.get(k).and_then(Value::as_f64).map(|v| v as f32);
        if let (Some(high), Some(medium), Some(low)) = (get("fastestFee"), get("halfHourFee"), get("economyFee")) {
            return bound(low, medium, high);
        }
    }
    defaults
}

pub fn fetch_fee_tiers_for(ltc_node: &str, network_name: &str) -> FeeTiers {
    let network = parse_network(network_name);
    let base = resolve_node(ltc_node, network);
    fetch_fee_tiers(&base)
}

fn broadcast_raw(base: &str, hex_tx: &str) -> Result<String, block_error::Error> {
    let text = http::post_text(&format!("{base}/tx"), hex_tx.to_string())?;
    let trimmed = text.trim();
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(trimmed.to_string())
    } else {
        Err(block_error::Error::new(format!("broadcast failed: {trimmed}")))
    }
}

pub fn sync_account(address: &str, ltc_node: &str, network_name: &str) -> Result<LtcSyncState, block_error::Error> {
    let network = parse_network(network_name);
    validate_address(address, network)?;
    let base = resolve_node(ltc_node, network);
    let (confirmed_sats, pending_sats) = match get_balance_stats(&base, address) {
        Ok(v) => v,
        Err(_) => {
            return Ok(LtcSyncState {
                confirmed_sats: 0,
                pending_sats: 0,
                receive_address: address.to_string(),
                history: Vec::new(),
                offline: true,
            });
        }
    };
    let tip = get_tip_height(&base);
    let history = get_history(&base, address, tip);
    Ok(LtcSyncState { confirmed_sats, pending_sats, receive_address: address.to_string(), history, offline: false })
}

fn placeholder_tx(num_inputs: usize, num_outputs: usize, memo_len: Option<usize>) -> Transaction {
    let mut inputs = Vec::with_capacity(num_inputs);
    for _ in 0..num_inputs {
        let mut witness = Witness::new();
        witness.push(vec![0u8; 72]); // worst-case DER signature + sighash-type byte
        witness.push(vec![0u8; 33]); // compressed pubkey
        inputs.push(TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness,
        });
    }
    let placeholder_hash = bdk_wallet::bitcoin::WPubkeyHash::all_zeros();
    let mut outputs = Vec::with_capacity(num_outputs);
    for _ in 0..num_outputs {
        outputs.push(TxOut { value: Amount::ZERO, script_pubkey: ScriptBuf::new_p2wpkh(&placeholder_hash) });
    }
    if let Some(len) = memo_len {
        // An OP_RETURN output carries no value but does occupy vsize, so a swap that did
        // not price it here would build an underpaid transaction that never confirms.
        outputs.push(TxOut { value: Amount::ZERO, script_pubkey: op_return_script(&vec![0u8; len]) });
    }
    Transaction { version: Version::TWO, lock_time: LockTime::ZERO, input: inputs, output: outputs }
}

/// Build the `OP_RETURN` output that carries a THORChain memo.
///
/// Bitcoin and Litecoin relay policy caps this at 80 bytes, which THORChain's own quote
/// response also states. Over that the transaction is simply not relayed, which for a swap
/// means the inbound payment never arrives.
fn op_return_script(memo: &[u8]) -> ScriptBuf {
    const OP_RETURN: u8 = 0x6a;
    const OP_PUSHDATA1: u8 = 0x4c;
    let mut bytes = Vec::with_capacity(memo.len() + 3);
    bytes.push(OP_RETURN);
    // A bare length byte is only a push up to 75; from 76 it would be read as an opcode.
    // Memos are capped at 80 bytes upstream, so OP_PUSHDATA1 covers the rest.
    if memo.len() > 75 {
        bytes.push(OP_PUSHDATA1);
    }
    bytes.push(memo.len() as u8);
    bytes.extend_from_slice(memo);
    ScriptBuf::from(bytes)
}

fn estimate_fee(num_inputs: usize, num_outputs: usize, fee_rate_sat_vb: f32, memo_len: Option<usize>) -> u64 {
    let vsize = placeholder_tx(num_inputs, num_outputs, memo_len).vsize();
    (vsize as f32 * fee_rate_sat_vb).ceil() as u64
}

pub fn prepare_send(
    from: &str,
    to: &str,
    amount_text: &str,
    ltc_node: &str,
    network_name: &str,
    fee_label: &str,
) -> Result<PreparedSend, block_error::Error> {
    prepare_send_with_memo(from, to, amount_text, ltc_node, network_name, fee_label, None)
}

/// Build a payment that may carry a THORChain memo.
///
/// A swap on this chain is exactly an ordinary send to the protocol vault with the routing
/// instruction attached as `OP_RETURN`, so it shares the coin selection, fee ceiling and
/// change handling rather than getting a parallel implementation that could drift.
pub fn prepare_send_with_memo(
    from: &str,
    to: &str,
    amount_text: &str,
    ltc_node: &str,
    network_name: &str,
    fee_label: &str,
    memo: Option<&str>,
) -> Result<PreparedSend, block_error::Error> {
    let network = parse_network(network_name);
    validate_address(from, network)?;
    validate_address(to, network)?;
    if let Some(memo) = memo {
        if memo.len() > 80 {
            return Err(block_error::Error::new(
                "memo is too long to fit in an OP_RETURN output".to_string(),
            ));
        }
    }
    let memo_len = memo.map(str::len);
    let amount_sats = ltc_to_sats(amount_text)?;
    if amount_sats == 0 {
        return Err(block_error::Error::new("amount must be greater than 0".into()));
    }
    let base = resolve_node(ltc_node, network);
    let candidates = get_utxos(&base, from).map_err(|_| block_error::Error::new("litecoin node is unreachable".into()))?;
    let tiers = fetch_fee_tiers(&base);
    let fee_rate = fee_rate_from_tier(&tiers, fee_label);

    // Two-output (recipient + change) assumption to start; if the fee implied by the actual
    // number of inputs selected turns out higher than assumed, retry with the corrected,
    // larger target. Strictly increasing target over a finite UTXO set guarantees this
    // terminates (either it converges, or selection eventually fails with "not enough funds").
    let mut target = amount_sats.saturating_add(estimate_fee(1, 2, fee_rate, memo_len));
    let (selected, total_in, fee_sats) = loop {
        let Some((sel, total)) = select_utxos_largest_first(&candidates, target) else {
            return Err(block_error::Error::new("not enough LTC to cover the amount and fee".into()));
        };
        let fee = estimate_fee(sel.len(), 2, fee_rate, memo_len);
        let needed = amount_sats.saturating_add(fee);
        if total >= needed {
            break (sel, total, fee);
        }
        target = needed;
    };

    let raw_change = total_in.saturating_sub(amount_sats).saturating_sub(fee_sats);
    let (final_fee, final_change) = if raw_change > DUST_LIMIT_SATS {
        (fee_sats, raw_change)
    } else {
        // Not worth an uneconomical change output; fold the dust into the fee instead.
        (fee_sats.saturating_add(raw_change), 0)
    };
    check_fee_is_sane(final_fee, amount_sats)?;

    Ok(PreparedSend {
        from: from.to_string(),
        to: to.to_string(),
        amount_sats,
        fee_sats: final_fee,
        fee_rate_sat_vb: fee_rate,
        total_sats: amount_sats.saturating_add(final_fee),
        change_sats: final_change,
        utxos: selected
            .into_iter()
            .map(|u| SelectedUtxo { txid: u.txid, vout: u.vout, value_sats: u.value_sats })
            .collect(),
        memo: memo.map(str::to_string),
    })
}

pub fn sign_and_broadcast(
    private_key_wif: &str,
    plan: &PreparedSend,
    ltc_node: &str,
    network_name: &str,
) -> Result<String, block_error::Error> {
    let network = parse_network(network_name);
    let (secret, wif_network) = decode_wif(private_key_wif)?;
    if wif_network != network {
        return Err(block_error::Error::new("private key network does not match the selected network".into()));
    }
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&secret)
        .map_err(|e| block_error::Error::new(format!("invalid private key: {e:?}")))?;
    let private_key = PrivateKey::new(secret_key, bdk_wallet::bitcoin::Network::Bitcoin);
    let public_key = private_key.public_key(&secp);
    let pubkey_hash = public_key
        .wpubkey_hash()
        .map_err(|_| block_error::Error::new("litecoin key is not compressed".to_string()))?;
    let from_script = ScriptBuf::new_p2wpkh(&pubkey_hash);

    // The plan's UTXOs and change output belong to `plan.from`. The UI re-reads the account
    // dropdown at confirm time, so without this the wrong key could be paired with this
    // plan — signing would fail at the node instead of here, and the change output would
    // have been addressed to an account the user never reviewed.
    let derived_from = encode_address(pubkey_hash.as_byte_array(), network)?;
    if derived_from != plan.from {
        return Err(block_error::Error::new(
            "this key does not belong to the account the transaction was reviewed for".to_string(),
        ));
    }

    // Re-validated against the network in force at broadcast, not just at prepare time.
    validate_address(&plan.to, network)?;
    let to_decoded = decode_address(&plan.to)?;
    let to_script = witness_program_script(&to_decoded)?;
    check_fee_is_sane(plan.fee_sats, plan.amount_sats)?;

    let mut inputs = Vec::with_capacity(plan.utxos.len());
    let mut prevout_amounts = Vec::with_capacity(plan.utxos.len());
    for u in &plan.utxos {
        let txid = Txid::from_str(&u.txid).map_err(|e| block_error::Error::new(format!("invalid utxo txid: {e:?}")))?;
        inputs.push(TxIn {
            previous_output: OutPoint { txid, vout: u.vout },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        });
        prevout_amounts.push(Amount::from_sat(u.value_sats));
    }

    // Output order follows THORChain's own instruction: the vault first, change back to self
    // second, the memo's OP_RETURN last. An ordinary send simply has no third output.
    let mut outputs = vec![TxOut { value: Amount::from_sat(plan.amount_sats), script_pubkey: to_script }];
    if plan.change_sats > 0 {
        outputs.push(TxOut { value: Amount::from_sat(plan.change_sats), script_pubkey: from_script.clone() });
    }
    if let Some(memo) = &plan.memo {
        if memo.len() > 80 {
            return Err(block_error::Error::new(
                "memo is too long to fit in an OP_RETURN output".to_string(),
            ));
        }
        outputs.push(TxOut {
            value: Amount::ZERO,
            script_pubkey: op_return_script(memo.as_bytes()),
        });
    }

    let mut tx = Transaction { version: Version::TWO, lock_time: LockTime::ZERO, input: inputs, output: outputs };

    // Compute every input's sighash from an immutable borrow first, then assign the resulting
    // witnesses afterward — avoids holding a `SighashCache<&Transaction>` borrow across the
    // mutation of `tx.input[i].witness` below.
    let mut sighashes = Vec::with_capacity(plan.utxos.len());
    {
        let mut cache = SighashCache::new(&tx);
        for (i, amount) in prevout_amounts.iter().enumerate() {
            let sighash = cache
                .p2wpkh_signature_hash(i, &from_script, *amount, EcdsaSighashType::All)
                .map_err(|e| block_error::Error::new(format!("sighash computation failed: {e:?}")))?;
            sighashes.push(sighash);
        }
    }

    for (i, sighash) in sighashes.into_iter().enumerate() {
        let message = Message::from_digest_slice(sighash.as_ref())
            .map_err(|e| block_error::Error::new(format!("invalid sighash: {e:?}")))?;
        let signature = secp.sign_ecdsa(&message, &secret_key);
        let sig_bytes = ecdsa::Signature { signature, sighash_type: EcdsaSighashType::All }.to_vec();
        let mut witness = Witness::new();
        witness.push(sig_bytes);
        witness.push(public_key.to_bytes());
        tx.input[i].witness = witness;
    }

    let base = resolve_node(ltc_node, network);
    let hex_tx = serialize_hex(&tx);
    broadcast_raw(&base, &hex_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bdk_wallet::bitcoin::consensus::deserialize;

    #[test]
    fn op_return_carries_the_whole_memo_as_one_push_up_to_80_bytes() {
        use bdk_wallet::bitcoin::script::Instruction;
        for len in [1usize, 75, 76, 80] {
            let memo = vec![b'='; len];
            let script = op_return_script(&memo);
            assert!(script.is_op_return(), "len {len}");
            let pushes: Vec<_> = script.instructions().skip(1).collect::<Result<_, _>>().unwrap();
            assert_eq!(pushes.len(), 1, "len {len}");
            match &pushes[0] {
                Instruction::PushBytes(bytes) => assert_eq!(bytes.as_bytes(), memo.as_slice(), "len {len}"),
                other => panic!("len {len}: expected a push, got {other:?}"),
            }
            assert!(script.len() <= 83, "len {len}: over the relay limit");
        }
    }

    #[test]
    fn parses_networks_and_default_urls() {
        assert_eq!(parse_network("testnet"), LtcNetwork::Testnet);
        assert_eq!(parse_network(""), LtcNetwork::Mainnet);
        assert!(default_esplora_url(LtcNetwork::Mainnet).contains("litecoinspace.org"));
        assert!(default_esplora_url(LtcNetwork::Testnet).contains("/testnet/"));
        assert_eq!(resolve_node("https://my.node/", LtcNetwork::Mainnet), "https://my.node");
    }

    #[test]
    fn bech32_address_matches_known_mnemonic() {
        // Cross-checked against src/currencies/ltc.rs's own known-mnemonic test, which asserts
        // this same address starts with "ltc1q" via the wallet construction path; here we check
        // the encode/decode round-trip in isolation.
        let hash: [u8; 20] = [
            0x1d, 0x0f, 0x17, 0x2a, 0x0e, 0xcb, 0x48, 0xae, 0xe1, 0xbe, 0x1f, 0x26, 0x87, 0xd2, 0x96, 0x3a, 0xe3,
            0x3f, 0x71, 0xa1,
        ];
        let address = encode_address(&hash, LtcNetwork::Mainnet).unwrap();
        assert!(address.starts_with("ltc1q"), "{address}");
        let decoded = decode_address(&address).unwrap();
        assert_eq!(decoded.network, LtcNetwork::Mainnet);
        assert_eq!(decoded.program, hash.to_vec());

        let testnet_address = encode_address(&hash, LtcNetwork::Testnet).unwrap();
        assert!(testnet_address.starts_with("tltc1q"), "{testnet_address}");
        assert!(validate_address(&testnet_address, LtcNetwork::Mainnet).is_err());
    }

    #[test]
    fn wif_round_trips_and_rejects_wrong_version() {
        let secret = [0x42u8; 32];
        let wif = encode_wif(&secret, LtcNetwork::Mainnet);
        let (decoded_secret, network) = decode_wif(&wif).unwrap();
        assert_eq!(decoded_secret, secret);
        assert_eq!(network, LtcNetwork::Mainnet);

        let testnet_wif = encode_wif(&secret, LtcNetwork::Testnet);
        let (_, testnet_network) = decode_wif(&testnet_wif).unwrap();
        assert_eq!(testnet_network, LtcNetwork::Testnet);

        // A Bitcoin mainnet WIF (version 0x80) must be rejected, not silently accepted.
        let mut btc_payload = vec![0x80u8];
        btc_payload.extend_from_slice(&secret);
        btc_payload.push(0x01);
        let checksum = double_sha256(&btc_payload);
        btc_payload.extend_from_slice(&checksum[..4]);
        let btc_wif = bs58::encode(btc_payload).into_string();
        assert!(decode_wif(&btc_wif).is_err());
    }

    #[test]
    fn amount_conversion_round_trips() {
        assert_eq!(ltc_to_sats("1").unwrap(), 100_000_000);
        assert_eq!(ltc_to_sats("0.00000001").unwrap(), 1);
        assert_eq!(ltc_to_sats(".5").unwrap(), 50_000_000);
        assert!(ltc_to_sats("0.000000001").is_err());
        assert!(ltc_to_sats("").is_err());
        assert_eq!(format_ltc(123_456_789), "1.23456789");
    }

    #[test]
    fn fee_tiers_map_labels() {
        let tiers = FeeTiers { low: 1.0, medium: 3.0, high: 8.0 };
        assert_eq!(fee_rate_from_tier(&tiers, "low"), 1.0);
        assert_eq!(fee_rate_from_tier(&tiers, "High"), 8.0);
        assert_eq!(fee_rate_from_tier(&tiers, "medium"), 3.0);
    }

    #[test]
    fn coin_selection_picks_largest_first_and_no_more_than_needed() {
        let utxos = vec![
            CandidateUtxo { txid: "a".into(), vout: 0, value_sats: 1_000 },
            CandidateUtxo { txid: "b".into(), vout: 0, value_sats: 50_000 },
            CandidateUtxo { txid: "c".into(), vout: 0, value_sats: 10_000 },
        ];
        let (selected, total) = select_utxos_largest_first(&utxos, 55_000).unwrap();
        // 50_000 (largest) then 10_000 covers 55_000; the 1_000 UTXO should not be needed.
        assert_eq!(selected.len(), 2);
        assert_eq!(total, 60_000);
        assert!(selected.iter().any(|u| u.txid == "b"));
        assert!(selected.iter().any(|u| u.txid == "c"));
        assert!(!selected.iter().any(|u| u.txid == "a"));

        assert!(select_utxos_largest_first(&utxos, 1_000_000).is_none());
    }

    #[test]
    fn vsize_grows_with_input_and_output_count() {
        let one_in = placeholder_tx(1, 2, None).vsize();
        let two_in = placeholder_tx(2, 2, None).vsize();
        let two_in_one_out = placeholder_tx(2, 1, None).vsize();
        assert!(two_in > one_in, "{two_in} vs {one_in}");
        assert!(two_in > two_in_one_out, "{two_in} vs {two_in_one_out}");
        // A single P2WPKH-in/P2WPKH-out(x2) transaction should land in a realistic byte range.
        assert!(one_in > 100 && one_in < 250, "{one_in}");
    }

    /// BIP143's published "Native P2WPKH" test vector (the P2WPKH input, input index 1). This
    /// independently verifies the signature against our computed sighash via actual ECDSA
    /// verification (not a hardcoded "expected sighash" string comparison), so a transcription
    /// slip in any intermediate hex can't produce a false pass — only a correct sighash
    /// computation can make a real signature verify.
    #[test]
    fn bip143_native_p2wpkh_known_vector() {
        let unsigned_hex = "0100000002fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f0000000000eeffffffef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a0100000000ffffffff02202cb206000000001976a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac9093510d000000001976a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac11000000";
        let tx_bytes = hex::decode(unsigned_hex).expect("valid hex");
        let tx: Transaction = deserialize(&tx_bytes).expect("valid tx");

        let script_pubkey_hex = "00141d0f172a0ecb48aee1be1f2687d2963ae33f71a1";
        let script_pubkey = ScriptBuf::from(hex::decode(script_pubkey_hex).unwrap());
        let value = Amount::from_sat(600_000_000);

        let mut cache = SighashCache::new(&tx);
        let sighash = cache
            .p2wpkh_signature_hash(1, &script_pubkey, value, EcdsaSighashType::All)
            .expect("sighash");
        let message = Message::from_digest_slice(sighash.as_ref()).expect("32-byte sighash");

        let secp = Secp256k1::new();
        let expected_der_plus_type =
            hex::decode("304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee01").unwrap();
        let expected_sig = ecdsa::Signature::from_slice(&expected_der_plus_type).expect("valid signature encoding");
        let pubkey_bytes = hex::decode("025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357").unwrap();
        let pubkey = bdk_wallet::bitcoin::secp256k1::PublicKey::from_slice(&pubkey_bytes).unwrap();
        secp.verify_ecdsa(&message, &expected_sig.signature, &pubkey)
            .expect("BIP143's published signature must verify against our computed sighash");

        // RFC6979 nonce derivation is deterministic, so signing that same sighash ourselves with
        // the vector's private key must reproduce the exact published signature.
        let secret_key_bytes = hex::decode("619c335025c7f4012e556c2a58b2506e30b8511b53ade95ea316fd8c3286feb9").unwrap();
        let secret_key = SecretKey::from_slice(&secret_key_bytes).unwrap();
        let our_signature = secp.sign_ecdsa(&message, &secret_key);
        assert_eq!(our_signature.serialize_der().as_ref(), expected_sig.signature.serialize_der().as_ref());
    }

    /// A plan carries the account it was built for, and signing refuses any other key.
    ///
    /// The UI re-reads the "From account" dropdown when Confirm is tapped, so without this
    /// binding a plan reviewed for one account could be signed with another account's key —
    /// spending its UTXOs and sending change to an address the user never saw.
    #[test]
    fn signing_refuses_a_key_from_a_different_account() {
        let secret_a = [0x11u8; 32];
        let secret_b = [0x22u8; 32];
        let wif_b = encode_wif(&secret_b, LtcNetwork::Mainnet);

        let secp = Secp256k1::new();
        let key_a = PrivateKey::new(SecretKey::from_slice(&secret_a).unwrap(), bdk_wallet::bitcoin::Network::Bitcoin);
        let hash_a = key_a.public_key(&secp).wpubkey_hash().unwrap();
        let address_a = encode_address(hash_a.as_byte_array(), LtcNetwork::Mainnet).unwrap();

        let plan = PreparedSend {
            from: address_a,
            to: "ltc1qw508d6qejxtdg4y5r3zarvary0c5xw7kgmn4n9".to_string(),
            amount_sats: 100_000,
            fee_sats: 250,
            fee_rate_sat_vb: 2.0,
            total_sats: 100_250,
            change_sats: 0,
            memo: None,
            utxos: vec![SelectedUtxo {
                txid: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
                vout: 0,
                value_sats: 200_000,
            }],
        };

        // Account B's key against account A's plan. Must be refused locally, before any
        // network call, rather than producing a transaction the node happens to reject.
        let err = sign_and_broadcast(&wif_b, &plan, "", "litecoin").unwrap_err();
        match err {
            block_error::Error::New(message) => {
                assert!(message.contains("does not belong to the account"), "got: {message}")
            }
            other => panic!("expected an account-binding error, got {other:?}"),
        }
    }

    #[test]
    fn preparing_a_send_refuses_a_fee_larger_than_the_amount() {
        // Guards the node-supplied fee estimate: this is the shape of the failure a hostile
        // or broken endpoint can otherwise force.
        assert!(check_fee_is_sane(98_000_000, 1_000_000).is_err());
        assert!(check_fee_is_sane(250, 100_000).is_ok());
    }
}

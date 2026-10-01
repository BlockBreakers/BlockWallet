use std::str::FromStr;

use bdk_electrum::electrum_client;
use bdk_electrum::BdkElectrumClient;
use bdk_esplora::esplora_client::Builder as EsploraBuilder;
use bdk_esplora::EsploraExt;
use bdk_wallet::bitcoin::absolute::LockTime;
use bdk_wallet::bitcoin::{
    Address, Amount, FeeRate, Network, Psbt, ScriptBuf, Transaction,
};
use bdk_wallet::chain::ChainPosition;
use bdk_wallet::keys::bip39::{Language, Mnemonic};
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::miniscript::Segwitv0;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, SignOptions, Wallet};

use crate::configuration::block_error;
use crate::currencies::fees::{check_fee_is_sane, clamp_fee_rate};

/// Consecutive unused addresses BDK scans past before concluding the wallet ends there.
///
/// Lowered from 20. Every extra gap slot is more HTTP requests per scan, and this wallet only
/// ever derives index 0 of each keychain, so a deep gap search buys nothing and was a large
/// part of what pushed the sync over Blockstream's rate limit. Five still tolerates a store
/// restored from elsewhere having used a few addresses.
const STOP_GAP: usize = 5;
const PARALLEL_REQUESTS: usize = 1;
/// Matches the ceiling the shared HTTP client applies to every other chain. BDK builds its
/// own transports, so without this the Bitcoin backends would be the one path with no
/// timeout at all — a wedged Esplora or Electrum server would pin the sync thread forever.
const NETWORK_TIMEOUT_SECS: u64 = 30;

fn esplora_client(url: &str) -> bdk_esplora::esplora_client::BlockingClient {
    EsploraBuilder::new(url)
        .timeout(NETWORK_TIMEOUT_SECS)
        .build_blocking()
}

fn electrum_client_for(url: &str) -> Result<BdkElectrumClient<electrum_client::Client>, block_error::Error> {
    let config = electrum_client::ConfigBuilder::new()
        .timeout(Some(NETWORK_TIMEOUT_SECS as u8))
        .build();
    let raw = electrum_client::Client::from_config(url, config)
        .map_err(|e| block_error::Error::new(format!("electrum client: {e}")))?;
    Ok(BdkElectrumClient::new(raw))
}

/// What a Bitcoin account signs with: a phrase under BIP84, or a single imported WIF key.
/// Manual `Debug` so the secret never reaches a log line.
#[derive(Clone)]
pub enum BtcKey {
    Seed { mnemonic: String, passphrase: String },
    Wif(String),
}

impl std::fmt::Debug for BtcKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BtcKey::Seed { .. } => "BtcKey::Seed(..)",
            BtcKey::Wif(_) => "BtcKey::Wif(..)",
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BtcBackend {
    Esplora(String),
    Electrum(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeeTiers {
    pub low: f32,
    pub medium: f32,
    pub high: f32,
}

impl Default for FeeTiers {
    fn default() -> Self {
        Self {
            low: 1.0,
            medium: 2.0,
            high: 5.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BtcHistoryItem {
    pub txid: String,
    pub amount_sats: i64,
    pub confirmations: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BtcSyncState {
    pub confirmed_sats: u64,
    pub pending_sats: u64,
    pub receive_address: String,
    pub history: Vec<BtcHistoryItem>,
    pub offline: bool,
}

impl BtcSyncState {
    pub fn balance_display(&self) -> String {
        if self.offline {
            return format!(
                "{} BTC (offline)",
                format_btc(self.confirmed_sats)
            );
        }
        if self.pending_sats == 0 {
            format!("{} BTC", format_btc(self.confirmed_sats))
        } else {
            format!(
                "{} BTC (+{} pending)",
                format_btc(self.confirmed_sats),
                format_btc(self.pending_sats)
            )
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSend {
    /// The account this plan was built against. `sign_and_broadcast` refuses a key that
    /// derives anything else, so a plan reviewed for one account can never be signed by
    /// another — the UI re-reads the account dropdown at confirm time, and without this
    /// binding a change to that dropdown would silently debit the wrong wallet.
    pub from: String,
    pub to: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub fee_rate_sat_vb: f32,
    pub total_sats: u64,
    /// THORChain routing instruction, carried as an `OP_RETURN` output.
    ///
    /// `None` for an ordinary send. When present this is what the network actually obeys,
    /// so it is fee-accounted by the builder and re-checked in `swap::safety` before signing.
    pub memo: Option<String>,
}

impl PreparedSend {
    pub fn summary(&self) -> String {
        format!(
            "To: {}\nAmount: {} BTC\nFee: {} BTC ({:.1} sat/vB)\nTotal: {} BTC",
            self.to,
            format_btc(self.amount_sats),
            format_btc(self.fee_sats),
            self.fee_rate_sat_vb,
            format_btc(self.total_sats)
        )
    }
}

pub fn parse_network(name: &str) -> Network {
    match name.trim().to_ascii_lowercase().as_str() {
        "testnet" | "test" => Network::Testnet,
        "signet" => Network::Signet,
        _ => Network::Bitcoin,
    }
}

pub fn network_name(network: Network) -> &'static str {
    match network {
        Network::Testnet => "testnet",
        Network::Signet => "signet",
        _ => "bitcoin",
    }
}

pub fn default_esplora_url(network: Network) -> &'static str {
    match network {
        Network::Testnet => "https://blockstream.info/testnet/api",
        Network::Signet => "https://mutinynet.com/api",
        _ => "https://blockstream.info/api",
    }
}

pub fn parse_backend(btc_node: &str, network: Network) -> BtcBackend {
    let node = btc_node.trim();
    if node.is_empty() {
        return BtcBackend::Esplora(default_esplora_url(network).to_string());
    }
    let lower = node.to_ascii_lowercase();
    if lower.starts_with("ssl://") || lower.starts_with("tcp://") {
        BtcBackend::Electrum(node.to_string())
    } else {
        BtcBackend::Esplora(node.trim_end_matches('/').to_string())
    }
}

pub fn validate_address(address: &str, network: Network) -> Result<Address, block_error::Error> {
    let parsed = Address::from_str(address.trim())
        .map_err(|e| block_error::Error::new(format!("invalid bitcoin address: {e}")))?;
    parsed
        .require_network(network)
        .map_err(|_| block_error::Error::new(format!("address is not valid on {}", network_name(network))))
}

pub fn is_bech32_address(address: &str) -> bool {
    let lower = address.trim().to_ascii_lowercase();
    lower.starts_with("bc1") || lower.starts_with("tb1") || lower.starts_with("bcrt1")
}

pub fn btc_to_sats(input: &str) -> Result<u64, block_error::Error> {
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
        let btc: u64 = s
            .parse()
            .map_err(|_| block_error::Error::new("amount must be a number".to_string()))?;
        btc.checked_mul(100_000_000)
            .ok_or_else(|| block_error::Error::new("amount is too large".to_string()))
    }
}

pub fn format_btc(sats: u64) -> String {
    let whole = sats / 100_000_000;
    let frac = sats % 100_000_000;
    format!("{whole}.{frac:08}")
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

fn open_wallet(key: &BtcKey, network: Network) -> Result<Wallet, block_error::Error> {
    match key {
        BtcKey::Seed { mnemonic, passphrase } => open_seed_wallet(mnemonic, passphrase, network),
        // Same descriptor the import builds, so the address synced and spent from is the one
        // the account shows.
        BtcKey::Wif(wif) => Wallet::create_single(format!("wpkh({})", wif.trim()))
            .network(network)
            .create_wallet_no_persist()
            .map_err(|e| block_error::Error::new(format!("BDK wallet from WIF failed: {e:?}"))),
    }
}

fn open_seed_wallet(mnemonic: &str, passphrase: &str, network: Network) -> Result<Wallet, block_error::Error> {
    let mnemonic = Mnemonic::parse_in(Language::English, mnemonic)
        .map_err(|e| block_error::Error::new(format!("Invalid mnemonic: {e:?}")))?;
    let pass = if passphrase.is_empty() {
        None
    } else {
        Some(passphrase.to_string())
    };
    let xkey: ExtendedKey<Segwitv0> = (mnemonic, pass)
        .into_extended_key()
        .map_err(|e| block_error::Error::new(format!("BDK extended key failed: {e:?}")))?;
    let xprv = xkey.into_xprv(network).ok_or_else(|| {
        block_error::Error::new("BDK could not produce an xprv from the mnemonic".to_string())
    })?;
    Wallet::create(
        Bip84(xprv, KeychainKind::External),
        Bip84(xprv, KeychainKind::Internal),
    )
    .network(network)
    .create_wallet_no_persist()
    .map_err(|e| block_error::Error::new(format!("BDK wallet create failed: {e:?}")))
}

fn sync_wallet(wallet: &mut Wallet, backend: &BtcBackend) -> Result<(), block_error::Error> {
    match backend {
        BtcBackend::Esplora(url) => {
            let client = esplora_client(url);
            let request = wallet.start_full_scan();
            let update = client
                .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
                .map_err(|e| block_error::Error::new(format!("esplora sync failed: {e}")))?;
            wallet
                .apply_update(update)
                .map_err(|e| block_error::Error::new(format!("apply esplora update: {e}")))?;
        }
        BtcBackend::Electrum(url) => {
            let client = electrum_client_for(url)?;
            let request = wallet.start_full_scan();
            let update = client
                .full_scan(request, STOP_GAP, 5, false)
                .map_err(|e| block_error::Error::new(format!("electrum sync failed: {e}")))?;
            wallet
                .apply_update(update)
                .map_err(|e| block_error::Error::new(format!("apply electrum update: {e}")))?;
        }
    }
    Ok(())
}

fn collect_state(wallet: &mut Wallet) -> BtcSyncState {
    let balance = wallet.balance();
    let confirmed = balance.confirmed.to_sat();
    let pending = balance.trusted_pending.to_sat() + balance.untrusted_pending.to_sat();
    let receive = wallet
        .next_unused_address(KeychainKind::External)
        .address
        .to_string();
    let tip = wallet.local_chain().tip().height();
    let mut history = Vec::new();
    for tx in wallet.transactions() {
        let (sent, received) = wallet.sent_and_received(tx.tx_node.tx.as_ref());
        let amount = received.to_sat() as i64 - sent.to_sat() as i64;
        let confirmations = match tx.chain_position {
            ChainPosition::Confirmed { anchor, .. } => {
                tip.saturating_sub(anchor.block_id.height).saturating_add(1)
            }
            ChainPosition::Unconfirmed { .. } => 0,
        };
        history.push(BtcHistoryItem {
            txid: tx.tx_node.txid.to_string(),
            amount_sats: amount,
            confirmations,
        });
    }
    BtcSyncState {
        confirmed_sats: confirmed,
        pending_sats: pending,
        receive_address: receive,
        history,
        offline: false,
    }
}

pub fn sync_account(
    key: &BtcKey,
    network_name: &str,
    btc_node: &str,
) -> Result<BtcSyncState, block_error::Error> {
    let network = parse_network(network_name);
    let backend = parse_backend(btc_node, network);
    let mut wallet = open_wallet(key, network)?;
    sync_wallet(&mut wallet, &backend)?;
    Ok(collect_state(&mut wallet))
}

pub fn fetch_fee_tiers(btc_node: &str, network_name: &str) -> FeeTiers {
    let network = parse_network(network_name);
    let backend = parse_backend(btc_node, network);
    let url = match backend {
        BtcBackend::Esplora(url) => url,
        BtcBackend::Electrum(_) => return FeeTiers::default(),
    };
    let estimates_url = format!("{}/fee-estimates", url.trim_end_matches('/'));
    let Ok(text) = crate::configuration::http::get_text(&estimates_url) else {
        return FeeTiers::default();
    };
    let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, f32>>(&text) else {
        return FeeTiers::default();
    };
    let defaults = FeeTiers::default();
    FeeTiers {
        high: clamp_fee_rate(*map.get("1").unwrap_or(&defaults.high), defaults.high),
        medium: clamp_fee_rate(*map.get("3").unwrap_or(&defaults.medium), defaults.medium),
        low: clamp_fee_rate(*map.get("6").unwrap_or(&defaults.low), defaults.low),
    }
}

fn finish_psbt(
    wallet: &mut Wallet,
    script: ScriptBuf,
    amount_sats: u64,
    fee_rate_sat_vb: f32,
    memo: Option<&str>,
) -> Result<(Psbt, u64), block_error::Error> {
    let fee_rate = FeeRate::from_sat_per_vb(fee_rate_sat_vb.ceil() as u64).ok_or_else(|| {
        block_error::Error::new("invalid fee rate".to_string())
    })?;
    let mut builder = wallet.build_tx();
    builder.add_recipient(script, Amount::from_sat(amount_sats));
    if let Some(memo) = memo {
        // THORChain routing instruction. Relay policy caps OP_RETURN at 80 bytes, and a
        // transaction over that is simply never relayed, so it is refused here rather than
        // broadcast into silence.
        let bytes = bdk_wallet::bitcoin::script::PushBytesBuf::try_from(memo.as_bytes().to_vec())
            .map_err(|_| block_error::Error::new("memo is too long for an OP_RETURN output".to_string()))?;
        builder.add_data(&bytes);
    }
    builder.fee_rate(fee_rate);
    builder.nlocktime(LockTime::ZERO);
    let psbt = builder
        .finish()
        .map_err(|e| block_error::Error::new(format!("could not build transaction: {e}")))?;
    let fee = psbt
        .fee()
        .map(|amount| amount.to_sat())
        .unwrap_or(0);
    Ok((psbt, fee))
}

pub fn prepare_send(
    key: &BtcKey,
    network_name: &str,
    btc_node: &str,
    to: &str,
    amount_sats: u64,
    fee_rate_sat_vb: f32,
) -> Result<PreparedSend, block_error::Error> {
    prepare_send_with_memo(key, network_name, btc_node, to, amount_sats, fee_rate_sat_vb, None)
}

/// Build a payment that may carry a THORChain memo.
///
/// A swap on this chain is an ordinary send to the protocol vault with the routing
/// instruction attached, so it shares coin selection, the fee ceiling and change handling
/// rather than getting a parallel implementation that could drift.
#[allow(clippy::too_many_arguments)]
pub fn prepare_send_with_memo(
    key: &BtcKey,
    network_name: &str,
    btc_node: &str,
    to: &str,
    amount_sats: u64,
    fee_rate_sat_vb: f32,
    memo: Option<&str>,
) -> Result<PreparedSend, block_error::Error> {
    if amount_sats == 0 {
        return Err(block_error::Error::new("amount must be greater than 0".to_string()));
    }
    let network = parse_network(network_name);
    let address = validate_address(to, network)?;
    let backend = parse_backend(btc_node, network);
    let mut wallet = open_wallet(key, network)?;
    let from = wallet
        .peek_address(KeychainKind::External, 0)
        .address
        .to_string();
    sync_wallet(&mut wallet, &backend)?;
    let (_psbt, fee) = finish_psbt(
        &mut wallet,
        address.script_pubkey(),
        amount_sats,
        fee_rate_sat_vb,
        memo,
    )?;
    check_fee_is_sane(fee, amount_sats)?;
    Ok(PreparedSend {
        from,
        to: address.to_string(),
        amount_sats,
        fee_sats: fee,
        fee_rate_sat_vb,
        total_sats: amount_sats.saturating_add(fee),
        memo: memo.map(str::to_string),
    })
}

pub fn sign_and_broadcast(
    key: &BtcKey,
    network_name: &str,
    btc_node: &str,
    plan: &PreparedSend,
) -> Result<String, block_error::Error> {
    let network = parse_network(network_name);
    let address = validate_address(&plan.to, network)?;
    let backend = parse_backend(btc_node, network);
    let mut wallet = open_wallet(key, network)?;
    let from = wallet
        .peek_address(KeychainKind::External, 0)
        .address
        .to_string();
    if from != plan.from {
        return Err(block_error::Error::new(
            "this key does not belong to the account the transaction was reviewed for".to_string(),
        ));
    }
    sync_wallet(&mut wallet, &backend)?;
    let (mut psbt, fee) = finish_psbt(
        &mut wallet,
        address.script_pubkey(),
        plan.amount_sats,
        plan.fee_rate_sat_vb,
        plan.memo.as_deref(),
    )?;
    // Re-checked here, not just at prepare time: the fee is rebuilt from a fresh sync, so a
    // node that inflated its estimate between review and confirm would otherwise slip past.
    check_fee_is_sane(fee, plan.amount_sats)?;
    let signed = wallet
        .sign(&mut psbt, SignOptions::default())
        .map_err(|e| block_error::Error::new(format!("signing failed: {e:?}")))?;
    if !signed {
        return Err(block_error::Error::new("transaction was not fully signed".to_string()));
    }
    let tx: Transaction = psbt
        .extract_tx()
        .map_err(|e| block_error::Error::new(format!("extract tx: {e:?}")))?;
    let txid = tx.compute_txid().to_string();
    broadcast_tx(&backend, &tx)?;
    Ok(txid)
}

fn broadcast_tx(backend: &BtcBackend, tx: &Transaction) -> Result<(), block_error::Error> {
    match backend {
        BtcBackend::Esplora(url) => {
            let client = esplora_client(url);
            client
                .broadcast(tx)
                .map_err(|e| block_error::Error::new(format!("broadcast failed: {e}")))?;
        }
        BtcBackend::Electrum(url) => {
            let client = electrum_client_for(url)?;
            client
                .transaction_broadcast(tx)
                .map_err(|e| block_error::Error::new(format!("broadcast failed: {e}")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_backends_and_networks() {
        assert_eq!(parse_network("testnet"), Network::Testnet);
        assert_eq!(parse_network(""), Network::Bitcoin);
        assert_eq!(
            parse_backend("", Network::Bitcoin),
            BtcBackend::Esplora("https://blockstream.info/api".into())
        );
        assert_eq!(
            parse_backend("ssl://electrum.blockstream.info:50002", Network::Bitcoin),
            BtcBackend::Electrum("ssl://electrum.blockstream.info:50002".into())
        );
        assert_eq!(
            parse_backend("https://mempool.space/api", Network::Bitcoin),
            BtcBackend::Esplora("https://mempool.space/api".into())
        );
    }

    #[test]
    fn validates_bech32_on_the_right_network() {
        let main = validate_address(
            "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu",
            Network::Bitcoin,
        )
        .unwrap();
        assert!(is_bech32_address(&main.to_string()));
        assert!(validate_address(
            "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu",
            Network::Testnet
        )
        .is_err());
        assert!(validate_address("not-an-address", Network::Bitcoin).is_err());
    }

    #[test]
    fn converts_btc_amounts() {
        assert_eq!(btc_to_sats("1").unwrap(), 100_000_000);
        assert_eq!(btc_to_sats("0.00000001").unwrap(), 1);
        assert_eq!(btc_to_sats(".5").unwrap(), 50_000_000);
        assert!(btc_to_sats("0.000000001").is_err());
        assert!(btc_to_sats("").is_err());
        assert_eq!(format_btc(123_456_789), "1.23456789");
    }

    #[test]
    fn prepared_send_summary_has_fee_and_total() {
        let prepared = PreparedSend {
            from: "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".into(),
            to: "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu".into(),
            amount_sats: 100_000,
            fee_sats: 250,
            fee_rate_sat_vb: 2.0,
            total_sats: 100_250,
            memo: None,
        };
        let text = prepared.summary();
        assert!(text.contains("0.00100000 BTC"));
        assert!(text.contains("0.00000250 BTC"));
        assert!(text.contains("2.0 sat/vB"));
    }

    #[test]
    fn hd_wallet_opens_without_network_for_mainnet_and_testnet() {
        const ABANDON: &str =
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let key = BtcKey::Seed { mnemonic: ABANDON.into(), passphrase: String::new() };
        let main = open_wallet(&key, Network::Bitcoin).unwrap();
        let main_addr = main.peek_address(KeychainKind::External, 0).address.to_string();
        assert!(main_addr.starts_with("bc1q"), "{main_addr}");
        let test = open_wallet(&key, Network::Testnet).unwrap();
        let test_addr = test.peek_address(KeychainKind::External, 0).address.to_string();
        assert!(test_addr.starts_with("tb1q"), "{test_addr}");
    }

    #[test]
    fn a_wif_account_opens_at_the_address_its_import_shows() {
        const WIF: &str = "KwDiBf89QgGbjEhKnhXJuH7LrciVrZi3qYjgd9M7rFU73sVHnoWn";
        let imported = crate::currencies::btc::BitcoinWallet::from_private_key(WIF).unwrap();
        let wallet = open_wallet(&BtcKey::Wif(WIF.into()), Network::Bitcoin).unwrap();
        let address = wallet.peek_address(KeychainKind::External, 0).address.to_string();
        assert_eq!(Some(address), imported.address);
    }

    #[test]
    fn fee_tiers_map_labels() {
        let tiers = FeeTiers {
            low: 1.0,
            medium: 3.0,
            high: 8.0,
        };
        assert_eq!(fee_rate_from_tier(&tiers, "low"), 1.0);
        assert_eq!(fee_rate_from_tier(&tiers, "High"), 8.0);
        assert_eq!(fee_rate_from_tier(&tiers, "medium"), 3.0);
    }
}

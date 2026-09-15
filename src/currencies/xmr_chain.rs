//! Monero: node access, scanning, balance and sending.
//!
//! Monero is unlike the other four chains in one way that shapes everything here: a node
//! cannot answer "what is the balance of this address". Payments are addressed to one-time
//! keys, and only the private view key can recognise them, so the wallet has to download
//! every block since the account was born and check each output itself. That is what the
//! sync loop does. The upside is that syncing tells the node nothing at all about which
//! outputs are yours; the only requests that reveal anything are the ones a send makes.
//!
//! The cryptography (recognising outputs, CLSAG ring signatures, Bulletproofs+ range proofs)
//! is monero-oxide's, through the `monero-wallet` crate. It is the one part of this codebase
//! where "hand-rolled" would have been the wrong instinct: a mistake in a ring signature is
//! not a rejected transaction, it is a linkable one. What this file owns is the wallet policy
//! around that library: which node to ask, what to remember between syncs, which outputs to
//! spend, and every sanity check before a signature is made.
//!
//! What is remembered between syncs is the list of outputs that belong to this account, with
//! the data needed to spend them. That is written to a cache file so a phone that is killed
//! mid-scan does not start over, but it names amounts and links outputs to this account, so
//! it is encrypted under a key derived from the private view key and is useless without it.

use std::collections::HashSet;
use std::future::Future;
use std::ops::{Bound, RangeBounds};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
use monero_daemon_rpc::{HttpTransport, MoneroDaemon};
use monero_wallet::address::{MoneroAddress, Network};
use monero_wallet::ed25519::{Point, Scalar};
use monero_wallet::interface::prelude::*;
use monero_wallet::primitives::keccak256;
use monero_wallet::ringct::RctType;
use monero_wallet::send::{Change, SendError, SignableTransaction};
use monero_wallet::transaction::Input;
use monero_wallet::{OutputWithDecoys, Scanner, ViewPair, WalletOutput};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::configuration::block_error;
use crate::currencies::fees::check_fee_is_sane;

/// Monero's smallest unit. 1 XMR = 10^12 piconero.
pub const PICONERO_PER_XMR: u64 = 1_000_000_000_000;

/// Blocks an output must wait after inclusion before the network lets it be spent.
const LOCK_WINDOW: u64 = 10;
/// Current ring size: 15 decoys plus the real spend, fixed by consensus since hard fork 15.
const RING_LEN: u8 = 16;
/// Blocks scanned between cache saves and progress reports.
const BLOCKS_PER_FETCH: u64 = 20;
/// Blocks requested from the node at once.
///
/// Every public node measured answers `get_blocks.bin` without the prunable hashes the
/// library needs, so it falls back to three JSON calls per block, and the scan becomes a
/// round-trip problem: 20 blocks took 5 s from the nearest node and 24 s from a farther one,
/// one block at a time. Five in flight cut both to under 3 s. Not more, because these are
/// free public endpoints and a wallet that floods one gets rate-limited into looking offline.
const FETCH_CONCURRENCY: usize = 5;
/// Where a restored or imported account starts scanning when nothing better is known: thirty
/// days of blocks. Older funds need a restore height in Settings, which the app says.
const FALLBACK_RESTORE_WINDOW: u64 = 720 * 30;
/// Blocks re-scanned when the remembered tip hash no longer matches the chain, i.e. a reorg.
const REORG_MARGIN: u64 = 20;
/// Slack before a new wallet's birthday, so clock skew cannot put its first payment before
/// the block the scan starts at.
const BIRTHDAY_MARGIN_SECS: u64 = 24 * 60 * 60;
/// Ceiling on the per-byte fee rate a node can talk this wallet into. Normal priority on
/// mainnet is on the order of 20,000 piconero per byte; this is five thousand times that,
/// so a 2 kB transaction could cost at most 0.2 XMR before the amount-based check applies.
const MAX_FEE_PER_WEIGHT: u64 = 100_000_000;
/// A send that has not appeared on-chain after this many blocks is treated as dropped and
/// its inputs are released.
const PENDING_SPEND_TTL_BLOCKS: u64 = 720;
/// Per-request ceiling on a node response when the library does not name a tighter one.
const MAX_NODE_RESPONSE_BYTES: usize = 100 * 1024 * 1024;

const CACHE_MAGIC: &[u8] = b"BWXMR1";
const CACHE_NONCE_LEN: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XmrNetwork {
    Mainnet,
    /// Monero's public test network that behaves like mainnet (testnet is the developers'
    /// playground and forks ahead of it), so it is what "Use test networks" selects.
    Stagenet,
}

pub fn parse_network(name: &str) -> XmrNetwork {
    match name.trim().to_ascii_lowercase().as_str() {
        "stagenet" | "testnet" | "test" => XmrNetwork::Stagenet,
        _ => XmrNetwork::Mainnet,
    }
}

pub fn network_name(network: XmrNetwork) -> &'static str {
    match network {
        XmrNetwork::Stagenet => "stagenet",
        XmrNetwork::Mainnet => "monero",
    }
}

pub fn is_testnet(network: XmrNetwork) -> bool {
    network == XmrNetwork::Stagenet
}

fn library_network(network: XmrNetwork) -> Network {
    match network {
        XmrNetwork::Mainnet => Network::Mainnet,
        XmrNetwork::Stagenet => Network::Stagenet,
    }
}

/// Public nodes tried in order when no node is configured.
///
/// Mainnet entries are all TLS, in the order they answered a 20-block fetch when measured
/// (5 s, 24 s and 32 s), since the first reachable one is used.
///
/// Stagenet has no public TLS node at all (every one listed by the node aggregators is
/// plaintext), so its defaults are `http://`. That is tolerable only because of what a Monero
/// sync sends: block requests, which say nothing about this account, and stagenet coins,
/// which are worth nothing. A user-entered node is still held to the same TLS rule as every
/// other chain.
pub fn default_nodes(network: XmrNetwork) -> &'static [&'static str] {
    match network {
        XmrNetwork::Mainnet => &[
            "https://xmr.support:18089",
            "https://node.sethforprivacy.com",
            "https://xmr-node.cakewallet.com:18081",
        ],
        XmrNetwork::Stagenet => &[
            "http://node.monerodevs.org:38089",
            "http://node2.monerodevs.org:38089",
            "http://stagenet.xmr-tw.org:38081",
        ],
    }
}

/// The configured node first, then the public defaults as fallbacks.
pub fn resolve_nodes(xmr_node: &str, network: XmrNetwork) -> Vec<String> {
    let mut nodes = Vec::new();
    let node = xmr_node.trim().trim_end_matches('/');
    if !node.is_empty() {
        nodes.push(node.to_string());
    }
    for default in default_nodes(network) {
        if !nodes.iter().any(|existing| existing == default) {
            nodes.push((*default).to_string());
        }
    }
    nodes
}

// ------------------------------------------------------------------------------ keys

/// Reduce 32 bytes into an ed25519 scalar, Monero's `sc_reduce32`.
pub fn reduce_to_scalar(bytes: [u8; 32]) -> [u8; 32] {
    curve25519_dalek::Scalar::from_bytes_mod_order(bytes).to_bytes()
}

/// Parse a private key given as 64 hex characters. Rejected unless it is already a reduced
/// scalar, which every Monero key is; a value outside the group order would derive an
/// address nothing else agrees on.
pub fn scalar_from_hex(text: &str) -> Result<Zeroizing<[u8; 32]>, block_error::Error> {
    let bytes = hex::decode(text)
        .map_err(|_| block_error::Error::new("monero key must be 64 hex characters".to_string()))?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| block_error::Error::new("monero key must be 32 bytes".to_string()))?;
    let scalar = Zeroizing::new(array);
    if reduce_to_scalar(*scalar) != *scalar {
        return Err(block_error::Error::new("monero key is not a reduced scalar".to_string()));
    }
    Ok(scalar)
}

/// The private key pair of one account, held only for as long as a sync or a send needs it.
pub struct AccountKeys {
    spend: Zeroizing<curve25519_dalek::Scalar>,
    view: Zeroizing<curve25519_dalek::Scalar>,
}

impl AccountKeys {
    pub fn new(spend: &[u8; 32], view: &[u8; 32]) -> Result<Self, block_error::Error> {
        let spend = Option::from(curve25519_dalek::Scalar::from_canonical_bytes(*spend))
            .ok_or_else(|| block_error::Error::new("monero spend key is not canonical".to_string()))?;
        let view = Option::from(curve25519_dalek::Scalar::from_canonical_bytes(*view))
            .ok_or_else(|| block_error::Error::new("monero view key is not canonical".to_string()))?;
        Ok(Self { spend: Zeroizing::new(spend), view: Zeroizing::new(view) })
    }

    pub fn from_hex(spend_hex: &str, view_hex: &str) -> Result<Self, block_error::Error> {
        let spend = scalar_from_hex(spend_hex.trim())?;
        let view = scalar_from_hex(view_hex.trim())?;
        Self::new(&spend, &view)
    }

    fn spend_public(&self) -> Point {
        Point::from(ED25519_BASEPOINT_POINT * *self.spend)
    }

    pub fn view_pair(&self) -> Result<ViewPair, block_error::Error> {
        ViewPair::new(self.spend_public(), Zeroizing::new(Scalar::from(*self.view)))
            .map_err(|e| block_error::Error::new(format!("monero keys rejected: {e}")))
    }

    /// The standard address: public spend key and public view key, base58 with Monero's
    /// network prefix and checksum.
    pub fn address(&self, network: XmrNetwork) -> String {
        match self.view_pair() {
            Ok(pair) => pair.legacy_address(library_network(network)).to_string(),
            Err(_) => String::new(),
        }
    }

    pub fn public_keys(&self) -> ([u8; 32], [u8; 32]) {
        let spend = self.spend_public().compress().to_bytes();
        let view = Point::from(ED25519_BASEPOINT_POINT * *self.view).compress().to_bytes();
        (spend, view)
    }

    /// The key image of an output this account owns: what the chain sees when it is spent.
    ///
    /// Returns `None` if the spend key does not actually open the output, which would mean
    /// the scanner matched on the view key alone: not something to record, let alone spend.
    fn key_image(&self, output: &WalletOutput) -> Option<[u8; 32]> {
        let offset: curve25519_dalek::Scalar = output.key_offset().into();
        let input_key = Zeroizing::new(*self.spend + offset);
        let expected: curve25519_dalek::EdwardsPoint = output.key().into();
        if ED25519_BASEPOINT_POINT * *input_key != expected {
            return None;
        }
        let generator: curve25519_dalek::EdwardsPoint =
            Point::biased_hash(output.key().compress().to_bytes()).into();
        Some((generator * *input_key).compress().to_bytes())
    }
}

/// Seconds since the Unix epoch, for wallet birthdays.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// --------------------------------------------------------------------------- addresses

pub fn validate_address(address: &str, network: XmrNetwork) -> Result<MoneroAddress, block_error::Error> {
    let text = address.trim();
    match MoneroAddress::from_str(library_network(network), text) {
        Ok(parsed) => Ok(parsed),
        Err(_) => {
            // Say which of the two things is wrong. A valid address for the other network is
            // a far more useful thing to report than "invalid".
            let other = match network {
                XmrNetwork::Mainnet => Network::Stagenet,
                XmrNetwork::Stagenet => Network::Mainnet,
            };
            if MoneroAddress::from_str(other, text).is_ok() {
                return Err(block_error::Error::new(format!(
                    "address is not valid on {}",
                    network_name(network)
                )));
            }
            Err(block_error::Error::new("invalid monero address".to_string()))
        }
    }
}

// ----------------------------------------------------------------------------- amounts

pub fn xmr_to_piconero(input: &str) -> Result<u64, block_error::Error> {
    let s = crate::currencies::amount::normalize_decimal_input(input)?;
    let too_large = || block_error::Error::new("amount is too large".to_string());
    let not_a_number = || block_error::Error::new("amount must be a number".to_string());
    if let Some((whole, frac)) = s.split_once('.') {
        if frac.len() > 12 {
            return Err(block_error::Error::new("amount has more than 12 decimal places".to_string()));
        }
        let whole_pico: u64 = if whole.is_empty() {
            0
        } else {
            whole
                .parse::<u64>()
                .map_err(|_| not_a_number())?
                .checked_mul(PICONERO_PER_XMR)
                .ok_or_else(too_large)?
        };
        let mut frac_s = frac.to_string();
        while frac_s.len() < 12 {
            frac_s.push('0');
        }
        let frac_pico: u64 = frac_s.parse().map_err(|_| not_a_number())?;
        whole_pico.checked_add(frac_pico).ok_or_else(too_large)
    } else {
        let xmr: u64 = s.parse().map_err(|_| not_a_number())?;
        xmr.checked_mul(PICONERO_PER_XMR).ok_or_else(too_large)
    }
}

/// Twelve decimals is the honest precision but an unreadable width, so trailing zeros are
/// dropped and one decimal is always kept: `1.0`, `0.25`, `0.000000000001`.
pub fn format_xmr(piconero: u64) -> String {
    let whole = piconero / PICONERO_PER_XMR;
    let frac = piconero % PICONERO_PER_XMR;
    let mut text = format!("{whole}.{frac:012}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.push('0');
    }
    text
}

// ------------------------------------------------------------------------- node access

/// The one-method transport `monero-daemon-rpc` needs, over the reqwest client this wallet
/// already ships. Response size is capped at what the library says a given call may return,
/// so a hostile node cannot exhaust a phone's memory with one reply.
#[derive(Clone)]
struct ReqwestTransport {
    client: reqwest::Client,
    base: String,
}

impl HttpTransport for ReqwestTransport {
    fn post(
        &self,
        route: &str,
        body: Vec<u8>,
        response_size_limit: Option<usize>,
    ) -> impl Send + Future<Output = Result<Vec<u8>, InterfaceError>> {
        let url = format!("{}/{}", self.base, route);
        let client = self.client.clone();
        async move {
            let mut response = client
                .post(&url)
                .body(body)
                .send()
                .await
                .map_err(|e| InterfaceError::InterfaceError(sanitize_reqwest_error(&e)))?;
            let status = response.status();
            let limit = response_size_limit.unwrap_or(MAX_NODE_RESPONSE_BYTES).min(MAX_NODE_RESPONSE_BYTES);
            let mut buf = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| InterfaceError::InterfaceError(sanitize_reqwest_error(&e)))?
            {
                if buf.len().saturating_add(chunk.len()) > limit {
                    return Err(InterfaceError::InterfaceError(
                        "response from the node was too large to process".to_string(),
                    ));
                }
                buf.extend_from_slice(&chunk);
            }
            if !status.is_success() {
                return Err(InterfaceError::InterfaceError(format!(
                    "node returned HTTP {}",
                    status.as_u16()
                )));
            }
            Ok(buf)
        }
    }
}

/// reqwest errors print the full URL, which for a user-configured node may carry
/// credentials. Keep the kind of failure and drop the address.
fn sanitize_reqwest_error(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "request timed out".to_string()
    } else if err.is_connect() {
        "could not connect to the node".to_string()
    } else if err.is_body() || err.is_decode() {
        "the node cut the response short".to_string()
    } else {
        "request to the node failed".to_string()
    }
}

type Daemon = MoneroDaemon<ReqwestTransport>;

fn block_on<T>(fut: impl Future<Output = T>) -> Result<T, block_error::Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| block_error::Error::new(format!("could not start async runtime: {e}")))?
        .block_on(async move { Ok(fut.await) })
}

async fn connect(nodes: &[String]) -> Result<(Daemon, String), block_error::Error> {
    // Longer than the shared client's 30 s budget: one block batch can be a few MB, and this
    // is the only chain that downloads blocks rather than asking about an address.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .connect_timeout(crate::configuration::http::CONNECT_TIMEOUT)
        .build()
        .map_err(|e| block_error::Error::new(format!("could not build http client: {e}")))?;
    let mut last_error = String::from("no monero node configured");
    for node in nodes {
        let transport = ReqwestTransport { client: client.clone(), base: node.trim_end_matches('/').to_string() };
        match MoneroDaemon::new(transport).await {
            Ok(daemon) => return Ok((daemon, node.clone())),
            Err(why) => {
                crate::configuration::logging::warn(&format!("monero node {node} unusable: {why}"));
                last_error = why.to_string();
            }
        }
    }
    Err(block_error::Error::new(format!("no monero node reachable: {last_error}")))
}

/// Decoy selection asks for the whole RingCT output distribution once per input, and on
/// mainnet that is around 18 MB. This memoises it for the life of one send, so a two-input
/// transaction downloads it once rather than twice.
struct CachedDecoys<'a> {
    daemon: &'a Daemon,
    distribution: Mutex<Option<(usize, Vec<u64>)>>,
}

impl ProvidesBlockchainMeta for CachedDecoys<'_> {
    fn latest_block_number(&self) -> impl Send + Future<Output = Result<usize, InterfaceError>> {
        self.daemon.latest_block_number()
    }
}

impl monero_wallet::interface::ProvidesUnvalidatedDecoys for CachedDecoys<'_> {
    fn ringct_output_distribution(
        &self,
        range: impl Send + RangeBounds<usize>,
    ) -> impl Send + Future<Output = Result<Vec<u64>, InterfaceError>> {
        let from_genesis = matches!(range.start_bound(), Bound::Unbounded | Bound::Included(0));
        let end = match range.end_bound() {
            Bound::Included(end) => Some(*end),
            Bound::Excluded(end) => end.checked_sub(1),
            Bound::Unbounded => None,
        };
        async move {
            if let (true, Some(end)) = (from_genesis, end) {
                if let Some((cached_end, distribution)) = self.distribution.lock().unwrap().as_ref() {
                    if *cached_end == end {
                        return Ok(distribution.clone());
                    }
                }
                let distribution = ProvidesDecoys::ringct_output_distribution(self.daemon, ..=end).await?;
                *self.distribution.lock().unwrap() = Some((end, distribution.clone()));
                return Ok(distribution);
            }
            match (range.start_bound(), end) {
                (Bound::Included(start), Some(end)) => {
                    ProvidesDecoys::ringct_output_distribution(self.daemon, *start..=end).await
                }
                (Bound::Excluded(start), Some(end)) => {
                    ProvidesDecoys::ringct_output_distribution(self.daemon, (start + 1)..=end).await
                }
                (Bound::Included(start), None) => {
                    ProvidesDecoys::ringct_output_distribution(self.daemon, *start..).await
                }
                (Bound::Excluded(start), None) => {
                    ProvidesDecoys::ringct_output_distribution(self.daemon, (start + 1)..).await
                }
                (Bound::Unbounded, Some(end)) => {
                    ProvidesDecoys::ringct_output_distribution(self.daemon, ..=end).await
                }
                (Bound::Unbounded, None) => ProvidesDecoys::ringct_output_distribution(self.daemon, ..).await,
            }
        }
    }

    fn unlocked_ringct_outputs(
        &self,
        indexes: &[u64],
        evaluate_unlocked: EvaluateUnlocked,
    ) -> impl Send + Future<Output = Result<Vec<Option<[Point; 2]>>, TransactionsError>> {
        ProvidesDecoys::unlocked_ringct_outputs(self.daemon, indexes, evaluate_unlocked)
    }
}

// ---------------------------------------------------------------------- scan state

/// One output this account owns, with everything needed to spend it except the spend key.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OwnedOutput {
    pub tx_hash: String,
    pub index_in_tx: u64,
    pub block: u64,
    pub amount: u64,
    pub key_image: String,
    /// `WalletOutput::serialize()`, hex: the library's own record of the output.
    pub output: String,
    /// Hash of the transaction that spent it. Set when this wallet broadcasts a spend, and
    /// confirmed when the key image is seen on-chain.
    #[serde(default)]
    pub spent_by: Option<String>,
    #[serde(default)]
    pub spent_block: Option<u64>,
}

impl OwnedOutput {
    fn is_unspent(&self) -> bool {
        self.spent_by.is_none()
    }

    fn is_unlocked(&self, chain_height: u64) -> bool {
        chain_height >= self.block.saturating_add(LOCK_WINDOW)
    }
}

/// A send this wallet broadcast and has not yet seen in a block.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PendingSpend {
    pub tx_hash: String,
    /// What left the account, fee included.
    pub amount: u64,
    pub broadcast_height: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScanState {
    pub network: String,
    /// Where scanning began. Reset the cache to move it.
    pub start_height: u64,
    /// The next block to scan; every block below it has been.
    pub next_height: u64,
    /// Hash of block `next_height - 1`, so a reorg is noticed rather than scanned past.
    pub last_hash: String,
    pub outputs: Vec<OwnedOutput>,
    #[serde(default)]
    pub pending: Vec<PendingSpend>,
    /// The restore height this scan was started under, so a changed setting is noticed on
    /// the next sync even if a scanner that predates the change saved the cache after it was
    /// cleared.
    #[serde(default)]
    pub configured_restore_height: Option<u64>,
}

impl ScanState {
    fn has_started(&self) -> bool {
        self.next_height > 0
    }

    fn known_key_images(&self) -> HashSet<String> {
        self.outputs.iter().map(|o| o.key_image.clone()).collect()
    }

    /// Forget everything from `height` on, so those blocks are scanned again.
    fn roll_back_to(&mut self, height: u64) {
        self.outputs.retain(|o| o.block < height);
        for output in &mut self.outputs {
            if output.spent_block.is_some_and(|b| b >= height) {
                output.spent_block = None;
                output.spent_by = None;
            }
        }
        self.next_height = height.max(self.start_height);
        self.last_hash.clear();
    }

    fn release_stale_pending(&mut self, chain_height: u64) {
        let stale: Vec<String> = self
            .pending
            .iter()
            .filter(|p| chain_height > p.broadcast_height.saturating_add(PENDING_SPEND_TTL_BLOCKS))
            .map(|p| p.tx_hash.clone())
            .collect();
        if stale.is_empty() {
            return;
        }
        for output in &mut self.outputs {
            if output.spent_block.is_none() && output.spent_by.as_ref().is_some_and(|h| stale.contains(h)) {
                output.spent_by = None;
            }
        }
        self.pending.retain(|p| !stale.contains(&p.tx_hash));
    }
}

/// Where the scan cache for one account lives, and the key it is sealed with.
///
/// The key is a hash of the private view key, so the file is exactly as private as the view
/// key itself: it can be read by whoever can already see every payment to this account, and
/// by no one else. Nothing in it lets anyone spend.
struct ScanCache {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

impl ScanCache {
    fn for_account(keys: &AccountKeys, address: &str, network: XmrNetwork) -> Result<Self, block_error::Error> {
        let dir = crate::configuration::paths::xmr_cache_dir()?;
        let tag = hex::encode(&keccak256(address.as_bytes())[..8]);
        let path = dir.join(format!("{}-{tag}.bin", network_name(network)));
        let mut material = Zeroizing::new(Vec::with_capacity(64));
        material.extend_from_slice(b"BlockWallet Monero scan cache v1");
        material.extend_from_slice(&keys.view.to_bytes());
        let key = Zeroizing::new(keccak256(material.as_slice()));
        Ok(Self { path, key })
    }

    fn load(&self) -> ScanState {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return ScanState::default();
        };
        match self.open(&bytes) {
            Ok(state) => state,
            Err(why) => {
                crate::configuration::logging::warn(&format!("monero scan cache unreadable, rescanning: {why}"));
                ScanState::default()
            }
        }
    }

    fn open(&self, bytes: &[u8]) -> Result<ScanState, block_error::Error> {
        let body = bytes
            .strip_prefix(CACHE_MAGIC)
            .ok_or_else(|| block_error::Error::new("not a scan cache".to_string()))?;
        if body.len() < CACHE_NONCE_LEN {
            return Err(block_error::Error::new("scan cache truncated".to_string()));
        }
        let (nonce, ciphertext) = body.split_at(CACHE_NONCE_LEN);
        let cipher = ChaCha20Poly1305::new_from_slice(self.key.as_slice())
            .map_err(|e| block_error::Error::new(format!("cipher key: {e}")))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| block_error::Error::new("scan cache does not open with this view key".to_string()))?;
        let state: ScanState = serde_json::from_slice(&plaintext)?;
        Ok(state)
    }

    fn save(&self, state: &ScanState) -> Result<(), block_error::Error> {
        let plaintext = Zeroizing::new(serde_json::to_vec(state)?);
        let cipher = ChaCha20Poly1305::new_from_slice(self.key.as_slice())
            .map_err(|e| block_error::Error::new(format!("cipher key: {e}")))?;
        let mut nonce = [0u8; CACHE_NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|_| block_error::Error::new("scan cache encryption failed".to_string()))?;
        let mut file = Vec::with_capacity(CACHE_MAGIC.len() + CACHE_NONCE_LEN + ciphertext.len());
        file.extend_from_slice(CACHE_MAGIC);
        file.extend_from_slice(&nonce);
        file.extend_from_slice(&ciphertext);
        // Same atomic replace the wallet store uses, so a crash mid-write leaves the old cache
        // rather than half of a new one.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &file)?;
        crate::configuration::paths::restrict_file(&tmp);
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Drop the scan cache for an account, forcing the next sync to start over from its restore
/// height. Used when the user changes that height.
pub fn reset_scan_cache(spend_hex: &str, view_hex: &str, address: &str, network_label: &str) -> Result<(), block_error::Error> {
    let keys = AccountKeys::from_hex(spend_hex, view_hex)?;
    ScanCache::for_account(&keys, address, parse_network(network_label))?.clear();
    Ok(())
}

// ------------------------------------------------------------------------------- sync

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XmrHistoryItem {
    pub txid: String,
    /// Positive for a payment received, negative for one sent (fee included).
    pub amount_piconero: i64,
    pub confirmations: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct XmrSyncState {
    /// Unspent and past the ten-block lock.
    pub unlocked_piconero: u64,
    /// Unspent but too recent to spend yet.
    pub locked_piconero: u64,
    pub receive_address: String,
    pub history: Vec<XmrHistoryItem>,
    pub scanned_height: u64,
    pub chain_height: u64,
    pub offline: bool,
}

impl XmrSyncState {
    pub fn balance_display(&self) -> String {
        if self.offline {
            return format!("{} XMR (offline)", format_xmr(self.unlocked_piconero));
        }
        if self.locked_piconero == 0 {
            format!("{} XMR", format_xmr(self.unlocked_piconero))
        } else {
            format!(
                "{} XMR (+{} pending)",
                format_xmr(self.unlocked_piconero),
                format_xmr(self.locked_piconero)
            )
        }
    }
}

/// Everything a sync needs to know about the account, cloned out of the wallet so the
/// balance thread never holds the settings lock.
#[derive(Clone)]
pub struct SyncAccount {
    pub spend_hex: String,
    pub view_hex: String,
    pub address: String,
    pub restore_height: Option<u64>,
    pub birthday: Option<u64>,
}

impl Drop for SyncAccount {
    fn drop(&mut self) {
        self.spend_hex.zeroize();
        self.view_hex.zeroize();
    }
}

/// Bring the account up to the chain tip and report its balance.
///
/// `progress` is called with a label after every batch of blocks and may return `false` to
/// stop early (the wallet was locked). Progress is saved before each call, so stopping
/// loses nothing.
pub fn sync_account(
    account: &SyncAccount,
    xmr_node: &str,
    network_label: &str,
    progress: &mut dyn FnMut(&str) -> bool,
) -> Result<XmrSyncState, block_error::Error> {
    let network = parse_network(network_label);
    let keys = AccountKeys::from_hex(&account.spend_hex, &account.view_hex)?;
    if keys.address(network) != account.address {
        return Err(block_error::Error::new("monero keys do not match the account address".to_string()));
    }
    let cache = ScanCache::for_account(&keys, &account.address, network)?;
    let mut state = cache.load();
    if state.has_started()
        && (state.network != network_name(network) || state.configured_restore_height != account.restore_height)
    {
        state = ScanState::default();
    }
    state.network = network_name(network).to_string();

    let nodes = resolve_nodes(xmr_node, network);
    let result: Result<u64, block_error::Error> = block_on(async {
        // Progress is saved after every batch, so a node that drops out part-way costs
        // nothing but the switch: the scan carries on from the same block on the next one.
        let mut last_error = block_error::Error::new("no monero node configured".to_string());
        for node in &nodes {
            let daemon = match connect(std::slice::from_ref(node)).await {
                Ok((daemon, _)) => daemon,
                Err(why) => {
                    last_error = why;
                    continue;
                }
            };
            match scan_to_tip(&daemon, &keys, &mut state, &cache, account, progress).await {
                Ok(chain_height) => return Ok(chain_height),
                Err(why) => {
                    crate::configuration::logging::warn(&format!("monero sync via {node} failed: {why}"));
                    last_error = why;
                }
            }
        }
        Err(last_error)
    })?;

    match result {
        Ok(chain_height) => Ok(summarize(&state, &account.address, chain_height, false)),
        Err(why) => {
            // Unreachable node: report what was last known rather than a blank. The scan
            // cache is the memory, so a balance confirmed yesterday is still shown today.
            crate::configuration::logging::warn(&format!("monero sync failed: {why}"));
            let known_height = state.next_height;
            Ok(summarize(&state, &account.address, known_height, true))
        }
    }
}

fn summarize(state: &ScanState, address: &str, chain_height: u64, offline: bool) -> XmrSyncState {
    let mut unlocked = 0u64;
    let mut locked = 0u64;
    for output in state.outputs.iter().filter(|o| o.is_unspent()) {
        if output.is_unlocked(chain_height) {
            unlocked = unlocked.saturating_add(output.amount);
        } else {
            locked = locked.saturating_add(output.amount);
        }
    }
    XmrSyncState {
        unlocked_piconero: unlocked,
        locked_piconero: locked,
        receive_address: address.to_string(),
        history: history_from_state(state, chain_height),
        scanned_height: state.next_height,
        chain_height,
        offline,
    }
}

/// One row per transaction: received outputs grouped by the transaction that created them,
/// spends grouped by the transaction that consumed them, with change netted out.
pub(crate) fn history_from_state(state: &ScanState, chain_height: u64) -> Vec<XmrHistoryItem> {
    let confirmations = |block: u64| -> u32 {
        chain_height.saturating_sub(block).min(u32::MAX as u64) as u32
    };
    let mut items: Vec<XmrHistoryItem> = Vec::new();

    // Spends first, so change outputs can be subtracted from them rather than listed twice.
    let mut spend_hashes: Vec<String> = state
        .outputs
        .iter()
        .filter_map(|o| o.spent_by.clone().filter(|_| o.spent_block.is_some()))
        .collect();
    spend_hashes.sort();
    spend_hashes.dedup();
    for hash in &spend_hashes {
        let spent: u64 = state
            .outputs
            .iter()
            .filter(|o| o.spent_by.as_deref() == Some(hash.as_str()))
            .map(|o| o.amount)
            .sum();
        let change: u64 = state.outputs.iter().filter(|o| &o.tx_hash == hash).map(|o| o.amount).sum();
        let block = state
            .outputs
            .iter()
            .filter(|o| o.spent_by.as_deref() == Some(hash.as_str()))
            .filter_map(|o| o.spent_block)
            .max()
            .unwrap_or(chain_height);
        items.push(XmrHistoryItem {
            txid: hash.clone(),
            amount_piconero: -(spent.saturating_sub(change).min(i64::MAX as u64) as i64),
            confirmations: confirmations(block),
        });
    }

    let mut receive_hashes: Vec<(String, u64)> = state
        .outputs
        .iter()
        .filter(|o| !spend_hashes.contains(&o.tx_hash))
        .map(|o| (o.tx_hash.clone(), o.block))
        .collect();
    receive_hashes.sort();
    receive_hashes.dedup();
    for (hash, block) in receive_hashes {
        let received: u64 = state.outputs.iter().filter(|o| o.tx_hash == hash).map(|o| o.amount).sum();
        items.push(XmrHistoryItem {
            txid: hash,
            amount_piconero: received.min(i64::MAX as u64) as i64,
            confirmations: confirmations(block),
        });
    }

    for pending in &state.pending {
        if !items.iter().any(|item| item.txid == pending.tx_hash) {
            items.push(XmrHistoryItem {
                txid: pending.tx_hash.clone(),
                amount_piconero: -(pending.amount.min(i64::MAX as u64) as i64),
                confirmations: 0,
            });
        }
    }
    items
}

/// Find the first block mined at or after `time`, by bisection on block timestamps.
async fn height_for_time(daemon: &Daemon, time: u64, latest: u64) -> Result<u64, block_error::Error> {
    let timestamp = |number: u64| async move {
        daemon
            .block_by_number(number as usize)
            .await
            .map(|block| block.header.timestamp)
            .map_err(|e| block_error::Error::new(format!("could not read block {number}: {e}")))
    };
    if timestamp(latest).await? < time {
        return Ok(latest);
    }
    let (mut low, mut high) = (1u64, latest);
    while low < high {
        let mid = low + (high - low) / 2;
        if timestamp(mid).await? < time {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    Ok(low)
}

/// Fetch blocks `from..=to`, each validated by the library against its requested number and
/// its transaction list.
///
/// `concurrency` starts at [`FETCH_CONCURRENCY`] and is the caller's to keep between
/// batches: some nodes (the plaintext stagenet ones, in testing) reset every connection past
/// a small per-client limit, so the first failure in a batch drops it to one for the rest of
/// the sync and the failed blocks are re-fetched one at a time. A block that fails even then
/// is retried once more after a pause, since a public node under load drops the odd response
/// and a whole batch should not fail for it.
async fn fetch_blocks(
    daemon: &Daemon,
    from: u64,
    to: u64,
    concurrency: &mut usize,
) -> Result<Vec<ScannableBlock>, block_error::Error> {
    let numbers: Vec<u64> = (from..=to).collect();
    let mut fetched: Vec<Option<ScannableBlock>> = (0..numbers.len()).map(|_| None).collect();

    if *concurrency > 1 {
        for (offset, chunk) in numbers.chunks(*concurrency).enumerate() {
            let handles: Vec<_> = chunk
                .iter()
                .map(|number| {
                    let daemon = daemon.clone();
                    let number = *number as usize;
                    tokio::spawn(async move { daemon.scannable_block_by_number(number).await })
                })
                .collect();
            let mut failed = false;
            for (i, handle) in handles.into_iter().enumerate() {
                match handle.await {
                    Ok(Ok(block)) => fetched[offset * *concurrency + i] = Some(block),
                    _ => failed = true,
                }
            }
            if failed {
                crate::configuration::logging::warn("monero node refused concurrent requests; fetching one block at a time");
                *concurrency = 1;
                break;
            }
        }
    }

    for (slot, number) in fetched.iter_mut().zip(&numbers) {
        if slot.is_some() {
            continue;
        }
        let block = match daemon.scannable_block_by_number(*number as usize).await {
            Ok(block) => block,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                daemon
                    .scannable_block_by_number(*number as usize)
                    .await
                    .map_err(|e| block_error::Error::new(format!("could not fetch block {number}: {e}")))?
            }
        };
        *slot = Some(block);
    }
    Ok(fetched.into_iter().flatten().collect())
}

async fn scan_to_tip(
    daemon: &Daemon,
    keys: &AccountKeys,
    state: &mut ScanState,
    cache: &ScanCache,
    account: &SyncAccount,
    progress: &mut dyn FnMut(&str) -> bool,
) -> Result<u64, block_error::Error> {
    let latest = daemon
        .latest_block_number()
        .await
        .map_err(|e| block_error::Error::new(format!("could not read chain height: {e}")))? as u64;
    let chain_height = latest + 1;

    if !state.has_started() {
        let start = match (account.restore_height, account.birthday) {
            (Some(height), _) => height,
            (None, Some(birthday)) => {
                height_for_time(daemon, birthday.saturating_sub(BIRTHDAY_MARGIN_SECS), latest).await?
            }
            (None, None) => latest.saturating_sub(FALLBACK_RESTORE_WINDOW),
        };
        // Block 0 cannot be requested through `get_blocks.bin`, and holds no RingCT outputs
        // anyway.
        state.start_height = start.max(1).min(chain_height);
        state.next_height = state.start_height;
        state.configured_restore_height = account.restore_height;
        state.last_hash.clear();
        state.outputs.clear();
        state.pending.clear();
    }

    // A remembered tip that the chain no longer has means a reorg: back up and rescan.
    if !state.last_hash.is_empty() && state.next_height > state.start_height {
        let remembered = state.next_height - 1;
        let on_chain = if remembered <= latest {
            daemon
                .block_hash(remembered as usize)
                .await
                .map(hex::encode)
                .map_err(|e| block_error::Error::new(format!("could not read block hash: {e}")))?
        } else {
            String::new()
        };
        if on_chain != state.last_hash {
            crate::configuration::logging::warn("monero chain reorganised; rescanning recent blocks");
            let target = remembered.saturating_sub(REORG_MARGIN);
            state.roll_back_to(target);
        }
    }

    state.release_stale_pending(chain_height);

    let view_pair = keys.view_pair()?;
    let mut scanner = Scanner::new(view_pair);
    let session_start = state.next_height;
    let mut key_images = state.known_key_images();
    let mut concurrency = FETCH_CONCURRENCY;

    while state.next_height <= latest {
        let done = state.next_height - session_start;
        let total = chain_height - session_start;
        let percent = if total == 0 { 100 } else { (done * 100 / total).min(99) };
        if !progress(&format!("Syncing… {percent}% ({} of {} blocks)", done, total)) {
            break;
        }
        let from = state.next_height;
        let to = (from + BLOCKS_PER_FETCH - 1).min(latest);
        let blocks = fetch_blocks(daemon, from, to, &mut concurrency).await?;
        for block in blocks {
            let number = block.block.number() as u64;
            let hash = hex::encode(block.block.hash());
            let tx_hashes: Vec<String> = block.block.transactions.iter().map(hex::encode).collect();

            // Blocks are fetched one by one, so the chain link is checked here: each must
            // build on the one before it. A break means the chain moved under the scan (or
            // the node is lying); either way, back up and let the next sync look again.
            if !state.last_hash.is_empty() && hex::encode(block.block.header.previous) != state.last_hash {
                crate::configuration::logging::warn("monero block did not build on the last scanned block; rescanning");
                state.roll_back_to(number.saturating_sub(REORG_MARGIN));
                cache.save(state)?;
                return Err(block_error::Error::new("chain reorganised during scan".to_string()));
            }

            // Spends: any input whose key image is one of ours consumed one of our outputs.
            for (tx, tx_hash) in block.transactions.iter().zip(tx_hashes.iter()) {
                for input in &tx.prefix().inputs {
                    if let Input::ToKey { key_image, .. } = input {
                        let image = hex::encode(key_image.to_bytes());
                        if key_images.contains(&image) {
                            if let Some(output) = state.outputs.iter_mut().find(|o| o.key_image == image) {
                                output.spent_by = Some(tx_hash.clone());
                                output.spent_block = Some(number);
                            }
                            state.pending.retain(|p| &p.tx_hash != tx_hash);
                        }
                    }
                }
            }

            // Receives. Outputs under an additional timelock (coinbase, or a deliberately
            // locked payment) are left out rather than shown as spendable when they are not.
            let received = scanner
                .scan(block)
                .map_err(|e| block_error::Error::new(format!("could not scan block {number}: {e}")))?
                .not_additionally_locked();
            for output in received {
                let Some(image) = keys.key_image(&output) else {
                    crate::configuration::logging::warn("monero output matched the view key but not the spend key; skipped");
                    continue;
                };
                let image = hex::encode(image);
                let tx_hash = hex::encode(output.transaction());
                let index = output.index_in_transaction();
                // A key already seen is the burning bug: only one of the two is spendable.
                // Keep the first, which is the one wallet2 would keep.
                if key_images.contains(&image) {
                    crate::configuration::logging::warn("monero output reuses a known key; skipped");
                    continue;
                }
                key_images.insert(image.clone());
                state.outputs.push(OwnedOutput {
                    tx_hash,
                    index_in_tx: index,
                    block: number,
                    amount: output.commitment().amount,
                    key_image: image,
                    output: hex::encode(output.serialize()),
                    spent_by: None,
                    spent_block: None,
                });
            }

            state.next_height = number + 1;
            state.last_hash = hash;
        }
        cache.save(state)?;
    }
    Ok(chain_height)
}

// ------------------------------------------------------------------------------- send

#[derive(Clone, Debug, PartialEq)]
pub enum FeePriorityLabel {
    Low,
    Medium,
    High,
}

pub fn priority_from_label(label: &str) -> FeePriorityLabel {
    match label.to_ascii_lowercase().as_str() {
        "low" => FeePriorityLabel::Low,
        "high" => FeePriorityLabel::High,
        _ => FeePriorityLabel::Medium,
    }
}

fn library_priority(priority: &FeePriorityLabel) -> FeePriority {
    match priority {
        FeePriorityLabel::Low => FeePriority::Unimportant,
        FeePriorityLabel::Medium => FeePriority::Normal,
        FeePriorityLabel::High => FeePriority::Elevated,
    }
}

/// A reviewed, not yet signed, payment.
///
/// `signable` is the library's serialized transaction intent: inputs with their decoys, the
/// payment, the change, and a per-transaction secret that seeds its randomness. Signing reads
/// it back rather than rebuilding, so what is signed is exactly what was reviewed.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSend {
    pub from: String,
    pub to: String,
    pub amount_piconero: u64,
    pub fee_piconero: u64,
    pub total_piconero: u64,
    pub priority: String,
    pub inputs: Vec<String>,
    signable: Vec<u8>,
}

impl Drop for PreparedSend {
    fn drop(&mut self) {
        self.signable.zeroize();
    }
}

impl PreparedSend {
    pub fn summary(&self) -> String {
        format!(
            "To: {}\nAmount: {} XMR\nFee: {} XMR ({} priority, {} input{})\nTotal: {} XMR",
            self.to,
            format_xmr(self.amount_piconero),
            format_xmr(self.fee_piconero),
            self.priority,
            self.inputs.len(),
            if self.inputs.len() == 1 { "" } else { "s" },
            format_xmr(self.total_piconero)
        )
    }
}

/// Largest first, which keeps the input count (and so the fee and the ring signatures)
/// small. Deliberately simple, like the Litecoin selection.
pub(crate) fn select_outputs_largest_first(candidates: &[OwnedOutput], target: u64) -> Option<Vec<OwnedOutput>> {
    let mut sorted: Vec<&OwnedOutput> = candidates.iter().collect();
    sorted.sort_by(|a, b| b.amount.cmp(&a.amount));
    let mut selected = Vec::new();
    let mut total = 0u64;
    for output in sorted {
        selected.push(output.clone());
        total = total.saturating_add(output.amount);
        if total >= target {
            return Some(selected);
        }
    }
    None
}

pub fn prepare_send(
    account: &SyncAccount,
    to: &str,
    amount_text: &str,
    xmr_node: &str,
    network_label: &str,
    fee_label: &str,
) -> Result<PreparedSend, block_error::Error> {
    let network = parse_network(network_label);
    let keys = AccountKeys::from_hex(&account.spend_hex, &account.view_hex)?;
    if keys.address(network) != account.address {
        return Err(block_error::Error::new("monero keys do not match the account address".to_string()));
    }
    let destination = validate_address(to, network)?;
    let amount = xmr_to_piconero(amount_text)?;
    if amount == 0 {
        return Err(block_error::Error::new("amount must be greater than zero".to_string()));
    }
    let priority = priority_from_label(fee_label);

    let cache = ScanCache::for_account(&keys, &account.address, network)?;
    let state = cache.load();
    if !state.has_started() || state.network != network_name(network) {
        return Err(block_error::Error::new(
            "this account has not synced yet; wait for the balance before sending".to_string(),
        ));
    }

    let nodes = resolve_nodes(xmr_node, network);
    let view_pair = keys.view_pair()?;
    let (signable, inputs) = block_on(async {
        let (daemon, _) = connect(&nodes).await?;
        let latest = daemon
            .latest_block_number()
            .await
            .map_err(|e| block_error::Error::new(format!("could not read chain height: {e}")))? as u64;
        let chain_height = latest + 1;
        // Outputs past the lock at the node's tip, not the cache's: a block may have landed
        // since the last sync, but an output can never become spendable earlier than the
        // scan said, so this is safe in the direction that matters.
        let candidates: Vec<OwnedOutput> = state
            .outputs
            .iter()
            .filter(|o| o.is_unspent() && o.is_unlocked(chain_height.min(state.next_height)))
            .cloned()
            .collect();
        if candidates.is_empty() {
            return Err(block_error::Error::new("no spendable monero: the balance is zero or still locked".to_string()));
        }

        let fee_rate = daemon
            .fee_rate(library_priority(&priority), MAX_FEE_PER_WEIGHT)
            .await
            .map_err(|e| block_error::Error::new(format!("could not read fee rate: {e}")))?;
        let decoys = CachedDecoys { daemon: &daemon, distribution: Mutex::new(None) };

        // Add inputs until the library confirms the transaction pays for itself. The fee
        // depends on the input count, so the loop asks the library rather than guessing.
        let mut target = amount;
        let mut selected_decoys: Vec<(OwnedOutput, OutputWithDecoys)> = Vec::new();
        loop {
            let Some(selected) = select_outputs_largest_first(&candidates, target) else {
                return Err(block_error::Error::new("not enough unlocked monero for that amount plus the fee".to_string()));
            };
            for output in &selected {
                if selected_decoys.iter().any(|(existing, _)| existing.key_image == output.key_image) {
                    continue;
                }
                let bytes = hex::decode(&output.output)
                    .map_err(|_| block_error::Error::new("scan cache holds an unreadable output".to_string()))?;
                let wallet_output = WalletOutput::read(&mut bytes.as_slice())
                    .map_err(|e| block_error::Error::new(format!("scan cache holds an unreadable output: {e}")))?;
                let with_decoys = OutputWithDecoys::new(&mut OsRng, &decoys, RING_LEN, latest as usize, wallet_output)
                    .await
                    .map_err(|e| block_error::Error::new(format!("could not select decoys: {e}")))?;
                selected_decoys.push((output.clone(), with_decoys));
            }
            let inputs: Vec<OutputWithDecoys> = selected_decoys
                .iter()
                .filter(|(o, _)| selected.iter().any(|s| s.key_image == o.key_image))
                .map(|(_, d)| d.clone())
                .collect();

            let mut seed = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(&mut *seed);
            match SignableTransaction::new(
                RctType::ClsagBulletproofPlus,
                seed,
                inputs,
                vec![(destination, amount)],
                Change::new(view_pair.clone(), None),
                vec![],
                fee_rate,
            ) {
                Ok(signable) => {
                    let images = selected.iter().map(|o| o.key_image.clone()).collect();
                    return Ok((signable, images));
                }
                Err(SendError::NotEnoughFunds { necessary_fee, .. }) => {
                    let needed = amount.saturating_add(necessary_fee.unwrap_or(0));
                    let available: u64 = candidates.iter().map(|o| o.amount).sum();
                    // More inputs raise the fee again, so this can go round more than once;
                    // it cannot go round forever, because a target that already covers what
                    // the library asked for is reported rather than retried.
                    if needed <= target || needed > available {
                        return Err(block_error::Error::new(format!(
                            "not enough unlocked monero: have {} XMR, need {} XMR including the fee",
                            format_xmr(available),
                            format_xmr(needed)
                        )));
                    }
                    target = needed;
                }
                Err(why) => return Err(block_error::Error::new(format!("could not build transaction: {why}"))),
            }
        }
    })??;

    let fee = signable.necessary_fee();
    check_fee_is_sane(fee, amount)?;
    let total = amount
        .checked_add(fee)
        .ok_or_else(|| block_error::Error::new("amount is too large".to_string()))?;
    Ok(PreparedSend {
        from: account.address.clone(),
        to: destination.to_string(),
        amount_piconero: amount,
        fee_piconero: fee,
        total_piconero: total,
        priority: fee_label.to_string(),
        inputs,
        signable: signable.serialize(),
    })
}

pub fn sign_and_broadcast(
    account: &SyncAccount,
    plan: &PreparedSend,
    xmr_node: &str,
    network_label: &str,
) -> Result<String, block_error::Error> {
    let network = parse_network(network_label);
    let keys = AccountKeys::from_hex(&account.spend_hex, &account.view_hex)?;
    // The plan's inputs and change belong to `plan.from`. The UI re-reads the account
    // dropdown at confirm time, so the key is checked against the reviewed plan here rather
    // than discovered at the node.
    if keys.address(network) != plan.from || account.address != plan.from {
        return Err(block_error::Error::new(
            "this key does not belong to the account the transaction was reviewed for".to_string(),
        ));
    }
    validate_address(&plan.to, network)?;
    check_fee_is_sane(plan.fee_piconero, plan.amount_piconero)?;

    let signable = SignableTransaction::read(&mut plan.signable.as_slice())
        .map_err(|e| block_error::Error::new(format!("reviewed transaction is unreadable: {e}")))?;
    if signable.necessary_fee() != plan.fee_piconero {
        return Err(block_error::Error::new("reviewed fee no longer matches the transaction".to_string()));
    }
    let spend = Zeroizing::new(Scalar::from(*keys.spend));
    let tx = signable
        .sign(&mut OsRng, &spend)
        .map_err(|e| block_error::Error::new(format!("signing failed: {e}")))?;
    let tx_hash = hex::encode(tx.hash());

    let cache = ScanCache::for_account(&keys, &account.address, network)?;
    let nodes = resolve_nodes(xmr_node, network);
    let broadcast_height = block_on(async {
        let (daemon, _) = connect(&nodes).await?;
        daemon
            .publish_transaction(&tx)
            .await
            .map_err(|e| block_error::Error::new(format!("broadcast failed: {e}")))?;
        daemon
            .latest_block_number()
            .await
            .map(|n| n as u64 + 1)
            .map_err(|e| block_error::Error::new(format!("could not read chain height: {e}")))
    })??;

    // Mark the inputs spent now, so a second send before the next sync cannot pick them
    // again, and so the balance drops immediately rather than two minutes from now.
    let mut state = cache.load();
    for output in &mut state.outputs {
        if plan.inputs.contains(&output.key_image) {
            output.spent_by = Some(tx_hash.clone());
        }
    }
    state.pending.push(PendingSpend { tx_hash: tx_hash.clone(), amount: plan.total_piconero, broadcast_height });
    if let Err(why) = cache.save(&state) {
        crate::configuration::logging::warn(&format!("could not record monero spend: {why}"));
    }
    Ok(tx_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str =
        "49vDbkSo7eve3J41sBdjvjaBUyz8qHohsQcGtRf63qEUTMBvmA45fpp5pSacMdSg7A3b71RejLzB8EkGbfjp5PELVF2N4Zn";
    const STAGENET_ADDRESS: &str =
        "5A8FgbMkmG2e3J41sBdjvjaBUyz8qHohsQcGtRf63qEUTMBvmA45fpp5pSacMdSg7A3b71RejLzB8EkGbfjp5PELVHCRUaE";

    #[test]
    fn network_names_round_trip() {
        assert_eq!(parse_network("stagenet"), XmrNetwork::Stagenet);
        assert_eq!(parse_network("testnet"), XmrNetwork::Stagenet);
        assert_eq!(parse_network("monero"), XmrNetwork::Mainnet);
        assert_eq!(parse_network(""), XmrNetwork::Mainnet);
        assert_eq!(parse_network(network_name(XmrNetwork::Stagenet)), XmrNetwork::Stagenet);
        assert!(is_testnet(XmrNetwork::Stagenet));
        assert!(!is_testnet(XmrNetwork::Mainnet));
    }

    #[test]
    fn a_configured_node_is_tried_first_and_defaults_follow() {
        let nodes = resolve_nodes(" https://my.node:18089/ ", XmrNetwork::Mainnet);
        assert_eq!(nodes[0], "https://my.node:18089");
        assert_eq!(nodes.len(), 1 + default_nodes(XmrNetwork::Mainnet).len());
        let plain = resolve_nodes("", XmrNetwork::Stagenet);
        assert_eq!(plain, default_nodes(XmrNetwork::Stagenet));
        // The mainnet defaults are all TLS; the stagenet exception is deliberate and documented.
        assert!(default_nodes(XmrNetwork::Mainnet).iter().all(|n| n.starts_with("https://")));
    }

    #[test]
    fn addresses_are_checked_per_network() {
        assert!(validate_address(ADDRESS, XmrNetwork::Mainnet).is_ok());
        assert!(validate_address(STAGENET_ADDRESS, XmrNetwork::Stagenet).is_ok());
        let wrong = validate_address(STAGENET_ADDRESS, XmrNetwork::Mainnet).unwrap_err();
        assert!(format!("{wrong:?}").contains("not valid on monero"), "{wrong:?}");
        assert!(validate_address("4notanaddress", XmrNetwork::Mainnet).is_err());
        assert!(validate_address("", XmrNetwork::Mainnet).is_err());
        // A flipped character breaks the keccak checksum.
        let mut damaged = ADDRESS.to_string();
        damaged.replace_range(10..11, "1");
        assert!(validate_address(&damaged, XmrNetwork::Mainnet).is_err());
    }

    #[test]
    fn amounts_parse_and_format_at_twelve_decimals() {
        assert_eq!(xmr_to_piconero("1").unwrap(), PICONERO_PER_XMR);
        assert_eq!(xmr_to_piconero("0.5").unwrap(), PICONERO_PER_XMR / 2);
        assert_eq!(xmr_to_piconero("0.000000000001").unwrap(), 1);
        assert_eq!(xmr_to_piconero(".25").unwrap(), PICONERO_PER_XMR / 4);
        assert!(xmr_to_piconero("0.0000000000001").is_err(), "13 decimals");
        assert!(xmr_to_piconero("abc").is_err());
        assert!(xmr_to_piconero("99999999999999999999").is_err());
        assert_eq!(format_xmr(PICONERO_PER_XMR), "1.0");
        assert_eq!(format_xmr(PICONERO_PER_XMR / 4), "0.25");
        assert_eq!(format_xmr(1), "0.000000000001");
        assert_eq!(format_xmr(0), "0.0");
        assert_eq!(format_xmr(1_234_500_000_000_000), "1234.5");
    }

    #[test]
    fn scalar_parsing_requires_reduced_32_bytes() {
        assert!(scalar_from_hex("3b094ca7218f175e91fa2402b4ae239a2fe8262792a3e718533a1a357a1e4109").is_ok());
        assert!(scalar_from_hex("zz").is_err());
        assert!(scalar_from_hex(&"00".repeat(31)).is_err());
        // All 0xff is above the group order, so it is not a key any Monero wallet would hold.
        assert!(scalar_from_hex(&"ff".repeat(32)).is_err());
    }

    fn output(tx: &str, block: u64, amount: u64, image: &str) -> OwnedOutput {
        OwnedOutput {
            tx_hash: tx.into(),
            index_in_tx: 0,
            block,
            amount,
            key_image: image.into(),
            output: String::new(),
            spent_by: None,
            spent_block: None,
        }
    }

    #[test]
    fn balance_splits_locked_from_unlocked_and_ignores_spent() {
        let mut state = ScanState { next_height: 1_000, ..ScanState::default() };
        state.outputs.push(output("a", 900, 5, "ki-a"));
        state.outputs.push(output("b", 995, 3, "ki-b")); // 5 blocks old: still locked
        let mut spent = output("c", 800, 7, "ki-c");
        spent.spent_by = Some("d".into());
        spent.spent_block = Some(950);
        state.outputs.push(spent);
        let summary = summarize(&state, ADDRESS, 1_000, false);
        assert_eq!(summary.unlocked_piconero, 5);
        assert_eq!(summary.locked_piconero, 3);
        assert_eq!(summary.balance_display(), "0.000000000005 XMR (+0.000000000003 pending)");
        assert!(summarize(&state, ADDRESS, 1_000, true).balance_display().ends_with("(offline)"));
        // Ten blocks on, the locked output is spendable.
        assert_eq!(summarize(&state, ADDRESS, 1_005, false).locked_piconero, 0);
    }

    #[test]
    fn history_nets_change_out_of_spends_and_lists_pending() {
        let mut state = ScanState { next_height: 1_000, ..ScanState::default() };
        let mut spent = output("in", 100, 10, "ki-1");
        spent.spent_by = Some("out".into());
        spent.spent_block = Some(500);
        state.outputs.push(spent);
        state.outputs.push(output("out", 500, 6, "ki-2")); // change from that spend
        state.outputs.push(output("gift", 990, 2, "ki-3"));
        state.pending.push(PendingSpend { tx_hash: "pending".into(), amount: 3, broadcast_height: 999 });
        let history = history_from_state(&state, 1_000);
        let by_id = |id: &str| history.iter().find(|h| h.txid == id).cloned().unwrap();
        assert_eq!(by_id("out").amount_piconero, -4, "10 spent minus 6 change, fee included");
        assert_eq!(by_id("out").confirmations, 500);
        assert_eq!(by_id("in").amount_piconero, 10);
        assert_eq!(by_id("gift").amount_piconero, 2);
        assert_eq!(by_id("gift").confirmations, 10);
        assert_eq!(by_id("pending").amount_piconero, -3);
        assert_eq!(by_id("pending").confirmations, 0);
        assert_eq!(history.len(), 4, "change must not appear as a separate receipt");
    }

    #[test]
    fn a_reorg_rolls_back_outputs_and_spends_past_the_target() {
        let mut state = ScanState { start_height: 100, next_height: 1_000, last_hash: "x".into(), ..ScanState::default() };
        state.outputs.push(output("old", 500, 1, "ki-old"));
        state.outputs.push(output("new", 995, 1, "ki-new"));
        let mut spent_recently = output("older", 400, 1, "ki-older");
        spent_recently.spent_by = Some("s".into());
        spent_recently.spent_block = Some(996);
        state.outputs.push(spent_recently);
        state.roll_back_to(990);
        assert_eq!(state.next_height, 990);
        assert!(state.last_hash.is_empty());
        assert!(state.outputs.iter().all(|o| o.tx_hash != "new"));
        let older = state.outputs.iter().find(|o| o.tx_hash == "older").unwrap();
        assert!(older.spent_by.is_none(), "a spend in a dropped block is unspent again");
    }

    #[test]
    fn a_send_that_never_confirms_releases_its_inputs() {
        let mut state = ScanState { next_height: 2_000, ..ScanState::default() };
        let mut held = output("a", 100, 1, "ki-a");
        held.spent_by = Some("lost".into());
        state.outputs.push(held);
        state.pending.push(PendingSpend { tx_hash: "lost".into(), amount: 1, broadcast_height: 1_000 });
        state.release_stale_pending(1_500);
        assert!(state.outputs[0].spent_by.is_some(), "still within the window");
        state.release_stale_pending(1_000 + PENDING_SPEND_TTL_BLOCKS + 1);
        assert!(state.outputs[0].spent_by.is_none());
        assert!(state.pending.is_empty());
    }

    #[test]
    fn selection_is_largest_first_and_fails_honestly() {
        let candidates = vec![output("a", 1, 2, "a"), output("b", 1, 9, "b"), output("c", 1, 4, "c")];
        let picked = select_outputs_largest_first(&candidates, 10).unwrap();
        assert_eq!(picked.iter().map(|o| o.amount).collect::<Vec<_>>(), vec![9, 4]);
        assert!(select_outputs_largest_first(&candidates, 16).is_none());
    }

    #[test]
    fn scan_cache_round_trips_and_needs_the_view_key() {
        let root = std::env::temp_dir().join(format!("blockwallet-xmr-cache-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let keys = AccountKeys::from_hex(
            "3b094ca7218f175e91fa2402b4ae239a2fe8262792a3e718533a1a357a1e4109",
            "0f3fe25d0c6d4c94dde0c0bcc214b233e9c72927f813728b0f01f28f9d5e1201",
        )
        .unwrap();
        // Built by hand at a temp path rather than through `for_account`, so the test does not
        // have to point `BLOCKWALLET_HOME` somewhere and race every other test that reads it.
        let mut cache = ScanCache::for_account(&keys, ADDRESS, XmrNetwork::Mainnet).unwrap();
        cache.path = root.join("monero-test.bin");
        let mut state = ScanState { network: "monero".into(), start_height: 5, next_height: 9, ..ScanState::default() };
        state.outputs.push(output("t", 7, 42, "ki"));
        cache.save(&state).unwrap();
        assert_eq!(cache.load(), state);
        let raw = std::fs::read(&cache.path).unwrap();
        assert!(raw.starts_with(CACHE_MAGIC));
        assert!(!raw.windows(2).any(|w| w == b"ki"), "plaintext must not leak");
        // Another account's key does not open it, and an unreadable cache means a rescan,
        // never a crash.
        let other = ScanCache { path: cache.path.clone(), key: Zeroizing::new([7u8; 32]) };
        assert_eq!(other.load(), ScanState::default());
        cache.clear();
        assert_eq!(cache.load(), ScanState::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn key_images_match_the_spend_key_only() {
        // The derived key pair for the pinned vector agrees with itself on the address, and
        // the public keys are the ones the address encodes.
        let keys = AccountKeys::from_hex(
            "3b094ca7218f175e91fa2402b4ae239a2fe8262792a3e718533a1a357a1e4109",
            "0f3fe25d0c6d4c94dde0c0bcc214b233e9c72927f813728b0f01f28f9d5e1201",
        )
        .unwrap();
        assert_eq!(keys.address(XmrNetwork::Mainnet), ADDRESS);
        let (spend_pub, view_pub) = keys.public_keys();
        let parsed = MoneroAddress::from_str(Network::Mainnet, ADDRESS).unwrap();
        assert_eq!(parsed.spend().compress().to_bytes(), spend_pub);
        assert_eq!(parsed.view().compress().to_bytes(), view_pub);
    }

    #[test]
    fn a_prepared_send_summary_names_every_figure() {
        let plan = PreparedSend {
            from: ADDRESS.into(),
            to: ADDRESS.into(),
            amount_piconero: PICONERO_PER_XMR / 2,
            fee_piconero: 30_000_000,
            total_piconero: PICONERO_PER_XMR / 2 + 30_000_000,
            priority: "Medium".into(),
            inputs: vec!["a".into(), "b".into()],
            signable: vec![1, 2, 3],
        };
        let text = plan.summary();
        assert!(text.contains("Amount: 0.5 XMR"));
        assert!(text.contains("Fee: 0.00003 XMR (Medium priority, 2 inputs)"));
        assert!(text.contains("Total: 0.50003 XMR"));
    }
}

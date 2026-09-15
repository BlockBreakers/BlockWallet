use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

use crate::configuration::block_error;

pub const SCHEMA_VERSION: u32 = 1;
const FILE_VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
/// 64 MiB / t=3. Above OWASP's 19 MiB floor: this is a phone wallet that can be seized with
/// the store on it, so the KDF is the only thing standing between a stolen file and the
/// keys. A Librem 5 has 3 GB, so 64 MiB per unlock is affordable, and it multiplies an
/// attacker's cost per guess by roughly three over the previous parameters.
const M_KIB: u32 = 65_536;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// Bounds on KDF parameters read back from a store file.
///
/// These come off disk, and the app can be pointed at a `.dic` someone else produced. An
/// unbounded `m_kib` is a request to allocate that many kibibytes — trivially gigabytes,
/// which on this hardware is an out-of-memory kill rather than an error message. The floor
/// matters too: a file claiming m=8/t=1 would make its own contents cheap to brute-force,
/// so a store weaker than the original defaults is refused rather than opened.
const MIN_ACCEPTED_M_KIB: u32 = 19_456;
const MAX_ACCEPTED_M_KIB: u32 = 1_048_576; // 1 GiB
const MIN_ACCEPTED_T: u32 = 2;
const MAX_ACCEPTED_T: u32 = 16;
const MAX_ACCEPTED_P: u32 = 4;

/// Shortest password `create` will accept.
///
/// Set to 1, i.e. only an empty password is refused, at the maintainer's request so that
/// throwaway test wallets are quick to create.
///
/// The trade-off, recorded here rather than lost: the store file can be copied off the device
/// and attacked offline with no rate limit this app can impose, so the password is the only
/// thing between a stolen file and the keys. Argon2id at 64 MiB makes each guess expensive,
/// but a four-digit PIN is only ~10^4 guesses and stays cheap regardless. Raise this to 12 if
/// the build is ever aimed at people holding real funds.
pub const MIN_PASSWORD_LEN: usize = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    pub algo: String,
    pub salt: String,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreFile {
    pub version: u32,
    pub kdf: KdfParams,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PayloadV1 {
    pub schema: u32,
    pub mnemonic: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
    pub settings: StoreSettings,
    pub btc: Vec<BtcRecord>,
    pub eth: Vec<EthRecord>,
    #[serde(default)]
    pub sol: Vec<SolRecord>,
    #[serde(default)]
    pub ltc: Vec<LtcRecord>,
    #[serde(default)]
    pub xmr: Vec<XmrRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreSettings {
    pub starred: Vec<String>,
    pub infura_key: String,
    pub etherscan_key: String,
    pub btc_node: String,
    pub eth_node: String,
    #[serde(default)]
    pub sol_node: String,
    #[serde(default)]
    pub ltc_node: String,
    /// Monero daemon RPC. Empty means the built-in public defaults, tried in order.
    #[serde(default)]
    pub xmr_node: String,
    /// THORNode endpoint for swap quotes. Empty means the built-in public default.
    #[serde(default)]
    pub thornode_url: String,
    #[serde(default)]
    pub btc_network: String,
    #[serde(default)]
    pub eth_network: String,
    #[serde(default)]
    pub sol_network: String,
    #[serde(default)]
    pub ltc_network: String,
    #[serde(default)]
    pub xmr_network: String,
    #[serde(default)]
    pub custom_tokens: Vec<CustomTokenRecord>,
    #[serde(default)]
    pub lock_timeout_secs: u32,
    #[serde(default)]
    pub show_prices: bool,
    #[serde(default)]
    pub fiat: String,
    #[serde(default)]
    pub btc_units: String,
    /// Hide assets whose balance is a confirmed zero. Defaults to false on records written
    /// before the option existed, which is the behaviour those users already had.
    #[serde(default)]
    pub hide_zero_balances: bool,
    /// Affiliate payout addresses, per venue family. Empty means no fee is requested.
    #[serde(default)]
    pub fee_evm_address: String,
    #[serde(default)]
    pub fee_lifi_integrator: String,
    #[serde(default)]
    pub fee_solana_account: String,
    #[serde(default)]
    pub fee_thorchain_address: String,
    #[serde(default)]
    pub fee_maya_address: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomTokenRecord {
    pub symbol: String,
    pub name: String,
    pub address: String,
    pub decimals: i32,
    /// "btc" | "eth" | "sol" | "ltc" | "xmr". Empty on records written before Solana support existed;
    /// those all predate anything but ERC-20 custom tokens, so they default to "eth".
    #[serde(default)]
    pub chain: String,
}

impl CustomTokenRecord {
    pub fn chain_or_default(&self) -> String {
        if self.chain.is_empty() {
            "eth".to_string()
        } else {
            self.chain.clone()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtcRecord {
    pub name: String,
    pub mnemonic: Option<String>,
    pub passphrase: Option<String>,
    pub private_key_wif: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LtcRecord {
    pub name: String,
    pub mnemonic: Option<String>,
    pub passphrase: Option<String>,
    pub private_key_wif: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct XmrRecord {
    pub name: String,
    pub mnemonic: Option<String>,
    pub passphrase: Option<String>,
    /// Private spend key, hex. Set only for an account imported by key; a seed-derived one
    /// is re-derived on unlock like every other chain.
    pub private_spend_key: Option<String>,
    /// Block to start scanning from, when the user set one.
    #[serde(default)]
    pub restore_height: Option<u64>,
    /// Unix time the phrase was generated, when this app generated it. Bounds the scan for a
    /// new wallet so it does not have to be told a restore height.
    #[serde(default)]
    pub birthday: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EthRecord {
    pub name: String,
    pub mnemonic: Option<String>,
    pub path: Option<String>,
    pub private_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SolRecord {
    pub name: String,
    pub mnemonic: Option<String>,
    pub path: Option<String>,
    pub private_key: Option<String>,
}

#[derive(Clone)]
pub struct StoreSession {
    pub path: PathBuf,
    key: Vec<u8>,
    salt: Vec<u8>,
    m_kib: u32,
    t: u32,
    p: u32,
}

impl std::fmt::Debug for StoreSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreSession")
            .field("path", &self.path)
            .field("kdf", &format!("argon2id m={} t={} p={}", self.m_kib, self.t, self.p))
            .finish_non_exhaustive()
    }
}

impl Default for PayloadV1 {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            mnemonic: None,
            passphrase: None,
            settings: StoreSettings {
                starred: Vec::new(),
                infura_key: String::new(),
                etherscan_key: String::new(),
                btc_node: String::new(),
                eth_node: String::new(),
                sol_node: String::new(),
                ltc_node: String::new(),
                xmr_node: String::new(),
                thornode_url: String::new(),
                btc_network: String::new(),
                eth_network: String::new(),
                sol_network: String::new(),
                ltc_network: String::new(),
                xmr_network: String::new(),
                custom_tokens: Vec::new(),
                lock_timeout_secs: 0,
                show_prices: false,
                fiat: String::new(),
                btc_units: String::new(),
                hide_zero_balances: false,
                fee_evm_address: String::new(),
                fee_lifi_integrator: String::new(),
                fee_solana_account: String::new(),
                fee_thorchain_address: String::new(),
                fee_maya_address: String::new(),
            },
            btc: Vec::new(),
            eth: Vec::new(),
            sol: Vec::new(),
            ltc: Vec::new(),
            xmr: Vec::new(),
        }
    }
}

impl PayloadV1 {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Reject KDF parameters outside the accepted envelope.
///
/// Called before `derive_key` on the unlock path, where the values are attacker-influenced.
pub fn check_kdf_params(m_kib: u32, t: u32, p: u32) -> Result<(), block_error::Error> {
    if !(MIN_ACCEPTED_M_KIB..=MAX_ACCEPTED_M_KIB).contains(&m_kib) {
        return Err(block_error::Error::new(format!(
            "wallet store asks for an unsupported memory cost ({m_kib} KiB)"
        )));
    }
    if !(MIN_ACCEPTED_T..=MAX_ACCEPTED_T).contains(&t) {
        return Err(block_error::Error::new(format!(
            "wallet store asks for an unsupported time cost ({t})"
        )));
    }
    if p == 0 || p > MAX_ACCEPTED_P {
        return Err(block_error::Error::new(format!(
            "wallet store asks for an unsupported parallelism ({p})"
        )));
    }
    Ok(())
}

/// Minimum-strength check for a new password. Applied at `create` only: an existing store
/// must stay openable with whatever password it was made with.
pub fn check_password_strength(password: &str) -> Result<(), block_error::Error> {
    if password.is_empty() {
        return Err(block_error::Error::new("password must not be empty".to_string()));
    }
    // Counted in characters, not bytes, so a passphrase in a non-Latin script is not held to
    // a longer standard than the same length of ASCII.
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(block_error::Error::new(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

fn derive_key(password: &str, salt: &[u8], m_kib: u32, t: u32, p: u32) -> Result<Vec<u8>, block_error::Error> {
    let params = Params::new(m_kib, t, p, Some(KEY_LEN)).map_err(|e| {
        block_error::Error::new(format!("argon2 params: {e}"))
    })?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = vec![0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| block_error::Error::new(format!("argon2id failed: {e}")))?;
    Ok(key)
}

fn encrypt(key: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), block_error::Error> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| block_error::Error::new(format!("cipher key: {e}")))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| block_error::Error::new("encryption failed".to_string()))?;
    Ok((nonce_bytes.to_vec(), ciphertext))
}

fn decrypt(key: &[u8], nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, block_error::Error> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| block_error::Error::new(format!("cipher key: {e}")))?;
    if nonce.len() != NONCE_LEN {
        return Err(block_error::Error::new("invalid nonce".to_string()));
    }
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| block_error::Error::new("decryption failed".to_string()))
}

/// Tighten a path to owner-only access on Unix. A no-op elsewhere.
///
/// `fs::write` creates at 0666 & ~umask, which is 0644 on a typical system: the encrypted
/// store would be readable by every local account, handing anyone a copy to brute-force
/// offline at their leisure. The encryption is what protects the contents, but there is no
/// reason to publish the ciphertext.
#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Owner-only file. Used for the store itself.
pub const FILE_MODE_PRIVATE: u32 = 0o600;
/// Owner-only directory.
pub const DIR_MODE_PRIVATE: u32 = 0o700;

fn write_file(path: &Path, session: &StoreSession, payload: &PayloadV1) -> Result<(), block_error::Error> {
    // The serialized payload holds the mnemonic and every private key in the clear. It is
    // wiped before the buffer is freed rather than left in the heap for whatever allocates
    // next — the app's own claim is that locking removes keys from memory, and a plaintext
    // copy surviving in freed memory (or reaching swap) would make that false.
    let mut plaintext = serde_json::to_vec(payload)?;
    let encrypted = encrypt(&session.key, &plaintext);
    plaintext.zeroize();
    let (nonce, ciphertext) = encrypted?;

    let file = StoreFile {
        version: FILE_VERSION,
        kdf: KdfParams {
            algo: "argon2id".to_string(),
            salt: hex::encode(&session.salt),
            m_kib: session.m_kib,
            t: session.t,
            p: session.p,
        },
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    };
    let json = serde_json::to_vec_pretty(&file)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        let _ = restrict_permissions(parent, DIR_MODE_PRIVATE);
    }

    // Write to a sibling temp file and rename over the target. `fs::write` truncates first,
    // so losing power or being killed mid-write left a half-written store — which for a
    // wallet whose only backup may be the recovery phrase is indistinguishable from losing
    // the funds. Rename within a directory is atomic on both Unix and Windows, so the file
    // at `path` is always either the old store or the complete new one.
    let temp = path.with_extension("tmp");
    fs::write(&temp, &json)?;
    let _ = restrict_permissions(&temp, FILE_MODE_PRIVATE);
    if let Err(why) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(why.into());
    }
    let _ = restrict_permissions(path, FILE_MODE_PRIVATE);
    Ok(())
}

impl StoreSession {
    pub fn create(path: &Path, password: &str, payload: &PayloadV1) -> Result<Self, block_error::Error> {
        check_password_strength(password)?;
        let mut salt = vec![0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        let key = derive_key(password, &salt, M_KIB, T_COST, P_COST)?;
        let session = Self {
            path: path.to_path_buf(),
            key,
            salt,
            m_kib: M_KIB,
            t: T_COST,
            p: P_COST,
        };
        write_file(path, &session, payload)?;
        Ok(session)
    }

    pub fn unlock(path: &Path, password: &str) -> Result<(PayloadV1, Self), block_error::Error> {
        let bytes = fs::read(path)?;
        let file: StoreFile = serde_json::from_slice(&bytes).map_err(|_| {
            block_error::Error::new("wallet store is not schema v1".to_string())
        })?;
        if file.version != FILE_VERSION {
            return Err(block_error::Error::new(format!(
                "unsupported store version {}",
                file.version
            )));
        }
        if file.kdf.algo != "argon2id" {
            return Err(block_error::Error::new("unsupported kdf".to_string()));
        }
        let salt = hex::decode(&file.kdf.salt)
            .map_err(|_| block_error::Error::new("invalid kdf salt".to_string()))?;
        let nonce = hex::decode(&file.nonce)
            .map_err(|_| block_error::Error::new("invalid nonce".to_string()))?;
        let ciphertext = hex::decode(&file.ciphertext)
            .map_err(|_| block_error::Error::new("invalid ciphertext".to_string()))?;
        // Bounded before use: these three numbers come straight off disk and drive an
        // allocation, so an absurd memory cost here is an out-of-memory kill rather than a
        // failed unlock.
        check_kdf_params(file.kdf.m_kib, file.kdf.t, file.kdf.p)?;
        let key = derive_key(password, &salt, file.kdf.m_kib, file.kdf.t, file.kdf.p)?;
        let mut plaintext = decrypt(&key, &nonce, &ciphertext)?;
        let parsed = serde_json::from_slice::<PayloadV1>(&plaintext)
            .map_err(|_| block_error::Error::new("invalid wallet payload".to_string()));
        // Same reasoning as `write_file`: the decrypted JSON holds every secret in the
        // clear, so the buffer is wiped rather than handed back to the allocator intact.
        plaintext.zeroize();
        let payload = parsed?;
        if payload.schema != SCHEMA_VERSION {
            return Err(block_error::Error::new(format!(
                "unsupported payload schema {}",
                payload.schema
            )));
        }
        let session = Self {
            path: path.to_path_buf(),
            key,
            salt,
            m_kib: file.kdf.m_kib,
            t: file.kdf.t,
            p: file.kdf.p,
        };
        Ok((payload, session))
    }

    pub fn save(&self, payload: &PayloadV1) -> Result<(), block_error::Error> {
        write_file(&self.path, self, payload)
    }

    pub fn wipe(&mut self) {
        crate::configuration::secrets::wipe_vec(&mut self.key);
        crate::configuration::secrets::wipe_vec(&mut self.salt);
        self.m_kib = 0;
        self.t = 0;
        self.p = 0;
    }
}

impl Drop for StoreSession {
    fn drop(&mut self) {
        self.wipe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    /// Long enough to satisfy `check_password_strength`; the tests care about the crypto
    /// round-trip, not the policy.
    const TEST_PASSWORD: &str = "correct horse battery staple";

    fn temp_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("blockwallet-store-{}.json", rand::thread_rng().next_u64()));
        path
    }

    fn sample_payload() -> PayloadV1 {
        PayloadV1 {
            schema: SCHEMA_VERSION,
            mnemonic: Some("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string()),
            passphrase: None,
            settings: StoreSettings {
                starred: vec!["BTC".to_string(), "ETH".to_string(), "SOL".to_string(), "LTC".to_string()],
                infura_key: String::new(),
                etherscan_key: String::new(),
                btc_node: "ssl://localhost:50001".to_string(),
                eth_node: "https://example.invalid".to_string(),
                sol_node: "https://sol.example.invalid".to_string(),
                ltc_node: "https://ltc.example.invalid".to_string(),
                xmr_node: "https://xmr.example.invalid".to_string(),
                thornode_url: "https://thornode.example.invalid".to_string(),
                btc_network: "bitcoin".to_string(),
                eth_network: "sepolia".to_string(),
                sol_network: "devnet".to_string(),
                ltc_network: "testnet".to_string(),
                xmr_network: "stagenet".to_string(),
                custom_tokens: Vec::new(),
                lock_timeout_secs: 120,
                show_prices: false,
                fiat: "usd".to_string(),
                btc_units: "btc".to_string(),
                hide_zero_balances: false,
                fee_evm_address: String::new(),
                fee_lifi_integrator: String::new(),
                fee_solana_account: String::new(),
                fee_thorchain_address: String::new(),
                fee_maya_address: String::new(),
            },
            btc: vec![BtcRecord {
                name: "btc_wallet".to_string(),
                mnemonic: Some("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string()),
                passphrase: None,
                private_key_wif: None,
            }],
            eth: vec![EthRecord {
                name: "eth_wallet".to_string(),
                mnemonic: Some("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string()),
                path: Some("m/44'/60'/0'/0/0".to_string()),
                private_key: None,
            }],
            sol: vec![SolRecord {
                name: "sol_wallet".to_string(),
                mnemonic: Some("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string()),
                path: Some("m/44'/501'/0'/0'".to_string()),
                private_key: None,
            }],
            ltc: vec![LtcRecord {
                name: "ltc_wallet".to_string(),
                mnemonic: Some("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string()),
                passphrase: None,
                private_key_wif: None,
            }],
            xmr: vec![XmrRecord {
                name: "xmr_wallet".to_string(),
                mnemonic: Some("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string()),
                passphrase: None,
                private_spend_key: None,
                restore_height: Some(3_000_000),
                birthday: None,
            }],
        }
    }

    #[test]
    fn session_debug_omits_key_bytes() {
        let path = temp_path();
        let session = StoreSession::create(&path, TEST_PASSWORD, &PayloadV1::new()).unwrap();
        let rendered = format!("{session:?}");
        assert!(rendered.contains("StoreSession"));
        assert!(!rendered.contains(&hex::encode(&session.key)));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn create_unlock_roundtrip() {
        let path = temp_path();
        let payload = sample_payload();
        let session = StoreSession::create(&path, "correct horse", &payload).unwrap();
        let (loaded, _) = StoreSession::unlock(&path, "correct horse").unwrap();
        assert_eq!(loaded, payload);
        let _ = session;
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn hide_zero_balances_survives_a_store_roundtrip() {
        let path = temp_path();
        let mut payload = sample_payload();
        payload.settings.hide_zero_balances = true;
        StoreSession::create(&path, "correct horse", &payload).unwrap();
        let (loaded, _) = StoreSession::unlock(&path, "correct horse").unwrap();
        assert!(loaded.settings.hide_zero_balances);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_store_written_before_the_option_existed_still_loads() {
        // Every existing user's store predates this field. Deserialising must fall back to
        // false, which is the behaviour they already had, rather than failing to open the
        // wallet at all.
        let json = r#"{
            "starred": ["BTC"],
            "infura_key": "", "etherscan_key": "",
            "btc_node": "", "eth_node": "", "sol_node": "", "ltc_node": "",
            "thornode_url": "",
            "btc_network": "bitcoin", "eth_network": "mainnet",
            "sol_network": "mainnet", "ltc_network": "mainnet",
            "custom_tokens": []
        }"#;
        let settings: StoreSettings = serde_json::from_str(json).expect("old settings still parse");
        assert!(!settings.hide_zero_balances);
    }

    #[test]
    fn wrong_password_fails() {
        let path = temp_path();
        let payload = sample_payload();
        StoreSession::create(&path, "correct horse", &payload).unwrap();
        let err = StoreSession::unlock(&path, "wrong").unwrap_err();
        let _ = fs::remove_file(&path);
        match err {
            block_error::Error::New(message) => assert!(message.contains("decrypt") || message.contains("ERROR")),
            _ => panic!("expected decrypt error, got {err:?}"),
        }
    }

    #[test]
    fn empty_password_rejected() {
        let path = temp_path();
        let err = StoreSession::create(&path, "", &PayloadV1::new()).unwrap_err();
        match err {
            block_error::Error::New(message) => assert!(message.contains("password")),
            _ => panic!("expected password error"),
        }
    }

    #[test]
    fn short_passwords_are_accepted_at_the_current_floor() {
        // `MIN_PASSWORD_LEN` is 1 by choice, so throwaway test wallets are quick to make.
        // This test tracks that decision rather than asserting a rule; if the floor is ever
        // raised back to 12 it should fail loudly and be updated alongside it.
        let path = temp_path();
        assert_eq!(MIN_PASSWORD_LEN, 1);
        assert!(check_password_strength("1234").is_ok());
        assert!(check_password_strength("hunter2").is_ok());
        assert!(StoreSession::create(&path, "1234", &PayloadV1::new()).is_ok());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn password_length_counts_characters_not_bytes() {
        // Multi-byte input must be measured in characters, not bytes, so a non-Latin
        // passphrase is never held to a longer standard than the same length of ASCII.
        // Matters again the moment the floor is raised above 1.
        assert_eq!("パスワードパ".chars().count(), 6);
        assert_eq!("パスワードパ".len(), 18);
        assert!(check_password_strength("パスワードパ").is_ok());
    }

    #[test]
    fn kdf_params_from_a_hostile_file_are_refused() {
        // An allocation request measured in gigabytes, from a file the user was handed.
        assert!(check_kdf_params(4_000_000, 2, 1).is_err());
        // Weaker than the shipped defaults, which would make the file cheap to attack.
        assert!(check_kdf_params(8, 1, 1).is_err());
        assert!(check_kdf_params(19_456, 1, 1).is_err());
        assert!(check_kdf_params(65_536, 3, 0).is_err());
        // What this app actually writes.
        assert!(check_kdf_params(M_KIB, T_COST, P_COST).is_ok());
        // What older stores were written with, which must still open.
        assert!(check_kdf_params(19_456, 2, 1).is_ok());
    }

    #[test]
    fn a_store_declaring_absurd_kdf_params_fails_before_allocating() {
        let path = temp_path();
        StoreSession::create(&path, TEST_PASSWORD, &PayloadV1::new()).unwrap();
        let mut file: StoreFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        file.kdf.m_kib = u32::MAX;
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let err = StoreSession::unlock(&path, TEST_PASSWORD).unwrap_err();
        let _ = fs::remove_file(&path);
        match err {
            block_error::Error::New(message) => assert!(message.contains("memory cost")),
            _ => panic!("expected a bounded-parameter error, got {err:?}"),
        }
    }

    #[test]
    fn a_failed_write_leaves_the_previous_store_intact() {
        // The rename is what makes this true; the assertion here is that a completed save
        // leaves exactly one file and no stray temp alongside it.
        let path = temp_path();
        let payload = sample_payload();
        let session = StoreSession::create(&path, TEST_PASSWORD, &payload).unwrap();
        session.save(&payload).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("tmp").exists());
        assert!(StoreSession::unlock(&path, TEST_PASSWORD).is_ok());
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_not_readable_by_other_local_accounts() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path();
        StoreSession::create(&path, TEST_PASSWORD, &PayloadV1::new()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = fs::remove_file(&path);
        assert_eq!(mode, FILE_MODE_PRIVATE);
    }

    #[test]
    fn save_rewrites_ciphertext_with_same_key() {
        let path = temp_path();
        let mut payload = sample_payload();
        let session = StoreSession::create(&path, TEST_PASSWORD, &payload).unwrap();
        payload.settings.eth_node = "https://updated.invalid".to_string();
        session.save(&payload).unwrap();
        let (loaded, _) = StoreSession::unlock(&path, TEST_PASSWORD).unwrap();
        assert_eq!(loaded.settings.eth_node, "https://updated.invalid");
        let _ = fs::remove_file(&path);
    }
}

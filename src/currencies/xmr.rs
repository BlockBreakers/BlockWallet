use colored::*;
use core::{fmt, fmt::Display};
use fast_qr::convert::{image::ImageBuilder, Builder, Shape};
use fast_qr::qr::QRBuilder;
use serde::Serialize;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use bdk_wallet::bitcoin::bip32::{DerivationPath, Xpriv};
use bdk_wallet::bitcoin::secp256k1::Secp256k1;
use bdk_wallet::bitcoin::NetworkKind;
use bip39::Mnemonic;
use monero_wallet::primitives::keccak256;
use rand::RngCore;
use zeroize::Zeroizing;

use crate::configuration::*;
use crate::currencies::xmr_chain::{self, XmrHistoryItem, XmrNetwork};

/// The BIP44 account this wallet derives its Monero keys from.
///
/// Monero has no derivation standard of its own for BIP39 phrases: its native format is a
/// 25-word seed that is the private spend key itself, and every BIP39 wallet that added Monero
/// invented its own bridge. This one is the Ledger Monero app's, which is the most widely
/// deployed: a secp256k1 BIP32 derivation at the SLIP-44 Monero coin type, keccak256 of the
/// 32-byte private key, reduced mod the ed25519 group order to give the private spend key, and
/// the private view key derived from the spend key exactly as every Monero wallet does. Coin
/// type 128 is Monero's in SLIP-44; the trailing `/0/0` is Ledger's, kept so the two agree.
///
/// The path is fixed to account 0. Additional Monero accounts are not derived from the phrase,
/// because a second one would need a convention nothing else follows; import a spend key
/// instead.
pub const XMR_PATH: &str = "m/44'/128'/0'/0/0";

pub fn generate_xmr_hd_wallet() -> Option<MoneroWallet> {
    match MoneroWallet::new() {
        Ok(wallet) => Some(wallet),
        Err(_) => {
            crate::configuration::logging::error("monero wallet generation failed");
            None
        }
    }
}

pub fn generate_from_mnemonic(mnemonic: &str, passphrase: &str) -> Option<MoneroWallet> {
    match MoneroWallet::from_mnemonic(mnemonic, passphrase) {
        Ok(wallet) => Some(wallet),
        Err(_) => {
            crate::configuration::logging::error("monero wallet from mnemonic failed");
            None
        }
    }
}

/// Import from a private spend key. The view key is derived from it, as every deterministic
/// Monero wallet does, so the one hex string is a complete backup.
pub fn generate_from_private_key(spend_key_hex: &str, restore_height: Option<u64>) -> Option<MoneroWallet> {
    match MoneroWallet::from_private_key_on(spend_key_hex, XmrNetwork::Mainnet, restore_height) {
        Ok(wallet) => Some(wallet),
        Err(_) => {
            crate::configuration::logging::error("monero wallet from spend key failed");
            None
        }
    }
}

#[derive(Serialize, Debug, Default, Clone)]
pub struct MoneroWallet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mnemonic: Option<String>,
    /// Private spend key, hex. The secret that owns the funds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    /// Private view key, hex. Reveals incoming payments but cannot spend. Needed to scan, so
    /// it is held alongside the spend key rather than re-derived on every sync.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_view_key: Option<String>,
    /// Public spend key followed by public view key, hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Block to start scanning from, when the user has said so. Overrides `birthday`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_height: Option<u64>,
    /// Unix time before which this account certainly held nothing: set when the phrase is
    /// generated, so a new wallet scans only from its own creation rather than from genesis.
    /// A restored or imported account has none, and falls back to a recent window unless a
    /// restore height is given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday: Option<u64>,
    pub balance: Arc<Mutex<String>>,
    pub history: Arc<Mutex<Vec<XmrHistoryItem>>>,
}

impl MoneroWallet {
    pub fn new() -> Result<Self, block_error::Error> {
        let mut entropy = vec![0u8; 16];
        rand::thread_rng().fill_bytes(&mut entropy);
        let mnemonic = Mnemonic::from_entropy(&entropy)
            .map_err(|e| block_error::Error::new(format!("Mnemonic generation failed: {e:?}")))?;
        let mut wallet = Self::from_mnemonic(&mnemonic.to_string(), "")?;
        wallet.birthday = Some(xmr_chain::now_unix());
        Ok(wallet)
    }

    pub fn from_mnemonic(mnemonic: &str, passphrase: &str) -> Result<Self, block_error::Error> {
        Self::from_mnemonic_on(mnemonic, passphrase, XmrNetwork::Mainnet)
    }

    pub fn from_mnemonic_on(mnemonic: &str, passphrase: &str, network: XmrNetwork) -> Result<Self, block_error::Error> {
        let parsed = Mnemonic::parse_normalized(mnemonic)
            .map_err(|e| block_error::Error::new(format!("Invalid mnemonic: {e:?}")))?;
        let seed = parsed.to_seed(passphrase);
        let secp = Secp256k1::new();
        // As in ltc.rs: NetworkKind::Main only satisfies Xpriv's type; it plays no part in the
        // derived bytes. Nothing Bitcoin-shaped survives past the next line.
        let master = Xpriv::new_master(NetworkKind::Main, &seed)
            .map_err(|e| block_error::Error::new(format!("BIP32 master key failed: {e:?}")))?;
        let path = DerivationPath::from_str(XMR_PATH)
            .map_err(|e| block_error::Error::new(format!("Invalid derivation path: {e:?}")))?;
        let derived = master
            .derive_priv(&secp, &path)
            .map_err(|e| block_error::Error::new(format!("BIP32 derive failed: {e:?}")))?;

        // Ledger's bridge from a secp256k1 key to Monero's ed25519 scalar: hash, then reduce.
        let raw = Zeroizing::new(derived.private_key.secret_bytes());
        let spend = Zeroizing::new(xmr_chain::reduce_to_scalar(keccak256(raw.as_slice())));

        let mut wallet = wallet_from_spend_key(&spend, network, None)?;
        wallet.mnemonic = Some(mnemonic.to_string());
        wallet.path = Some(XMR_PATH.to_string());
        if !passphrase.is_empty() {
            wallet.password = Some(passphrase.to_string());
        }
        Ok(wallet)
    }

    pub fn from_private_key_on(
        spend_key_hex: &str,
        network: XmrNetwork,
        restore_height: Option<u64>,
    ) -> Result<Self, block_error::Error> {
        let spend = xmr_chain::scalar_from_hex(spend_key_hex.trim())?;
        wallet_from_spend_key(&spend, network, restore_height)
    }

    pub fn set_wallet_name(&mut self, name: String) {
        self.wallet_name = Some(name);
    }

    pub fn wipe_secrets(&mut self) {
        crate::configuration::secrets::wipe_optional_string(&mut self.mnemonic);
        crate::configuration::secrets::wipe_optional_string(&mut self.password);
        crate::configuration::secrets::wipe_optional_string(&mut self.private_key);
        crate::configuration::secrets::wipe_optional_string(&mut self.private_view_key);
        crate::configuration::secrets::wipe_optional_string(&mut self.public_key);
    }
}

/// Same reasoning as the other wallet types: every clone carries its own copy of the keys,
/// and only the copy the app holds is reached by the lock button.
impl Drop for MoneroWallet {
    fn drop(&mut self) {
        self.wipe_secrets();
    }
}

impl MoneroWallet {
    pub fn generate_qr_address(&self) -> Result<gdk4::Texture, block_error::Error> {
        let address = self.address.as_deref().unwrap_or("");
        if address.is_empty() {
            return Err(block_error::Error::new("no receive address for QR".to_string()));
        }
        let qrcode = QRBuilder::new(address.to_string())
            .build()
            .map_err(|e| block_error::Error::new(format!("QR encode failed: {e:?}")))?;
        let img = ImageBuilder::default()
            .shape(Shape::RoundedSquare)
            .fit_width(300)
            .to_pixmap(&qrcode);
        let encoded_png = match img.encode_png() {
            Ok(png) => png,
            Err(e) => return Err(block_error::Error::IOError(e.into())),
        };
        let texture = gdk4::Texture::from_bytes(&glib::Bytes::from(&encoded_png))?;
        Ok(texture)
    }
}

/// Builds the account from a private spend key. The view key is `H(spend)` reduced, which is
/// what `monero-wallet-cli` and every GUI derive for a "deterministic" wallet, so an account
/// restored elsewhere from this spend key alone sees the same view key and the same address.
fn wallet_from_spend_key(
    spend: &Zeroizing<[u8; 32]>,
    network: XmrNetwork,
    restore_height: Option<u64>,
) -> Result<MoneroWallet, block_error::Error> {
    let view = Zeroizing::new(xmr_chain::reduce_to_scalar(keccak256(spend.as_slice())));
    let keys = xmr_chain::AccountKeys::new(spend, &view)?;
    let address = keys.address(network);
    let (spend_pub, view_pub) = keys.public_keys();

    // Field by field: the zeroizing `Drop` on this type rules out struct-update syntax.
    let mut wallet = MoneroWallet::default();
    wallet.private_key = Some(hex::encode(spend.as_slice()));
    wallet.private_view_key = Some(hex::encode(view.as_slice()));
    wallet.public_key = Some(format!("{}{}", hex::encode(spend_pub), hex::encode(view_pub)));
    wallet.address = Some(address);
    wallet.network = Some(xmr_chain::network_name(network).to_string());
    wallet.restore_height = restore_height;
    wallet.balance = Arc::new(Mutex::new(String::from("Uninitialized")));
    wallet.history = Arc::new(Mutex::new(Vec::new()));
    Ok(wallet)
}

#[cfg_attr(tarpaulin, skip)]
impl Display for MoneroWallet {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let output = [
            match &self.wallet_name {
                Some(wallet_name) => format!("      {}          {}\n", "Wallet Name".cyan().bold(), wallet_name),
                _ => "".to_owned(),
            },
            match &self.path {
                Some(path) => format!("      {}                 {}\n", "Path".cyan().bold(), path),
                _ => "".to_owned(),
            },
            match &self.address {
                Some(address) => format!("      {}              {}\n", "Address".cyan().bold(), address),
                _ => "".to_owned(),
            },
            match &self.network {
                Some(network) => format!("      {}              {}\n", "Network".cyan().bold(), network),
                _ => "".to_owned(),
            },
        ]
        .concat();
        let output = output[..output.len() - 1].to_owned();
        write!(f, "\n{}", output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    /// Pinned so the derivation cannot drift silently: a change here would move every
    /// user's funds to a different address on the next upgrade. Recomputed independently
    /// in a scratch crate from the scheme in the `XMR_PATH` doc comment.
    const ABANDON_SPEND: &str = "3b094ca7218f175e91fa2402b4ae239a2fe8262792a3e718533a1a357a1e4109";
    const ABANDON_VIEW: &str = "0f3fe25d0c6d4c94dde0c0bcc214b233e9c72927f813728b0f01f28f9d5e1201";
    const ABANDON_ADDRESS: &str =
        "49vDbkSo7eve3J41sBdjvjaBUyz8qHohsQcGtRf63qEUTMBvmA45fpp5pSacMdSg7A3b71RejLzB8EkGbfjp5PELVF2N4Zn";

    #[test]
    fn generate_from_known_mnemonic_matches_pinned_vector() {
        let wallet = generate_from_mnemonic(ABANDON, "").unwrap();
        assert_eq!(wallet.private_key.as_deref(), Some(ABANDON_SPEND));
        assert_eq!(wallet.private_view_key.as_deref(), Some(ABANDON_VIEW));
        assert_eq!(wallet.address.as_deref(), Some(ABANDON_ADDRESS));
        assert_eq!(wallet.path.as_deref(), Some(XMR_PATH));
        assert_eq!(wallet.network.as_deref(), Some("monero"));
        assert!(wallet.birthday.is_none(), "a restored phrase has no known birthday");
    }

    #[test]
    fn stagenet_reencodes_the_same_keys() {
        let mainnet = MoneroWallet::from_mnemonic_on(ABANDON, "", XmrNetwork::Mainnet).unwrap();
        let stagenet = MoneroWallet::from_mnemonic_on(ABANDON, "", XmrNetwork::Stagenet).unwrap();
        assert_eq!(mainnet.private_key, stagenet.private_key);
        assert_ne!(mainnet.address, stagenet.address);
        assert!(stagenet.address.as_deref().unwrap().starts_with('5'), "{:?}", stagenet.address);
        assert_eq!(stagenet.network.as_deref(), Some("stagenet"));
    }

    #[test]
    fn a_new_wallet_records_its_birthday() {
        let created = generate_xmr_hd_wallet().unwrap();
        assert!(created.birthday.is_some());
        let phrase = created.mnemonic.clone().unwrap();
        let restored = MoneroWallet::from_mnemonic(&phrase, "").unwrap();
        assert_eq!(created.address, restored.address);
        assert_eq!(created.private_key, restored.private_key);
    }

    #[test]
    fn spend_key_import_roundtrips_address_and_view_key() {
        let generated = generate_from_mnemonic(ABANDON, "").unwrap();
        let wallet = generate_from_private_key(ABANDON_SPEND, Some(3_000_000)).unwrap();
        assert_eq!(wallet.address, generated.address);
        assert_eq!(wallet.private_view_key, generated.private_view_key);
        assert_eq!(wallet.restore_height, Some(3_000_000));
        assert!(wallet.mnemonic.is_none());
        assert!(generate_from_private_key("not hex", None).is_none());
        assert!(generate_from_private_key("00", None).is_none());
    }

    #[test]
    fn passphrase_changes_the_address() {
        let without = generate_from_mnemonic(ABANDON, "").unwrap();
        let with = generate_from_mnemonic(ABANDON, "trezor").unwrap();
        assert_ne!(without.address, with.address);
        assert_eq!(with.password.as_deref(), Some("trezor"));
    }

    #[test]
    fn wipe_secrets_clears_key_material_and_keeps_address() {
        let mut wallet = MoneroWallet::from_mnemonic(ABANDON, "").unwrap();
        let address = wallet.address.clone();
        wallet.wipe_secrets();
        assert!(wallet.mnemonic.is_none());
        assert!(wallet.private_key.is_none());
        assert!(wallet.private_view_key.is_none());
        assert!(wallet.public_key.is_none());
        assert_eq!(wallet.address, address);
    }

    #[test]
    fn display_omits_mnemonic_and_keys() {
        let wallet = MoneroWallet::from_mnemonic(ABANDON, "").unwrap();
        let rendered = format!("{wallet}");
        assert!(!rendered.contains("abandon"));
        assert!(!rendered.contains(ABANDON_SPEND));
        assert!(!rendered.contains(ABANDON_VIEW));
        assert!(rendered.contains(ABANDON_ADDRESS));
    }
}

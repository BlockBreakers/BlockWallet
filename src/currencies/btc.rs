use colored::*;
use core::{fmt, fmt::Display};
use serde::Serialize;

use fast_qr::convert::{image::ImageBuilder, Builder, Shape};
use fast_qr::qr::QRBuilder;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use bdk_wallet::bitcoin::bip32::{DerivationPath, Xpriv};
use bdk_wallet::bitcoin::key::CompressedPublicKey;
use bdk_wallet::bitcoin::secp256k1::Secp256k1;
use bdk_wallet::bitcoin::{Address, Network, NetworkKind, PrivateKey};
use crate::currencies::btc_chain::{self, BtcHistoryItem};
use bdk_wallet::keys::bip39::{Language, Mnemonic, WordCount};
use bdk_wallet::keys::{DerivableKey, ExtendedKey, GeneratableKey, GeneratedKey};
use bdk_wallet::miniscript::Segwitv0;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, Wallet};

use crate::configuration::*;

fn bip84_account_path(network: Network) -> &'static str {
    match network {
        Network::Bitcoin => "m/84'/0'/0'",
        _ => "m/84'/1'/0'",
    }
}

fn bip84_receive_path(network: Network) -> &'static str {
    match network {
        Network::Bitcoin => "m/84'/0'/0'/0/0",
        _ => "m/84'/1'/0'/0/0",
    }
}

pub fn generate_from_private_key(private_key: &str) -> Option<BitcoinWallet> {
    match BitcoinWallet::from_private_key(private_key) {
        Ok(btc_wallet) => return Some(btc_wallet),
        Err(_) => {
            crate::configuration::logging::error("bitcoin wallet from WIF failed");
            return None
        }
    }
}

pub fn generate_from_mnemonic(mnemonic: &str, passphrase: &str) -> Option<BitcoinWallet> {
    match BitcoinWallet::from_mnemonic(mnemonic, passphrase) {
        Ok(btc_wallet) => Some(btc_wallet),
        Err(_) => {
            crate::configuration::logging::error("bitcoin wallet from mnemonic failed");
            None
        }
    }
}

pub fn generate_from_extended_private_key(extended_private_key: &str, _path: &str) -> Option<BitcoinWallet> {
    match BitcoinWallet::from_extended_private_key(extended_private_key) {
        Ok(btc_wallet) => Some(btc_wallet),
        Err(_) => {
            crate::configuration::logging::error("bitcoin wallet from extended key failed");
            None
        }
    }
}

#[derive(Serialize, Debug, Default, Clone)]
pub struct BitcoinWallet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mnemonic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    pub balance: Arc<Mutex<String>>,
    pub history: Arc<Mutex<Vec<BtcHistoryItem>>>,
}

impl BitcoinWallet {
    pub fn new() -> Result<Self, block_error::Error> {
        let mnemonic: GeneratedKey<_, Segwitv0> =
            Mnemonic::generate((WordCount::Words12, Language::English)).map_err(|e| {
                block_error::Error::new(format!("BDK mnemonic generation failed: {:?}", e))
            })?;
        Self::from_mnemonic(&mnemonic.to_string(), "")
    }

    pub fn from_mnemonic(mnemonic: &str, passphrase: &str) -> Result<Self, block_error::Error> {
        Self::from_mnemonic_on(mnemonic, passphrase, Network::Bitcoin)
    }

    pub fn from_mnemonic_on(
        mnemonic: &str,
        passphrase: &str,
        network: Network,
    ) -> Result<Self, block_error::Error> {
        let mnemonic = Mnemonic::parse_in(Language::English, mnemonic).map_err(|e| {
            block_error::Error::new(format!("Invalid mnemonic: {:?}", e))
        })?;
        let xprv = mnemonic_to_xprv(&mnemonic, passphrase, network)?;
        wallet_from_bip84_xprv(xprv, network, Some(mnemonic.to_string()), passphrase)
    }

    pub fn from_extended_private_key(extended_private_key: &str) -> Result<Self, block_error::Error> {
        let xprv = Xpriv::from_str(extended_private_key).map_err(|e| {
            block_error::Error::new(format!("Invalid extended private key: {:?}", e))
        })?;
        let network = network_from_kind(xprv.network);
        wallet_from_bip84_xprv(xprv, network, None, "")
    }

    pub fn from_private_key(pk: &str) -> Result<Self, block_error::Error> {
        let secp = Secp256k1::new();
        let private_key = PrivateKey::from_wif(pk).map_err(|e| {
            block_error::Error::new(format!("Invalid WIF private key: {:?}", e))
        })?;
        let public_key = private_key.public_key(&secp);
        let compressed = CompressedPublicKey::try_from(public_key).map_err(|e| {
            block_error::Error::new(format!("WIF key is not compressed (required for BIP84 wpkh): {:?}", e))
        })?;
        let network = network_from_kind(private_key.network);
        let address = Address::p2wpkh(&compressed, network);

        let descriptor = format!("wpkh({})", pk);
        let _wallet = Wallet::create_single(descriptor)
            .network(network)
            .create_wallet_no_persist()
            .map_err(|e| block_error::Error::new(format!("BDK wallet from WIF failed: {:?}", e)))?;

        // Built field by field rather than with `..Default::default()`: this type has a
        // zeroizing `Drop`, and struct-update syntax would have to move out of a temporary
        // that cannot be dropped piecewise.
        let mut wallet = Self::default();
        wallet.private_key = Some(private_key.to_wif());
        wallet.public_key = Some(public_key.to_string());
        wallet.address = Some(address.to_string());
        wallet.network = Some(format!("{}", network));
        wallet.compressed = Some(private_key.compressed);
        wallet.path = Some(String::from("wpkh(WIF)"));
        wallet.balance = Arc::new(Mutex::new(String::from("Uninitialized")));
        wallet.history = Arc::new(Mutex::new(Vec::new()));
        Ok(wallet)
    }

    /// The phrase when the account has one, otherwise its imported WIF. `None` once locked.
    pub fn signing_key(&self) -> Option<btc_chain::BtcKey> {
        if let Some(mnemonic) = self.mnemonic.clone().filter(|value| !value.is_empty()) {
            return Some(btc_chain::BtcKey::Seed {
                mnemonic,
                passphrase: self.password.clone().unwrap_or_default(),
            });
        }
        self.private_key
            .clone()
            .filter(|value| !value.is_empty())
            .map(btc_chain::BtcKey::Wif)
    }

    pub fn set_wallet_name(&mut self, name: String) {
        self.wallet_name = Some(name);
    }

    pub fn wipe_secrets(&mut self) {
        crate::configuration::secrets::wipe_optional_string(&mut self.mnemonic);
        crate::configuration::secrets::wipe_optional_string(&mut self.password);
        crate::configuration::secrets::wipe_optional_string(&mut self.private_key);
        crate::configuration::secrets::wipe_optional_string(&mut self.public_key);
        crate::configuration::secrets::wipe_optional_string(&mut self.transaction_hex);
    }

}

/// Every clone of a wallet carries its own copy of the mnemonic and private key in freshly
/// allocated `String`s. `lock_store` only reaches the copy the app holds, so without this any
/// other clone — the snapshot a send screen is built from, a temporary passed by value — would
/// hand its plaintext back to the allocator intact, and from there potentially to swap.
impl Drop for BitcoinWallet {
    fn drop(&mut self) {
        self.wipe_secrets();
    }
}

impl BitcoinWallet {
    pub fn generate_qr_address(&self) -> Result<gdk4::Texture, block_error::Error> {
        let address = match &self.address {
            Some(addr) => addr,
            None => ""
        };

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
            Err(e)  => return Err(block_error::Error::IOError(e.into()))
        };

        let texture = gdk4::Texture::from_bytes(&glib::Bytes::from(&encoded_png))?;
        Ok(texture)
    }
}

fn mnemonic_to_xprv(mnemonic: &Mnemonic, passphrase: &str, network: Network) -> Result<Xpriv, block_error::Error> {
    let passphrase = if passphrase.is_empty() { None } else { Some(passphrase.to_string()) };
    let xkey: ExtendedKey = (mnemonic.clone(), passphrase)
        .into_extended_key()
        .map_err(|e| block_error::Error::new(format!("BDK extended key failed: {:?}", e)))?;
    xkey.into_xprv(network).ok_or_else(|| {
        block_error::Error::new("BDK could not produce an xprv from the mnemonic".to_string())
    })
}

fn wallet_from_bip84_xprv(
    xprv: Xpriv,
    network: Network,
    mnemonic: Option<String>,
    passphrase: &str,
) -> Result<BitcoinWallet, block_error::Error> {
    let wallet = Wallet::create(
        Bip84(xprv, KeychainKind::External),
        Bip84(xprv, KeychainKind::Internal),
    )
    .network(network)
    .create_wallet_no_persist()
    .map_err(|e| block_error::Error::new(format!("BDK BIP84 wallet create failed: {:?}", e)))?;

    let address = wallet.peek_address(KeychainKind::External, 0).address;

    let secp = Secp256k1::new();
    let receive_path = DerivationPath::from_str(bip84_receive_path(network)).map_err(|e| {
        block_error::Error::new(format!("Invalid BIP84 path: {:?}", e))
    })?;
    let derived = xprv.derive_priv(&secp, &receive_path).map_err(|e| {
        block_error::Error::new(format!("BIP84 derive failed: {:?}", e))
    })?;
    let private_key = PrivateKey::new(derived.private_key, network);
    let public_key = private_key.public_key(&secp);

    let mut wallet = BitcoinWallet::default();
    wallet.mnemonic = mnemonic;
    wallet.password = if passphrase.is_empty() { None } else { Some(passphrase.to_string()) };
    wallet.private_key = Some(private_key.to_wif());
    wallet.public_key = Some(public_key.to_string());
    wallet.address = Some(address.to_string());
    wallet.network = Some(format!("{}", network));
    wallet.compressed = Some(true);
    wallet.path = Some(String::from(bip84_account_path(network)));
    wallet.balance = Arc::new(Mutex::new(String::from("Uninitialized")));
    wallet.history = Arc::new(Mutex::new(Vec::new()));
    Ok(wallet)
}

fn network_from_kind(kind: NetworkKind) -> Network {
    match kind {
        NetworkKind::Main => Network::Bitcoin,
        NetworkKind::Test => Network::Testnet,
    }
}

#[cfg_attr(tarpaulin, skip)]
impl Display for BitcoinWallet {
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
            match &self.compressed {
                Some(compressed) => format!("      {}           {}\n", "Compressed".cyan().bold(), compressed),
                _ => "".to_owned(),
            },
            match &self.transaction_id {
                Some(transaction_id) => format!("      {}       {}\n", "Transaction Id".cyan().bold(), transaction_id),
                _ => "".to_owned(),
            },
        ]
        .concat();

        // Removes final new line character
        let output = output[..output.len() - 1].to_owned();
        write!(f, "\n{}", output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_wallet_is_bip84_native_segwit() {
        let wallet = BitcoinWallet::new().expect("BDK wallet");
        let address = wallet.address.clone().expect("address");
        assert!(address.starts_with("bc1q"), "{address}");
        assert_eq!(wallet.path.as_deref(), Some("m/84'/0'/0'"));
        assert!(wallet.mnemonic.is_some());
    }

    #[test]
    fn testnet_mnemonic_derives_tb1_address() {
        let wallet = BitcoinWallet::from_mnemonic_on(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "",
            Network::Testnet,
        )
        .unwrap();
        let address = wallet.address.clone().unwrap();
        assert!(address.starts_with("tb1q"), "{address}");
        assert_eq!(wallet.path.as_deref(), Some("m/84'/1'/0'"));
    }

    #[test]
    fn mnemonic_restore_matches() {
        let created = BitcoinWallet::new().expect("BDK wallet");
        let phrase = created.mnemonic.clone().unwrap();
        let restored = BitcoinWallet::from_mnemonic(&phrase, "").expect("restore");
        assert_eq!(created.address, restored.address);
        assert_eq!(created.private_key, restored.private_key);
    }

    #[test]
    fn generate_from_known_mnemonic_is_bip84() {
        let wallet = generate_from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "",
        )
        .unwrap();
        assert!(wallet.mnemonic.is_some());
        let address = wallet.address.clone().unwrap();
        assert!(address.starts_with("bc1q"), "BIP84 address should be native segwit: {address}");
        assert_eq!(wallet.path.as_deref(), Some("m/84'/0'/0'"));
    }

    #[test]
    fn wipe_secrets_clears_key_material_and_keeps_address() {
        let mut wallet = BitcoinWallet::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "",
        )
        .unwrap();
        let address = wallet.address.clone();
        assert!(wallet.mnemonic.is_some());
        assert!(wallet.private_key.is_some());
        wallet.wipe_secrets();
        assert!(wallet.mnemonic.is_none());
        assert!(wallet.private_key.is_none());
        assert!(wallet.password.is_none());
        assert!(wallet.public_key.is_none());
        assert_eq!(wallet.address, address);
    }

    #[test]
    fn display_omits_mnemonic_and_private_key() {
        let wallet = BitcoinWallet::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "",
        )
        .unwrap();
        let key = wallet.private_key.clone().unwrap();
        let rendered = format!("{wallet}");
        assert!(!rendered.contains("abandon"));
        assert!(!rendered.contains(&key));
        assert!(rendered.contains("bc1q"));
    }

    #[test]
    fn generate_from_wif_roundtrips_address() {
        let generated = BitcoinWallet::new().unwrap();
        let wif = generated.private_key.clone().unwrap();
        let wallet = generate_from_private_key(&wif).unwrap();
        assert_eq!(wallet.address, generated.address);
        assert!(wallet.public_key.is_some());
    }
}

use bip39::Mnemonic;
use rand::RngCore;

use crate::configuration::block_error;
use crate::currencies::btc::BitcoinWallet;
use crate::currencies::eth::EthereumWallet;
use crate::currencies::ltc::LitecoinWallet;
use crate::currencies::ltc_chain::LtcNetwork;
use crate::currencies::sol::SolanaWallet;
use crate::currencies::xmr::MoneroWallet;
use crate::currencies::xmr_chain::XmrNetwork;

pub const BTC_PASSPHRASE: &str = "";
pub const ETH_PATH: &str = "m/44'/60'/0'/0/0";
pub const SOL_PATH: &str = "m/44'/501'/0'/0'";

pub fn generate_mnemonic() -> Result<String, block_error::Error> {
    generate_mnemonic_words(12)
}

pub fn generate_mnemonic_words(word_count: u8) -> Result<String, block_error::Error> {
    let entropy_len = match word_count {
        12 => 16,
        24 => 32,
        _ => {
            return Err(block_error::Error::new(format!(
                "unsupported word count {word_count}; use 12 or 24"
            )))
        }
    };
    let mut entropy = vec![0u8; entropy_len];
    rand::thread_rng().fill_bytes(&mut entropy);
    let mnemonic = Mnemonic::from_entropy(&entropy)
        .map_err(|e| block_error::Error::new(format!("seed generation failed: {e}")))?;
    Ok(mnemonic.to_string())
}

pub fn parse_mnemonic(phrase: &str) -> Result<String, block_error::Error> {
    let mnemonic = Mnemonic::parse_normalized(phrase.trim())
        .map_err(|e| block_error::Error::new(format!("invalid mnemonic: {e}")))?;
    Ok(mnemonic.to_string())
}

pub fn bitcoin_from_seed(mnemonic: &str, passphrase: &str, name: &str) -> Result<BitcoinWallet, block_error::Error> {
    bitcoin_from_seed_on(mnemonic, passphrase, name, bdk_wallet::bitcoin::Network::Bitcoin)
}

pub fn bitcoin_from_seed_on(
    mnemonic: &str,
    passphrase: &str,
    name: &str,
    network: bdk_wallet::bitcoin::Network,
) -> Result<BitcoinWallet, block_error::Error> {
    let mut wallet = BitcoinWallet::from_mnemonic_on(mnemonic, passphrase, network)?;
    if !name.is_empty() {
        wallet.set_wallet_name(name.to_string());
    }
    Ok(wallet)
}

pub fn ethereum_from_seed(
    mnemonic: &str,
    path: &str,
    passphrase: &str,
    name: &str,
) -> Result<EthereumWallet, block_error::Error> {
    let path = if path.is_empty() { ETH_PATH } else { path };
    let mut wallet = EthereumWallet::from_mnemonic(mnemonic, path, passphrase)?;
    if !name.is_empty() {
        wallet.set_wallet_name(name.to_string());
    }
    Ok(wallet)
}

pub fn solana_from_seed(mnemonic: &str, passphrase: &str, name: &str) -> Result<SolanaWallet, block_error::Error> {
    let mut wallet = SolanaWallet::from_mnemonic(mnemonic, SOL_PATH, passphrase)?;
    if !name.is_empty() {
        wallet.set_wallet_name(name.to_string());
    }
    Ok(wallet)
}

pub fn litecoin_from_seed(mnemonic: &str, passphrase: &str, name: &str) -> Result<LitecoinWallet, block_error::Error> {
    litecoin_from_seed_on(mnemonic, passphrase, name, LtcNetwork::Mainnet)
}

pub fn litecoin_from_seed_on(
    mnemonic: &str,
    passphrase: &str,
    name: &str,
    network: LtcNetwork,
) -> Result<LitecoinWallet, block_error::Error> {
    let mut wallet = LitecoinWallet::from_mnemonic_on(mnemonic, passphrase, network)?;
    if !name.is_empty() {
        wallet.set_wallet_name(name.to_string());
    }
    Ok(wallet)
}

pub fn monero_from_seed(mnemonic: &str, passphrase: &str, name: &str) -> Result<MoneroWallet, block_error::Error> {
    monero_from_seed_on(mnemonic, passphrase, name, XmrNetwork::Mainnet)
}

pub fn monero_from_seed_on(
    mnemonic: &str,
    passphrase: &str,
    name: &str,
    network: XmrNetwork,
) -> Result<MoneroWallet, block_error::Error> {
    let mut wallet = MoneroWallet::from_mnemonic_on(mnemonic, passphrase, network)?;
    if !name.is_empty() {
        wallet.set_wallet_name(name.to_string());
    }
    Ok(wallet)
}

pub fn accounts_from_seed(
    mnemonic: &str,
    passphrase: &str,
) -> Result<(BitcoinWallet, EthereumWallet, SolanaWallet, LitecoinWallet, MoneroWallet), block_error::Error> {
    accounts_from_seed_on(mnemonic, passphrase, bdk_wallet::bitcoin::Network::Bitcoin)
}

pub fn accounts_from_seed_on(
    mnemonic: &str,
    passphrase: &str,
    network: bdk_wallet::bitcoin::Network,
) -> Result<(BitcoinWallet, EthereumWallet, SolanaWallet, LitecoinWallet, MoneroWallet), block_error::Error> {
    let phrase = parse_mnemonic(mnemonic)?;
    let btc = bitcoin_from_seed_on(&phrase, passphrase, "Bitcoin", network)?;
    let eth = ethereum_from_seed(&phrase, ETH_PATH, passphrase, "Ethereum")?;
    let sol = solana_from_seed(&phrase, passphrase, "Solana")?;
    let ltc_network = match network {
        bdk_wallet::bitcoin::Network::Bitcoin => LtcNetwork::Mainnet,
        _ => LtcNetwork::Testnet,
    };
    let ltc = litecoin_from_seed_on(&phrase, passphrase, "Litecoin", ltc_network)?;
    let xmr_network = match network {
        bdk_wallet::bitcoin::Network::Bitcoin => XmrNetwork::Mainnet,
        _ => XmrNetwork::Stagenet,
    };
    let xmr = monero_from_seed_on(&phrase, passphrase, "Monero", xmr_network)?;
    Ok((btc, eth, sol, ltc, xmr))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn generated_seed_derives_btc_eth_sol_ltc_and_xmr() {
        let phrase = generate_mnemonic().unwrap();
        let (btc, eth, sol, ltc, xmr) = accounts_from_seed(&phrase, BTC_PASSPHRASE).unwrap();
        assert_eq!(xmr.mnemonic.as_deref(), Some(phrase.as_str()));
        assert!(xmr.address.as_ref().unwrap().starts_with('4'));
        assert_eq!(btc.mnemonic.as_deref(), Some(phrase.as_str()));
        assert_eq!(eth.mnemonic.as_deref(), Some(phrase.as_str()));
        assert_eq!(sol.mnemonic.as_deref(), Some(phrase.as_str()));
        assert_eq!(ltc.mnemonic.as_deref(), Some(phrase.as_str()));
        assert!(btc.address.as_ref().unwrap().starts_with("bc1q"));
        let eth_address = eth.address.as_ref().unwrap();
        assert!(eth_address.starts_with("0x"));
        assert_eq!(eth_address.len(), 42);
        assert_eq!(eth.path.as_deref(), Some(ETH_PATH));
        assert_eq!(sol.path.as_deref(), Some(SOL_PATH));
        assert!(!sol.address.as_ref().unwrap().is_empty());
        assert!(ltc.address.as_ref().unwrap().starts_with("ltc1q"));
    }

    #[test]
    fn known_mnemonic_is_shared_across_chains() {
        let (btc, eth, sol, ltc, xmr) = accounts_from_seed(ABANDON, "").unwrap();
        assert_eq!(btc.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(xmr.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(
            xmr.address.as_deref(),
            Some("49vDbkSo7eve3J41sBdjvjaBUyz8qHohsQcGtRf63qEUTMBvmA45fpp5pSacMdSg7A3b71RejLzB8EkGbfjp5PELVF2N4Zn")
        );
        assert_eq!(eth.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(sol.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(ltc.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(
            btc.address.as_deref(),
            Some("bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu")
        );
        assert_eq!(
            eth.address.as_deref(),
            Some("0x9858EfFD232B4033E47d90003D41EC34EcaEda94")
        );
        assert_eq!(eth.path.as_deref(), Some(ETH_PATH));
        assert_eq!(sol.path.as_deref(), Some(SOL_PATH));
        let again = accounts_from_seed(ABANDON, "").unwrap();
        assert_eq!(btc.address, again.0.address);
        assert_eq!(eth.address, again.1.address);
        assert_eq!(sol.address, again.2.address);
        assert_eq!(ltc.address, again.3.address);
        assert_eq!(xmr.address, again.4.address);
    }

    #[test]
    fn invalid_mnemonic_is_rejected() {
        assert!(parse_mnemonic("not a mnemonic").is_err());
        assert!(accounts_from_seed("not a mnemonic", "").is_err());
    }

    #[test]
    fn generate_mnemonic_words_supports_12_and_24() {
        let twelve = generate_mnemonic_words(12).unwrap();
        let twenty_four = generate_mnemonic_words(24).unwrap();
        assert_eq!(twelve.split_whitespace().count(), 12);
        assert_eq!(twenty_four.split_whitespace().count(), 24);
        assert!(generate_mnemonic_words(15).is_err());
        let (btc, eth, sol, ltc, xmr) = accounts_from_seed(&twenty_four, "").unwrap();
        assert_eq!(xmr.mnemonic.as_deref(), Some(twenty_four.as_str()));
        assert_eq!(btc.mnemonic.as_deref(), Some(twenty_four.as_str()));
        assert_eq!(eth.mnemonic.as_deref(), Some(twenty_four.as_str()));
        assert_eq!(sol.mnemonic.as_deref(), Some(twenty_four.as_str()));
        assert_eq!(ltc.mnemonic.as_deref(), Some(twenty_four.as_str()));
    }

    #[test]
    fn bip39_passphrase_changes_all_addresses() {
        let without = accounts_from_seed(ABANDON, "").unwrap();
        let with = accounts_from_seed(ABANDON, "trezor").unwrap();
        assert_ne!(without.0.address, with.0.address);
        assert_ne!(without.1.address, with.1.address);
        assert_ne!(without.2.address, with.2.address);
        assert_ne!(without.3.address, with.3.address);
        assert_ne!(without.4.address, with.4.address);
        assert_eq!(with.0.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(with.1.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(with.2.mnemonic.as_deref(), Some(ABANDON));
        assert_eq!(with.3.mnemonic.as_deref(), Some(ABANDON));
        let again = accounts_from_seed(ABANDON, "trezor").unwrap();
        assert_eq!(with.0.address, again.0.address);
        assert_eq!(with.1.address, again.1.address);
        assert_eq!(with.2.address, again.2.address);
        assert_eq!(with.3.address, again.3.address);
        assert_eq!(with.4.address, again.4.address);
    }

    #[test]
    fn ltc_testnet_network_maps_to_tltc_path() {
        let (_, _, _, ltc, xmr) =
            accounts_from_seed_on(ABANDON, "", bdk_wallet::bitcoin::Network::Testnet).unwrap();
        assert!(ltc.address.as_ref().unwrap().starts_with("tltc1q"));
        assert_eq!(ltc.path.as_deref(), Some("m/84'/1'/0'/0/0"));
        assert!(xmr.address.as_ref().unwrap().starts_with('5'), "stagenet prefix");
        assert_eq!(xmr.network.as_deref(), Some("stagenet"));
    }
}

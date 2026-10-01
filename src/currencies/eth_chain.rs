use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{Filter, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use serde::{Deserialize, Serialize};

use crate::configuration::block_error;
use crate::currencies::fees::clamp_gas_price;
use crate::currencies::multicall;
use crate::currencies::tokens::Token;

const TRANSFER_TOPIC: B256 = B256::new([
    0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b, 0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d, 0xaa,
    0x95, 0x2b, 0xa7, 0xf1, 0x63, 0xc4, 0xa1, 0x16, 0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23, 0xb3, 0xef,
]);
const SELECTOR_BALANCE_OF: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
const SELECTOR_TRANSFER: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
const SELECTOR_DECIMALS: [u8; 4] = [0x31, 0x3c, 0xe5, 0x67];
const SELECTOR_SYMBOL: [u8; 4] = [0x95, 0xd8, 0x9b, 0x41];
const SELECTOR_NAME: [u8; 4] = [0x06, 0xfd, 0xde, 0x03];

const NATIVE_SENTINEL: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const LOG_LOOKBACK: u64 = 1_500;
const HISTORY_CAP: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EthNetwork {
    Mainnet,
    Sepolia,
    ArbitrumOne,
    Base,
    Optimism,
    PolygonPos,
    BnbSmartChain,
    AvalancheCChain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegistryToken {
    pub symbol: String,
    pub name: String,
    pub address: String,
    pub decimals: u8,
    pub native: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeeTiers {
    pub low: u128,
    pub medium: u128,
    pub high: u128,
    pub priority: u128,
}

impl Default for FeeTiers {
    fn default() -> Self {
        Self {
            low: 1_000_000_000,
            medium: 1_500_000_000,
            high: 2_500_000_000,
            priority: 100_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EthHistoryItem {
    pub txid: String,
    pub from: String,
    pub to: String,
    pub symbol: String,
    pub amount: String,
    pub incoming: bool,
    pub confirmations: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EthSyncState {
    pub eth_wei: U256,
    pub receive_address: String,
    pub erc20: BTreeMap<String, String>,
    pub history: Vec<EthHistoryItem>,
    pub offline: bool,
    pub native_symbol: String,
}

impl EthSyncState {
    pub fn balance_display(&self) -> String {
        if self.offline {
            return format!("{} {} (offline)", format_units_trimmed(self.eth_wei, 18), self.native_symbol);
        }
        format!("{} {}", format_units_trimmed(self.eth_wei, 18), self.native_symbol)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSend {
    /// The account this plan was built against, and whose nonce `nonce` holds.
    ///
    /// `sign_and_broadcast` refuses a key that resolves to anything else. Without this the
    /// UI's re-read of the account dropdown at confirm time could pair a plan with a
    /// different account's key: if the two happened to share a next-nonce — routine for
    /// freshly derived accounts, which both sit at 0 — the transaction would be perfectly
    /// valid and would debit an account the user never reviewed.
    pub from: String,
    pub to: String,
    pub token_symbol: String,
    pub token_address: Option<String>,
    pub amount: U256,
    pub amount_display: String,
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub fee_wei: U256,
    pub fee_symbol: String,
    pub chain_id: u64,
    pub nonce: u64,
}

impl PreparedSend {
    pub fn summary(&self) -> String {
        let fee = format_units_trimmed(self.fee_wei, 18);
        if self.token_address.is_none() {
            let total = self.amount.saturating_add(self.fee_wei);
            format!(
                "Network: chain {}\nTo: {}\nAmount: {} {}\nMax fee: {} {}\nTotal (amount + max fee): {} {}",
                self.chain_id,
                self.to,
                self.amount_display,
                self.fee_symbol,
                fee,
                self.fee_symbol,
                format_units_trimmed(total, 18),
                self.fee_symbol
            )
        } else {
            format!(
                "Network: chain {}\nTo: {}\nAmount: {} {}\nMax fee: {} {} (paid in {})",
                self.chain_id,
                self.to,
                self.amount_display,
                self.token_symbol,
                fee,
                self.fee_symbol,
                self.fee_symbol
            )
        }
    }
}

pub fn parse_network(name: &str) -> EthNetwork {
    match name.trim().to_ascii_lowercase().as_str() {
        "sepolia" | "testnet" | "test" => EthNetwork::Sepolia,
        "arbitrum" | "arbitrum-one" | "arb" => EthNetwork::ArbitrumOne,
        "base" => EthNetwork::Base,
        "optimism" | "op" | "op-mainnet" => EthNetwork::Optimism,
        "polygon" | "polygon-pos" | "matic" => EthNetwork::PolygonPos,
        "bsc" | "bnb" | "bnb-smart-chain" | "binance" => EthNetwork::BnbSmartChain,
        "avalanche" | "avax" | "avalanche-c-chain" => EthNetwork::AvalancheCChain,
        _ => EthNetwork::Mainnet,
    }
}

pub fn network_name(network: EthNetwork) -> &'static str {
    match network {
        EthNetwork::Sepolia => "sepolia",
        EthNetwork::Mainnet => "mainnet",
        EthNetwork::ArbitrumOne => "arbitrum",
        EthNetwork::Base => "base",
        EthNetwork::Optimism => "optimism",
        EthNetwork::PolygonPos => "polygon",
        EthNetwork::BnbSmartChain => "bsc",
        EthNetwork::AvalancheCChain => "avalanche",
    }
}

/// True only for known real testnets. Used to gate the "this spends real value" send
/// confirmation — anything that isn't a testnet (including every L2/sidechain) must show it.
pub fn is_testnet(network: EthNetwork) -> bool {
    matches!(network, EthNetwork::Sepolia)
}

pub fn chain_id(network: EthNetwork) -> u64 {
    match network {
        EthNetwork::Mainnet => 1,
        EthNetwork::Sepolia => 11155111,
        EthNetwork::ArbitrumOne => 42161,
        EthNetwork::Base => 8453,
        EthNetwork::Optimism => 10,
        EthNetwork::PolygonPos => 137,
        EthNetwork::BnbSmartChain => 56,
        EthNetwork::AvalancheCChain => 43114,
    }
}

/// Public endpoints used when the user has not supplied their own.
///
/// Health-checked with `eth_chainId` against each chain's declared id. Two were replaced after
/// going dead: `eth.llamarpc.com` returns HTTP 521, and `polygon-rpc.com` now answers 401 and
/// requires a key. Both had made their networks completely non-functional (no balance, no
/// history, no sending, no ENS) while looking like ordinary connectivity failures. publicnode
/// serves both and was already the Sepolia default.
///
/// Worth re-checking periodically: a dead default is indistinguishable from being offline
/// unless someone looks.
pub fn default_rpc(network: EthNetwork) -> &'static str {
    match network {
        EthNetwork::Mainnet => "https://ethereum-rpc.publicnode.com",
        EthNetwork::Sepolia => "https://ethereum-sepolia-rpc.publicnode.com",
        EthNetwork::ArbitrumOne => "https://arb1.arbitrum.io/rpc",
        EthNetwork::Base => "https://mainnet.base.org",
        EthNetwork::Optimism => "https://mainnet.optimism.io",
        EthNetwork::PolygonPos => "https://polygon-bor-rpc.publicnode.com",
        EthNetwork::BnbSmartChain => "https://bsc-dataseed.binance.org",
        EthNetwork::AvalancheCChain => "https://api.avax.network/ext/bc/C/rpc",
    }
}

/// The chain's native gas token symbol. Not always "ETH": L2s and sidechains with their own
/// native asset (Polygon, BNB Smart Chain, Avalanche C-Chain) use their own symbol.
///
/// Polygon's is POL, not MATIC. The token was renamed and this said MATIC until the on-chain
/// verification pass read `symbol()` from Polygon's own native-token predeploy and got back
/// POL. Note that MATIC still exists as a perfectly real bridged ERC-20 on mainnet, BSC and
/// Arbitrum, and those entries are correctly still called MATIC; it is only Polygon's gas
/// token that was renamed.
pub fn native_symbol(network: EthNetwork) -> &'static str {
    match network {
        EthNetwork::PolygonPos => "POL",
        EthNetwork::BnbSmartChain => "BNB",
        EthNetwork::AvalancheCChain => "AVAX",
        _ => "ETH",
    }
}

pub fn resolve_rpc(eth_node: &str, network: EthNetwork, infura_key: &str) -> String {
    let node = eth_node.trim();
    if !node.is_empty() {
        return node.trim_end_matches('/').to_string();
    }
    let key = infura_key.trim();
    if !key.is_empty() {
        // Infura's URL scheme isn't uniform across every L2 we support (and doesn't cover all
        // of them); only use it for the two networks it's verified for here, default RPC
        // otherwise.
        match network {
            EthNetwork::Mainnet => return format!("https://mainnet.infura.io/v3/{key}"),
            EthNetwork::Sepolia => return format!("https://sepolia.infura.io/v3/{key}"),
            _ => {}
        }
    }
    default_rpc(network).to_string()
}

pub fn bundled_tokens(network: EthNetwork) -> Vec<RegistryToken> {
    let symbol = native_symbol(network);
    let name = match network {
        EthNetwork::PolygonPos => "Polygon",
        EthNetwork::BnbSmartChain => "BNB",
        EthNetwork::AvalancheCChain => "Avalanche",
        _ => "Ethereum",
    };
    let mut tokens = vec![RegistryToken {
        symbol: symbol.into(),
        name: name.into(),
        address: NATIVE_SENTINEL.into(),
        decimals: 18,
        native: true,
    }];
    // Every entry below was verified on-chain before it was bundled: `symbol()` and
    // `decimals()` were read from the contract itself and had to agree with what goes in
    // here. The curated source list supplied candidate addresses and nothing more, which is
    // the right division of trust, and it earned its keep: the list had FLUX at 18 decimals
    // where all three of its contracts say 8, which would have misreported the balance by ten
    // orders of magnitude.
    //
    // Where a symbol differs from the commonly used one, the on-chain value wins and the name
    // says why. Most of those are Avalanche bridge assets, whose contracts genuinely report
    // `WETH.e`, `LINK.e` and so on: a user holding the bridged asset should see the bridged
    // symbol rather than be told they hold the native one. Two contracts report a symbol that
    // cannot be displayed at all (Arbitrum USDT0 uses a non-ASCII glyph, and Arbitrum's
    // bridged MKR reports a stringified bytes32), and those fall back to the conventional
    // symbol rather than being mangled into a plausible-looking but different string.
    //
    // Generated rather than hand-typed, then checked by
    // `every_bundled_token_list_is_internally_consistent`.
    match network {
        EthNetwork::Sepolia => {
            tokens.push(erc20(
                "USDC",
                "USD Coin",
                "0x1c7d4b196cb0c7b01d743fbc6116a902379c7238",
                6,
            ));
        }
        EthNetwork::Mainnet => {
            tokens.extend([
                erc20("1INCH", "1inch", "0x111111111117dC0aa78b770fA6A738034120C302", 18),
                erc20("AAVE", "Aave", "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9", 18),
                erc20("ALPHA", "Alpha Venture DAO", "0xa1faa113cbE53436Df28FF0aEe54275c13B40975", 18),
                erc20("ANKR", "Ankr", "0x8290333ceF9e6D528dD5618Fb97a76f268f3EDD4", 18),
                erc20("ARPA", "ARPA Chain", "0xBA50933C268F567BDC86E1aC131BE072C6B0b71a", 18),
                erc20("AUSD", "AUSD", "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", 6),
                erc20("AXL", "Axelar", "0x467719aD09025FcC6cF6F8311755809d45a5E5f3", 6),
                erc20("BAL", "Balancer", "0xba100000625a3754423978a60c9317c58a424e3D", 18),
                erc20("BUSD", "Binance USD", "0x4Fabb145d64652a948d72533023f6E7A623C7C53", 18),
                erc20("cbETH", "Coinbase Wrapped Staked ETH", "0xBe9895146f7AF43049ca1c1AE358B0541Ea49704", 18),
                erc20("COMP", "Compound", "0xc00e94Cb662C3520282E6f5717214004A7f26888", 18),
                erc20("CRV", "Curve DAO Token", "0xD533a949740bb3306d119CC777fa900bA034cd52", 18),
                erc20("CTSI", "Cartesi", "0x491604c0FDF08347Dd1fa4Ee062a822A5DD06B5D", 18),
                erc20("DAI", "Dai Stablecoin", "0x6B175474E89094C44Da98b954EedeAC495271d0F", 18),
                erc20("DRV", "Derive", "0xB1D1eae60EEA9525032a6DCb4c1CE336a1dE71BE", 18),
                erc20("ENS", "Ethereum Name Service", "0xC18360217D8F7Ab5e7c516566761Ea12Ce7F9D72", 18),
                erc20("EURA", "agEur (listed as AGEUR)", "0x1a7e4e63778B4f12a199C062f3eFdD288afCBce8", 18),
                erc20("EURC", "Euro Coin", "0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c", 6),
                erc20("FARM", "Harvest Finance", "0xa0246c9032bC3A600820415aE600c6388619A14D", 18),
                erc20("FET", "Fetch ai", "0xaea46A60368A7bD060eec7DF8CBa43b7EF41Ad85", 18),
                erc20("FLUX", "Flux", "0x720CD16b011b987Da3518fbf38c3071d4F0D1495", 8),
                erc20("FRAX", "Frax", "0x853d955aCEf822Db058eb8505911ED77F175b99e", 18),
                erc20("FXS", "Frax Share", "0x3432B6A60D23Ca0dFCa7761B7ab56459D9C964D0", 18),
                erc20("GRT", "The Graph", "0xc944E90C64B2c07662A292be6244BDf05Cda44a7", 18),
                erc20("KII", "Kiichain", "0xEEC6574eAbBa52bac3f0277F2cD5Ac7e67197886", 18),
                erc20("KRL", "KRYLL", "0x464eBE77c293E473B48cFe96dDCf88fcF7bFDAC0", 18),
                erc20("KUJI", "Kujira", "0x96543ef8d2C75C26387c1a319ae69c0BEE6f3fe7", 6),
                erc20("LDO", "Lido DAO", "0x5a98fcbea516cf06857215779fd812ca3bef1b32", 18),
                erc20("LINK", "ChainLink Token", "0x514910771AF9Ca656af840dff83E8264EcF986CA", 18),
                erc20("LRC", "LoopringCoin V2", "0xBBbbCA6A901c926F240b89EacB641d8Aec7AEafD", 18),
                erc20("MASK", "Mask Network", "0x69af81e73A73B40adF4f3d4223Cd9b1ECE623074", 18),
                erc20("MATIC", "Polygon", "0x7D1AfA7B718fb893dB30A3aBc0Cfc608AaCfeBB0", 18),
                erc20("MIM", "Magic Internet Money", "0x99D8a9C45b2ecA8864373A26D1459e3Dff1e17F3", 18),
                erc20("MKR", "Maker", "0x9f8F72aA9304c8B593d555F12eF6589cC3A579A2", 18),
                erc20("MULTI", "Multichain", "0x65Ef703f5594D2573eb71Aaf55BC0CB548492df4", 18),
                erc20("PENDLE", "Pendle", "0x808507121B80c02388fAd14726482e061B8da827", 18),
                erc20("PERP", "Perpetual Protocol", "0xbC396689893D065F41bc2C6EcbeE5e0085233447", 18),
                erc20("RAI", "Rai Reflex Index", "0x03ab458634910AaD20eF5f1C8ee96F1D6ac54919", 18),
                erc20("RPL", "Rocket Pool Protocol", "0xD33526068D116cE69F19A9ee46F0bd304F21A51f", 18),
                erc20("SNT", "Status", "0x744d70FDBE2Ba4CF95131626614a1763DF805B9E", 18),
                erc20("SNX", "Synthetix Network Token", "0xC011a73ee8576Fb46F5E1c5751cA3B9Fe0af2a6F", 18),
                erc20("SOL", "SOL Wormhole ", "0xD31a59c85aE9D8edEFeC411D448f90841571b89c", 9),
                erc20("STG", "Stargate Finance", "0xAf5191B0De278C7286d6C7CC6ab6BB8A73bA2Cd6", 18),
                erc20("sUSD", "Synth sUSD", "0x57Ab1ec28D129707052df4dF418D58a2D46d5f51", 18),
                erc20("SUSHI", "Sushi", "0x6B3595068778DD592e39A122f4f5a5cF09C90fE2", 18),
                erc20("SYN", "Synapse", "0x0f2D719407FdBeFF09D87557AbB7232601FD9F29", 18),
                erc20("TEL", "Telcoin", "0x467Bccd9d29f223BcE8043b84E8C8B282827790F", 2),
                erc20("UMA", "UMA Voting Token v1", "0x04Fa0d235C4abf4BcF4787aF4CF447DE572eF828", 18),
                erc20("UNI", "Uniswap", "0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", 18),
                erc20("USDC", "USDCoin", "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", 6),
                erc20("USDT", "Tether USD", "0xdAC17F958D2ee523a2206206994597C13D831ec7", 6),
                erc20("WBTC", "Wrapped BTC", "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", 8),
                erc20("WETH", "Wrapped Ether", "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", 18),
                erc20("WOO", "WOO Network", "0x4691937a7508860F876c9c0a2a617E7d9E945D4B", 18),
                erc20("YFI", "yearn finance", "0x0bc529c00C6401aEF6D220BE8C6Ea1667F6Ad93e", 18),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
                erc20("ZRX", "0x Protocol Token", "0xE41d2489571d322189246DaFA5ebDe1F4699F498", 18),
            ]);
        }
        EthNetwork::ArbitrumOne => {
            tokens.extend([
                erc20("1INCH", "1inch", "0x6314C31A7a1652cE482cffe247E9CB7c3f4BB9aF", 18),
                erc20("AAVE", "Aave", "0xba5DdD1f9d7F570dc94a51479a000E3BCE967196", 18),
                erc20("ALPHA", "Alpha Venture DAO", "0xC9CBf102c73fb77Ec14f8B4C8bd88e050a6b2646", 18),
                erc20("ANKR", "Ankr", "0x1bfc5d35bf0f7B9e15dc24c78b8C02dbC1e95447", 18),
                erc20("ARB", "Arbitrum", "0x912CE59144191C1204E64559FE8253a0e49E6548", 18),
                erc20("AUSD", "AUSD", "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", 6),
                erc20("AXL", "Axelar", "0x23ee2343B892b1BB63503a4FAbc840E0e2C6810f", 6),
                erc20("BAL", "Balancer", "0x040d1EdC9569d4Bab2D15287Dc5A4F10F56a56B8", 18),
                erc20("BUSD", "Binance USD", "0x31190254504622cEFdFA55a7d3d272e6462629a2", 18),
                erc20("cbETH", "Coinbase Wrapped Staked ETH", "0x1DEBd73E752bEaF79865Fd6446b0c970EaE7732f", 18),
                erc20("COMP", "Compound", "0x354A6dA3fcde098F8389cad84b0182725c6C91dE", 18),
                erc20("CRV", "Curve DAO Token", "0x11cDb42B0EB46D95f990BeDD4695A6e3fA034978", 18),
                erc20("CTSI", "Cartesi", "0x319f865b287fCC10b30d8cE6144e8b6D1b476999", 18),
                erc20("DAI", "Dai Stablecoin", "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1", 18),
                erc20("DRV", "Derive", "0x77b7787a09818502305C95d68A2571F090abb135", 18),
                erc20("ENS", "Ethereum Name Service", "0xfeA31d704DEb0975dA8e77Bf13E04239e70d7c28", 18),
                erc20("EURA", "agEur (listed as AGEUR)", "0xFA5Ed56A203466CbBC2430a43c66b9D8723528E7", 18),
                erc20("EUROC", "Euro Coin (listed as EURC)", "0x863708032B5c328e11aBcbC0DF9D79C71Fc52a48", 6),
                erc20("FARM", "Harvest Finance", "0x8553d254Cb6934b16F87D2e486b64BbD24C83C70", 18),
                erc20("FET", "Fetch ai", "0x4BE87C766A7CE11D5Cc864b6C3Abb7457dCC4cC9", 18),
                erc20("FLUX", "Flux", "0x63806C056Fa458c548Fb416B15E358A9D685710A", 8),
                erc20("FRAX", "Frax", "0x7468a5d8E02245B00E8C0217fCE021C70Bc51305", 18),
                erc20("FXS", "Frax Share", "0xd9f9d2Ee2d3EFE420699079f16D9e924affFdEA4", 18),
                erc20("GRT", "The Graph", "0x9623063377AD1B27544C965cCd7342f7EA7e88C7", 18),
                erc20("KRL", "KRYLL", "0xf75eE6D319741057a82a88Eeff1DbAFAB7307b69", 18),
                erc20("KUJI", "Kujira", "0x3A18dcC9745eDcD1Ef33ecB93b0b6eBA5671e7Ca", 6),
                erc20("LINK", "ChainLink Token", "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4", 18),
                erc20("LRC", "LoopringCoin V2", "0x46d0cE7de6247b0A95f67b43B589b4041BaE7fbE", 18),
                erc20("MASK", "Mask Network", "0x533A7B414CD1236815a5e09F1E97FC7d5c313739", 18),
                erc20("MATIC", "Polygon", "0x561877b6b3DD7651313794e5F2894B2F18bE0766", 18),
                erc20("MIM", "Magic Internet Money", "0xB20A02dfFb172C474BC4bDa3fD6f4eE70C04daf2", 18),
                erc20("MKR", "Maker", "0x2e9a6Df78E42a30712c10a9Dc4b1C8656f8F2879", 18),
                erc20("MULTI", "Multichain", "0x7b9b94aebe5E2039531af8E31045f377EcD9A39A", 18),
                erc20("PENDLE", "Pendle", "0x0c880f6761F1af8d9Aa9C466984b80DAb9a8c9e8", 18),
                erc20("PERP", "Perpetual Protocol", "0x753D224bCf9AAFaCD81558c32341416df61D3DAC", 18),
                erc20("RAI", "Rai Reflex Index", "0xaeF5bbcbFa438519a5ea80B4c7181B4E78d419f2", 18),
                erc20("RPL", "Rocket Pool Protocol", "0xB766039cc6DB368759C1E56B79AFfE831d0Cc507", 18),
                erc20("SNT", "Status", "0x707F635951193dDaFBB40971a0fCAAb8A6415160", 18),
                erc20("SNX", "Synthetix Network Token", "0xcBA56Cd8216FCBBF3fA6DF6137F3147cBcA37D60", 18),
                erc20("SOL", "SOL Wormhole ", "0xb74Da9FE2F96B9E0a5f4A3cf0b92dd2bEC617124", 9),
                erc20("STG", "Stargate Finance", "0xe018C7a3d175Fb0fE15D70Da2c874d3CA16313EC", 18),
                erc20("sUSD", "Synth sUSD", "0xA970AF1a584579B618be4d69aD6F73459D112F95", 18),
                erc20("SUSHI", "Sushi", "0xd4d42F0b6DEF4CE0383636770eF773390d85c61A", 18),
                erc20("SYN", "Synapse", "0x1bCfc0B4eE1471674cd6A9F6B363A034375eAD84", 18),
                erc20("TEL", "Telcoin", "0x0419E8bfBBB2623728c3A6129090DA4Ff4e48113", 2),
                erc20("UMA", "UMA Voting Token v1", "0xd693Ec944A85eeca4247eC1c3b130DCa9B0C3b22", 18),
                erc20("UNI", "Uniswap", "0xFa7F8980b0f1E64A2062791cc3b0871572f1F7f0", 18),
                erc20("USDC", "USDCoin", "0xaf88d065e77c8cC2239327C5EDb3A432268e5831", 6),
                erc20("USDT", "Tether USD (USDT0)", "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9", 6),
                erc20("WBTC", "Wrapped BTC", "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f", 8),
                erc20("WETH", "Wrapped Ether", "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", 18),
                erc20("WOO", "WOO Network", "0xcAFcD85D8ca7Ad1e1C6F82F651fA15E33AEfD07b", 18),
                erc20("XAUt0", "Tether Gold", "0x40461291347e1eCbb09499F3371D3f17f10d7159", 6),
                erc20("YFI", "yearn finance", "0x82e3A8F066a6989666b031d916c43672085b1582", 18),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
                erc20("ZRX", "0x Protocol Token", "0xBD591Bd4DdB64b77B5f76Eab8f03d02519235Ae2", 18),
            ]);
        }
        EthNetwork::Base => {
            tokens.extend([
                erc20("ARPA", "ARPA Chain", "0x1C9Fa01e87487712706Fb469a13bEb234262C867", 18),
                erc20("AUSD", "AUSD", "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", 6),
                erc20("cbBTC", "Coinbase Wrapped BTC", "0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf", 8),
                erc20("cbETH", "Coinbase Wrapped Staked ETH", "0x2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22", 18),
                erc20("COMP", "Compound", "0x9e1028F5F1D5eDE59748FFceE5532509976840E0", 18),
                erc20("DAI", "Dai Stablecoin", "0x50c5725949A6F0c72E6C4a641F24049A917DB0Cb", 18),
                erc20("DRV", "Derive", "0x9d0E8f5b25384C7310CB8C6aE32C8fbeb645d083", 18),
                erc20("EURC", "EURC", "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42", 6),
                erc20("FARM", "Harvest Finance", "0xD08a2917653d4E460893203471f0000826fb4034", 18),
                erc20("FET", "Fetch ai", "0x74F804B4140ee70830B3Eef4e690325841575F89", 18),
                erc20("FLUX", "Flux", "0xb008BDCF9CdFf9da684a190941dC3dCa8C2Cdd44", 8),
                erc20("KII", "Kiichain", "0x3EBA6644819546C44Eb3e7c3A92f034f921dcA80", 18),
                erc20("KRL", "KRYLL", "0xDAE49C25fAd3a62a8e8bFB6dA12c46bE611f9f7a", 18),
                erc20("LRC", "LoopringCoin V2", "0x0D760ee479401Bb4C40BDB7604b329FfF411b3f2", 18),
                erc20("RPL", "Rocket Pool Protocol", "0x1f73EAf55d696BFFA9b0EA16fa987B93b0f4d302", 18),
                erc20("SNT", "Status", "0x662015EC830DF08C0FC45896FaB726542e8AC09E", 18),
                erc20("SNX", "Synthetix Network Token", "0x22e6966B799c4D5B13BE962E1D117b56327FDa66", 18),
                erc20("TEL", "Telcoin", "0x09bE1692ca16e06f536F0038fF11D1dA8524aDB1", 2),
                erc20("UNI", "Uniswap", "0xc3De830EA07524a0761646a6a4e4be0e114a3C83", 18),
                erc20("USDC", "USD Coin", "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", 6),
                erc20("USDT", "Tether USD", "0xfde4C96c8593536E31F229EA8f37b2ADa2699bb2", 6),
                erc20("WETH", "Wrapped Ether", "0x4200000000000000000000000000000000000006", 18),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
                erc20("ZRX", "0x Protocol Token", "0x3bB4445D30AC020a84c1b5A8A2C6248ebC9779D0", 18),
            ]);
        }
        EthNetwork::Optimism => {
            tokens.extend([
                erc20("1INCH", "1inch", "0xAd42D013ac31486B73b6b059e748172994736426", 18),
                erc20("AAVE", "Aave", "0x76FB31fb4af56892A25e32cFC43De717950c9278", 18),
                erc20("ARPA", "ARPA Chain", "0x334cc734866E97D8452Ae6261d68Fd9bc9BFa31E", 18),
                erc20("BAL", "Balancer", "0xFE8B128bA8C78aabC59d4c64cEE7fF28e9379921", 18),
                erc20("BUSD", "Binance USD", "0x9C9e5fD8bbc25984B178FdCE6117Defa39d2db39", 18),
                erc20("cbETH", "Coinbase Wrapped Staked ETH", "0xadDb6A0412DE1BA0F936DCaeb8Aaa24578dcF3B2", 18),
                erc20("CRV", "Curve DAO Token", "0x0994206dfE8De6Ec6920FF4D779B0d950605Fb53", 18),
                erc20("CTSI", "Cartesi", "0xEc6adef5E1006bb305bB1975333e8fc4071295bf", 18),
                erc20("DAI", "Dai Stablecoin", "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1", 18),
                erc20("DRV", "Derive", "0x33800De7E817A70A694F31476313A7c572BBa100", 18),
                erc20("ENS", "Ethereum Name Service", "0x65559aA14915a70190438eF90104769e5E890A00", 18),
                erc20("FRAX", "Frax", "0x2E3D870790dC77A83DD1d18184Acc7439A53f475", 18),
                erc20("FXS", "Frax Share", "0x67CCEA5bb16181E7b4109c9c2143c24a1c2205Be", 18),
                erc20("KRL", "KRYLL", "0x2ed6222CB75E353b8789bec7Bb443b7eC9022021", 18),
                erc20("KUJI", "Kujira", "0x3A18dcC9745eDcD1Ef33ecB93b0b6eBA5671e7Ca", 6),
                erc20("LINK", "ChainLink Token", "0x350a791Bfc2C21F9Ed5d10980Dad2e2638ffa7f6", 18),
                erc20("LRC", "LoopringCoin V2", "0xFEaA9194F9F8c1B65429E31341a103071464907E", 18),
                erc20("MASK", "Mask Network", "0x3390108E913824B8eaD638444cc52B9aBdF63798", 18),
                erc20("MKR", "Maker", "0xab7bAdEF82E9Fe11f6f33f87BC9bC2AA27F2fCB5", 18),
                erc20("OP", "Optimism", "0x4200000000000000000000000000000000000042", 18),
                erc20("PENDLE", "Pendle", "0xBC7B1Ff1c6989f006a1185318eD4E7b5796e66E1", 18),
                erc20("PERP", "Perpetual Protocol", "0x9e1028F5F1D5eDE59748FFceE5532509976840E0", 18),
                erc20("RAI", "Rai Reflex Index", "0x7FB688CCf682d58f86D7e38e03f9D22e7705448B", 18),
                erc20("RPL", "Rocket Pool Protocol", "0xC81D1F0EB955B0c020E5d5b264E1FF72c14d1401", 18),
                erc20("SNT", "Status", "0x650AF3C15AF43dcB218406d30784416D64Cfb6B2", 18),
                erc20("SNX", "Synthetix Network Token", "0x8700dAec35aF8Ff88c16BdF0418774CB3D7599B4", 18),
                erc20("SOL", "SOL Wormhole ", "0xba1Cf949c382A32a09A17B2AdF3587fc7fA664f1", 9),
                erc20("sUSD", "Synth sUSD", "0x8c6f28f2F1A3C87F0f938b96d27520d9751ec8d9", 18),
                erc20("SUSHI", "Sushi", "0x3eaEb77b03dBc0F6321AE1b72b2E9aDb0F60112B", 18),
                erc20("UMA", "UMA Voting Token v1", "0xE7798f023fC62146e8Aa1b36Da45fb70855a77Ea", 18),
                erc20("UNI", "Uniswap", "0x6fd9d7AD17242c41f7131d257212c54A0e816691", 18),
                erc20("USDC", "USDCoin", "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85", 6),
                erc20("USDT", "Tether USD", "0x94b008aA00579c1307B0EF2c499aD98a8ce58e58", 6),
                erc20("WBTC", "Wrapped BTC", "0x68f180fcCe6836688e9084f035309E29Bf0A2095", 8),
                erc20("WETH", "Wrapped Ether", "0x4200000000000000000000000000000000000006", 18),
                erc20("WOO", "WOO Network", "0x871f2F2ff935FD1eD867842FF2a7bfD051A5E527", 18),
                erc20("YFI", "yearn finance", "0x9046D36440290FfDE54FE0DD84Db8b1CfEE9107B", 18),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
                erc20("ZRX", "0x Protocol Token", "0xD1917629B3E6A72E6772Aab5dBe58Eb7FA3C2F33", 18),
            ]);
        }
        EthNetwork::PolygonPos => {
            tokens.extend([
                erc20("AAVE", "Aave", "0xD6DF932A45C0f255f85145f286eA0b292B21C90B", 18),
                erc20("AUSD", "AUSD", "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", 6),
                erc20("BAL", "Balancer", "0x9a71012B13CA4d3D0Cdc72A177DF3ef03b0E76A3", 18),
                erc20("COMP", "Compound", "0x8505b9d2254A7Ae468c0E9dd10Ccea3A837aef5c", 18),
                erc20("CRV", "Curve DAO Token", "0x172370d5Cd63279eFa6d502DAB29171933a610AF", 18),
                erc20("DAI", "Dai Stablecoin", "0x8f3Cf7ad23Cd3CaDbD9735AFf958023239c6A063", 18),
                erc20("ENS", "Ethereum Name Service", "0xbD7A5Cf51d22930B8B3Df6d834F9BCEf90EE7c4f", 18),
                erc20("EURA", "agEur (listed as AGEUR)", "0xE0B52e49357Fd4DAf2c15e02058DCE6BC0057db4", 18),
                erc20("GRT", "The Graph", "0x5fe2B58c013d7601147DcdD68C143A77499f5531", 18),
                erc20("KII", "Kiichain", "0xEEC6574eAbBa52bac3f0277F2cD5Ac7e67197886", 18),
                erc20("LINK", "ChainLink Token", "0x53E0bca35eC356BD5ddDFebbD1Fc0fD03FaBad39", 18),
                erc20("LRC", "LoopringCoin V2", "0x84e1670F61347CDaeD56dcc736FB990fBB47ddC1", 18),
                erc20("MKR", "Maker", "0x6f7C932e7684666C9fd1d44527765433e01fF61d", 18),
                erc20("SNX", "Synthetix Network Token", "0x50B728D8D964fd00C2d0AAD81718b71311feF68a", 18),
                erc20("sUSD", "Synth sUSD", "0xF81b4Bec6Ca8f9fe7bE01CA734F55B2b6e03A7a0", 18),
                erc20("TEL", "Telcoin", "0xdF7837DE1F2Fa4631D716CF2502f8b230F1dcc32", 2),
                erc20("UMA", "UMA Voting Token v1", "0x3066818837c5e6eD6601bd5a91B0762877A6B731", 18),
                erc20("UNI", "Uniswap", "0xb33EaAd8d922B1083446DC23f610c2567fB5180f", 18),
                erc20("USDC", "USDCoin", "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", 6),
                erc20("USDT0", "Tether USD (listed as USDT)", "0xc2132D05D31c914a87C6611C10748AEb04B58e8F", 6),
                erc20("WBTC", "Wrapped BTC", "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6", 8),
                erc20("WETH", "Wrapped Ether", "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619", 18),
                erc20("XAUt0", "Tether Gold", "0xF1815bd50389c46847f0Bda824eC8da914045D14", 6),
                erc20("YFI", "yearn finance", "0xDA537104D6A5edd53c6fBba9A898708E465260b6", 18),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
                erc20("ZRX", "0x Protocol Token", "0x5559Edb74751A0edE9DeA4DC23aeE72cCA6bE3D5", 18),
            ]);
        }
        EthNetwork::BnbSmartChain => {
            tokens.extend([
                erc20("1INCH", "1inch", "0x111111111117dC0aa78b770fA6A738034120C302", 18),
                erc20("AAVE", "Aave", "0xfb6115445Bff7b52FeB98650C87f44907E58f802", 18),
                erc20("ALPHA", "Alpha Venture DAO", "0xa1faa113cbE53436Df28FF0aEe54275c13B40975", 18),
                erc20("ANKR", "Ankr", "0xf307910A4c7bbc79691fD374889b36d8531B08e3", 18),
                erc20("ARPA", "ARPA Chain", "0x6F769E65c14Ebd1f68817F5f1DcDb61Cfa2D6f7e", 18),
                erc20("AUSD", "AUSD", "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", 6),
                erc20("AXL", "Axelar", "0x8b1f4432F943c465A973FeDC6d7aa50Fc96f1f65", 6),
                erc20("BUSD", "Binance USD", "0xe9e7CEA3DedcA5984780Bafc599bD69ADd087D56", 18),
                erc20("COMP", "Compound", "0x52CE071Bd9b1C4B00A0b92D298c512478CaD67e8", 18),
                erc20("CTSI", "Cartesi", "0x8dA443F84fEA710266C8eB6bC34B71702d033EF2", 18),
                erc20("DAI", "Dai Stablecoin", "0x1AF3F329e8BE154074D8769D1FFa4eE058B1DBc3", 18),
                erc20("ETH", "Wrapped Ether (listed as WETH)", "0x2170Ed0880ac9A755fd29B2688956BD959F933F8", 18),
                erc20("EURA", "agEur (listed as AGEUR)", "0x12f31B73D812C6Bb0d735a218c086d44D5fe5f89", 18),
                erc20("FARM", "Harvest Finance", "0x4B5C23cac08a567ecf0c1fFcA8372A45a5D33743", 18),
                erc20("FET", "Fetch ai", "0x031b41e504677879370e9DBcF937283A8691Fa7f", 18),
                erc20("FRAX", "Frax", "0x90C97F71E18723b0Cf0dfa30ee176Ab653E89F40", 18),
                erc20("FXS", "Frax Share", "0xe48A3d7d0Bc88d552f730B62c006bC925eadB9eE", 18),
                erc20("KII", "Kiichain", "0xEEC6574eAbBa52bac3f0277F2cD5Ac7e67197886", 18),
                erc20("KUJI", "Kujira", "0x073690e6CE25bE816E68F32dCA3e11067c9FB5Cc", 6),
                erc20("LINK", "ChainLink Token", "0xF8A0BF9cF54Bb92F17374d9e9A321E6a111a51bD", 18),
                erc20("MASK", "Mask Network", "0x2eD9a5C8C13b93955103B9a7C167B67Ef4d568a3", 18),
                erc20("MATIC", "Polygon", "0xCC42724C6683B7E57334c4E856f4c9965ED682bD", 18),
                erc20("MIM", "Magic Internet Money", "0xfE19F0B51438fd612f6FD59C1dbB3eA319f433Ba", 18),
                erc20("MULTI", "Multichain", "0x9Fb9a33956351cf4fa040f65A13b835A3C8764E3", 18),
                erc20("PERP", "Perpetual Protocol", "0x4e7f408be2d4E9D60F49A64B89Bb619c84C7c6F5", 18),
                erc20("SOL", "SOL Wormhole ", "0xfA54fF1a158B5189Ebba6ae130CEd6bbd3aEA76e", 9),
                erc20("STG", "Stargate Finance", "0xB0D502E938ed5f4df2E681fE6E419ff29631d62b", 18),
                erc20("SUSHI", "Sushi", "0x947950BcC74888a40Ffa2593C5798F11Fc9124C4", 18),
                erc20("SYN", "Synapse", "0xa4080f1778e69467E905B8d6F72f6e441f9e9484", 18),
                erc20("UNI", "Uniswap", "0xBf5140A22578168FD562DCcF235E5D43A02ce9B1", 18),
                erc20("USDC", "USDCoin", "0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d", 18),
                erc20("USDT", "Tether USD", "0x55d398326f99059fF775485246999027B3197955", 18),
                erc20("WBNB", "Wrapped BNB", "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", 18),
                erc20("WOO", "WOO Network", "0x4691937a7508860F876c9c0a2a617E7d9E945D4B", 18),
                erc20("XAUt", "Tether Gold (listed as XAUT0)", "0x21cAef8A43163Eea865baeE23b9C2E327696A3bf", 6),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
            ]);
        }
        EthNetwork::AvalancheCChain => {
            tokens.extend([
                erc20("1INCH.e", "1inch (bridged)", "0xd501281565bf7789224523144Fe5D98e8B28f267", 18),
                erc20("AAVE.e", "Aave (bridged)", "0x63a72806098Bd3D9520cC43356dD78afe5D386D9", 18),
                erc20("ALPHA.e", "Alpha Venture DAO (bridged)", "0x2147EFFF675e4A4eE1C2f918d181cDBd7a8E208f", 18),
                erc20("ANKR", "Ankr", "0x20CF1b6E9d856321ed4686877CF4538F2C84B4dE", 18),
                erc20("AUSD", "AUSD", "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", 6),
                erc20("AXL", "Axelar", "0x44c784266cf024a60e8acF2427b9857Ace194C5d", 6),
                erc20("BUSD", "Binance USD", "0x9C9e5fD8bbc25984B178FdCE6117Defa39d2db39", 18),
                erc20("COMP.e", "Compound (bridged)", "0xc3048E19E76CB9a3Aa9d77D8C03c29Fc906e2437", 18),
                erc20("CTSI", "Cartesi", "0x6b289CCeAA8639e3831095D75A3e43520faBf552", 18),
                erc20("EURA", "agEur (listed as AGEUR)", "0xAEC8318a9a59bAEb39861d10ff6C7f7bf1F96C57", 18),
                erc20("EURC", "Euro Coin", "0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD", 6),
                erc20("FLUX", "Flux", "0xc4B06F17ECcB2215a5DBf042C672101Fc20daF55", 8),
                erc20("FRAX", "Frax", "0xD24C2Ad096400B6FBcd2ad8B24E7acBc21A1da64", 18),
                erc20("FXS", "Frax Share", "0x214DB107654fF987AD859F34125307783fC8e387", 18),
                erc20("GRT.e", "The Graph (bridged)", "0x8a0cAc13c7da965a312f08ea4229c37869e85cB9", 18),
                erc20("LINK.e", "ChainLink Token (bridged)", "0x5947BB275c521040051D82396192181b413227A3", 18),
                erc20("MIM", "Magic Internet Money", "0x130966628846BFd36ff31a822705796e8cb8C18D", 18),
                erc20("MKR.e", "Maker (bridged)", "0x88128fd4b259552A9A1D457f435a6527AAb72d42", 18),
                erc20("MULTI", "Multichain", "0x9Fb9a33956351cf4fa040f65A13b835A3C8764E3", 18),
                erc20("PENDLE", "Pendle", "0xfB98B335551a418cD0737375a2ea0ded62Ea213b", 18),
                erc20("RAI", "Rai Reflex Index", "0x97Cd1CFE2ed5712660bb6c14053C0EcB031Bff7d", 18),
                erc20("SNX.e", "Synthetix Network Token (bridged)", "0xBeC243C995409E6520D7C41E404da5dEba4b209B", 18),
                erc20("SOL", "SOL Wormhole ", "0xFE6B19286885a4F7F55AdAD09C3Cd1f906D2478F", 9),
                erc20("STG", "Stargate Finance", "0x2F6F07CDcf3588944Bf4C42aC74ff24bF56e7590", 18),
                erc20("SUSHI.e", "Sushi (bridged)", "0x37B608519F91f70F2EeB0e5Ed9AF4061722e4F76", 18),
                erc20("SYN", "Synapse", "0x1f1E7c893855525b303f99bDF5c3c05Be09ca251", 18),
                erc20("UMA.e", "UMA Voting Token v1 (bridged)", "0x3Bd2B1c7ED8D396dbb98DED3aEbb41350a5b2339", 18),
                erc20("USDC", "USDC Token", "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E", 6),
                erc20("USDt", "Tether USD", "0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7", 6),
                erc20("WAVAX", "Wrapped AVAX", "0xB31f66AA3C1e785363F0875A1B74E27b85FD66c7", 18),
                erc20("WBTC.e", "Wrapped BTC (bridged)", "0x50b7545627a5162F82A992c33b87aDc75187B218", 8),
                erc20("WETH.e", "Wrapped Ether (bridged)", "0x49D5c2BdFfac6CE2BFdB6640F4F80f226bc10bAB", 18),
                erc20("WOO.e", "WOO Network (bridged)", "0xaBC9547B534519fF73921b1FBA6E672b5f58D083", 18),
                erc20("XAUt0", "Tether Gold", "0x2775d5105276781B4b85bA6eA6a6653bEeD1dd32", 6),
                erc20("YFI.e", "yearn finance (bridged)", "0x9eAaC1B23d935365bD7b542Fe22cEEe2922f52dc", 18),
                erc20("ZRO", "LayerZero", "0x6985884C4392D348587B19cb9eAAf157F13271cd", 18),
                erc20("ZRX.e", "0x Protocol Token (bridged)", "0x596fA47043f99A4e0F122243B841E55375cdE0d2", 18),
            ]);
        }
    }
    tokens
}

fn encode_address_arg(address: Address) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[12..].copy_from_slice(address.as_slice());
    out
}

fn encode_u256_arg(amount: U256) -> [u8; 32] {
    amount.to_be_bytes::<32>()
}

fn encode_balance_of(account: Address) -> Bytes {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&SELECTOR_BALANCE_OF);
    data.extend_from_slice(&encode_address_arg(account));
    Bytes::from(data)
}

fn encode_transfer(to: Address, amount: U256) -> Bytes {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&SELECTOR_TRANSFER);
    data.extend_from_slice(&encode_address_arg(to));
    data.extend_from_slice(&encode_u256_arg(amount));
    Bytes::from(data)
}

fn encode_selector(selector: [u8; 4]) -> Bytes {
    Bytes::from(selector.to_vec())
}

fn decode_u256(bytes: &[u8]) -> U256 {
    if bytes.is_empty() {
        return U256::ZERO;
    }
    U256::from_be_slice(bytes)
}

fn decode_string(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 64 {
        return std::str::from_utf8(bytes)
            .ok()
            .map(|s| s.trim_matches('\0').trim().to_string())
            .filter(|s| !s.is_empty());
    }
    let len = U256::from_be_slice(&bytes[32..64]).try_into().ok()?;
    let start = 64usize;
    let end = start.saturating_add(len);
    if end > bytes.len() {
        return None;
    }
    String::from_utf8(bytes[start..end].to_vec()).ok()
}

fn address_topic(address: Address) -> B256 {
    let mut topic = [0u8; 32];
    topic[12..].copy_from_slice(address.as_slice());
    B256::from(topic)
}

async fn eth_call<P: Provider>(provider: &P, to: Address, data: Bytes) -> Result<Bytes, block_error::Error> {
    let tx = TransactionRequest::default().with_to(to).with_input(data);
    provider
        .call(tx)
        .await
        .map_err(|e| block_error::Error::new(format!("eth_call failed: {e}")))
}

fn erc20(symbol: &str, name: &str, address: &str, decimals: u8) -> RegistryToken {
    RegistryToken {
        symbol: symbol.into(),
        name: name.into(),
        address: address.into(),
        decimals,
        native: false,
    }
}

/// Decided by address only. The symbol is not evidence: BNB Smart Chain's bundled list has an
/// ERC-20 whose contract reports "ETH", and treating that as native would send BNB instead.
pub fn is_native_token(token: &Token) -> bool {
    token.address.trim().eq_ignore_ascii_case(NATIVE_SENTINEL) || token.address.trim().is_empty()
}

pub fn validate_address(address: &str) -> Result<Address, block_error::Error> {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return Err(block_error::Error::new("receive address is required".into()));
    }
    if trimmed.to_ascii_lowercase().contains(".eth") {
        return Err(block_error::Error::new("ENS names are not supported yet".into()));
    }
    Address::from_str(trimmed)
        .map_err(|e| block_error::Error::new(format!("invalid ethereum address: {e}")))
}

pub fn parse_token_amount(input: &str, decimals: u8) -> Result<U256, block_error::Error> {
    let s = crate::currencies::amount::normalize_decimal_input(input)?;
    let (whole, frac) = match s.split_once('.') {
        Some((whole, frac)) => (whole, frac),
        None => (s.as_str(), ""),
    };
    if frac.len() > decimals as usize {
        return Err(block_error::Error::new(format!(
            "amount has more than {decimals} decimal places"
        )));
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(block_error::Error::new("amount must be a number".into()));
    }
    let whole = if whole.is_empty() { "0" } else { whole };
    let mut frac = frac.to_string();
    while frac.len() < decimals as usize {
        frac.push('0');
    }
    let combined = format!("{whole}{frac}");
    let combined = combined.trim_start_matches('0');
    let combined = if combined.is_empty() { "0" } else { combined };
    U256::from_str(combined).map_err(|e| block_error::Error::new(format!("amount is too large: {e}")))
}

pub fn format_units_trimmed(amount: U256, decimals: u8) -> String {
    if amount.is_zero() {
        return "0".to_string();
    }
    let mut raw = amount.to_string();
    let decimals = decimals as usize;
    if decimals == 0 {
        return raw;
    }
    if raw.len() <= decimals {
        raw = format!("{:0>width$}", raw, width = decimals + 1);
    }
    let split = raw.len() - decimals;
    let whole = &raw[..split];
    let frac = raw[split..].trim_end_matches('0');
    if frac.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{frac}")
    }
}

/// Below this the fee-versus-amount rule is switched off: a dust-sized send legitimately
/// costs more in gas than it moves. 0.001 ETH, in wei.
const FEE_RATIO_FLOOR_WEI: u128 = 1_000_000_000_000_000;

/// The wei-denominated counterpart to [`crate::currencies::fees::check_fee_is_sane`]. Kept
/// here rather than in that module because it needs `U256`: a fee above ~18.4 ETH does not
/// fit in a `u64`, and saturating into one would let exactly the largest fees through.
fn check_native_fee_is_sane(fee_wei: U256, amount: U256) -> Result<(), block_error::Error> {
    if fee_wei > amount && fee_wei > U256::from(FEE_RATIO_FLOOR_WEI) {
        return Err(block_error::Error::new(format!(
            "network fee ({fee_wei} wei) is larger than the amount being sent ({amount} wei); \
             check the fee tier and the node this wallet is pointed at"
        )));
    }
    Ok(())
}

pub fn fee_from_tier(tiers: &FeeTiers, label: &str) -> (u128, u128) {
    let max_fee = match label.to_ascii_lowercase().as_str() {
        "low" => tiers.low,
        "high" => tiers.high,
        _ => tiers.medium,
    };
    // Both bounded: the estimate is whatever the node said, and `max_fee_per_gas` multiplied
    // by the gas limit is the ceiling on what this transaction can cost. The tip is also
    // capped at the max fee, which nodes require; the Low tier scales only the max fee, so
    // on a chain with a near-zero base fee the tip would otherwise exceed it.
    let max_fee = clamp_gas_price(max_fee);
    (max_fee, clamp_gas_price(tiers.priority).min(max_fee))
}

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> Result<T, block_error::Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| block_error::Error::new(format!("tokio runtime: {e}")))?
        .block_on(async move { Ok(fut.await) })
}

fn http_provider(rpc: &str) -> Result<impl Provider + Clone, block_error::Error> {
    let url = rpc
        .parse()
        .map_err(|e| block_error::Error::new(format!("invalid ETH RPC URL: {e}")))?;
    Ok(ProviderBuilder::new().connect_http(url))
}

fn signed_provider(
    rpc: &str,
    signer: PrivateKeySigner,
) -> Result<impl Provider + Clone, block_error::Error> {
    let url = rpc
        .parse()
        .map_err(|e| block_error::Error::new(format!("invalid ETH RPC URL: {e}")))?;
    Ok(ProviderBuilder::new().wallet(signer).connect_http(url))
}

fn signer_from_key(private_key: &str) -> Result<PrivateKeySigner, block_error::Error> {
    let mut key = private_key.trim().to_string();
    if let Some(stripped) = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")) {
        key = stripped.to_string();
    }
    PrivateKeySigner::from_str(&key)
        .or_else(|_| PrivateKeySigner::from_str(&format!("0x{key}")))
        .map_err(|e| block_error::Error::new(format!("invalid ethereum private key: {e}")))
}

pub fn sync_account(
    address: &str,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
    tokens: &[Token],
    etherscan_key: &str,
) -> Result<EthSyncState, block_error::Error> {
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    let account = validate_address(address)?;
    match block_on(sync_account_async(
        account,
        rpc,
        network,
        tokens.to_vec(),
        etherscan_key.to_string(),
    )) {
        Ok(Ok(state)) => Ok(state),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

async fn sync_account_async(
    account: Address,
    rpc: String,
    network: EthNetwork,
    tokens: Vec<Token>,
    etherscan_key: String,
) -> Result<EthSyncState, block_error::Error> {
    let symbol = native_symbol(network).to_string();
    let provider = match http_provider(&rpc) {
        Ok(provider) => provider,
        Err(_) => {
            return Ok(EthSyncState {
                eth_wei: U256::ZERO,
                receive_address: format!("{account:?}"),
                erc20: BTreeMap::new(),
                history: Vec::new(),
                offline: true,
                native_symbol: symbol,
            });
        }
    };

    let eth_wei = match provider.get_balance(account).await {
        Ok(value) => value,
        Err(_) => {
            return Ok(EthSyncState {
                eth_wei: U256::ZERO,
                receive_address: format!("{account:?}"),
                erc20: BTreeMap::new(),
                history: Vec::new(),
                offline: true,
                native_symbol: symbol,
            });
        }
    };

    let erc20 = fetch_token_balances(&provider, account, &tokens).await;

    // Native transfers move no tokens, so they emit no logs and `erc20_history` cannot see
    // them. They need an indexer. Etherscan is used when a key is configured; otherwise
    // Blockscout, which serves the same Etherscan-compatible shape without one.
    //
    // This used to be gated on the key alone, so a wallet with no Etherscan key simply had no
    // native history at all — the Activity list stayed empty however much ETH arrived or was
    // sent, which read as a bug in sending rather than a missing data source.
    let mut history = erc20_history(&provider, account, &tokens).await;
    let native = if !etherscan_key.trim().is_empty() {
        native_history_etherscan(account, chain_id(network), &etherscan_key, &symbol)
    } else {
        native_history_blockscout(account, network, &symbol)
    };
    match native {
        Ok(rows) => history.extend(rows),
        Err(why) => {
            crate::configuration::logging::warn(&format!("native history unavailable: {why}"))
        }
    }
    history.sort_by(|a, b| b.confirmations.cmp(&a.confirmations).then(b.txid.cmp(&a.txid)));
    history.truncate(HISTORY_CAP);

    Ok(EthSyncState {
        eth_wei,
        receive_address: format!("{account:?}"),
        erc20,
        history,
        offline: false,
        native_symbol: symbol,
    })
}

/// Fetch every bundled token balance for an account in one request.
///
/// Falls back to individual `balanceOf` calls if the batch fails for any reason: an RPC that
/// rejects a large `eth_call`, a chain where Multicall3 somehow is not deployed, or a
/// malformed response. The fallback is slow and is exactly the behaviour this replaced, so it
/// is a safety net rather than a path anything should routinely take. Getting a wrong balance
/// is worse than getting one slowly.
async fn fetch_token_balances<P: Provider>(
    provider: &P,
    account: Address,
    tokens: &[Token],
) -> BTreeMap<String, String> {
    let mut wanted: Vec<(&Token, Address)> = Vec::new();
    for token in tokens {
        if is_native_token(token) {
            continue;
        }
        if let Ok(contract) = Address::from_str(token.address.trim()) {
            wanted.push((token, contract));
        }
    }
    if wanted.is_empty() {
        return BTreeMap::new();
    }

    if let Some(balances) = batched_token_balances(provider, account, &wanted).await {
        return balances;
    }

    crate::configuration::logging::warn(
        "multicall balance read failed; falling back to one call per token",
    );
    let mut out = BTreeMap::new();
    for (token, contract) in &wanted {
        if let Ok(raw) = eth_call(provider, *contract, encode_balance_of(account)).await {
            let decimals = token.decimals.max(0) as u8;
            out.insert(
                token.symbol.clone(),
                format_units_trimmed(decode_u256(raw.as_ref()), decimals),
            );
        }
    }
    out
}

/// One `eth_call` covering every token, via Multicall3.
///
/// `None` means the batch could not be trusted and the caller should fall back. A token whose
/// own call reverted is simply omitted from the map, the same outcome the per-token version
/// reached by failing its request, so a single broken contract cannot blank the rest.
async fn batched_token_balances<P: Provider>(
    provider: &P,
    account: Address,
    wanted: &[(&Token, Address)],
) -> Option<BTreeMap<String, String>> {
    let multicall = Address::from_str(multicall::MULTICALL3).ok()?;
    let calls: Vec<multicall::Call3> = wanted
        .iter()
        .map(|(_, contract)| multicall::Call3 {
            target: *contract,
            allow_failure: true,
            call_data: encode_balance_of(account),
        })
        .collect();

    let raw = eth_call(provider, multicall, multicall::encode_aggregate3(&calls))
        .await
        .ok()?;
    let results = multicall::decode_aggregate3(raw.as_ref()).ok()?;

    // A result count that does not match what was asked for means the pairing of answers to
    // tokens is not reliable, and a balance attributed to the wrong token is worse than no
    // balance at all.
    if results.len() != wanted.len() {
        return None;
    }

    let mut out = BTreeMap::new();
    for ((token, _), result) in wanted.iter().zip(results) {
        if !result.success || result.return_data.len() < 32 {
            continue;
        }
        let decimals = token.decimals.max(0) as u8;
        out.insert(
            token.symbol.clone(),
            format_units_trimmed(decode_u256(result.return_data.as_ref()), decimals),
        );
    }
    Some(out)
}

/// Token transfer history for every bundled contract, in two log queries rather than two
/// per token.
///
/// The per-token loop this replaces cost `2 * tokens` `eth_getLogs` calls on every sync
/// cycle, which is the same shape of self-inflicted rate limiting that made Bitcoin look
/// permanently offline. `eth_getLogs` accepts a list of contract addresses in a single
/// filter, so a wider bundled token list now costs nothing extra: incoming and outgoing stay
/// at one call each however many tokens are listed. Logs are matched back to their token by
/// the emitting contract address.
async fn erc20_history<P: Provider>(
    provider: &P,
    account: Address,
    tokens: &[Token],
) -> Vec<EthHistoryItem> {
    let Ok(latest) = provider.get_block_number().await else {
        return Vec::new();
    };
    let from_block = latest.saturating_sub(LOG_LOOKBACK);

    let mut by_contract: BTreeMap<Address, &Token> = BTreeMap::new();
    for token in tokens {
        if is_native_token(token) {
            continue;
        }
        if let Ok(contract_addr) = Address::from_str(token.address.trim()) {
            by_contract.entry(contract_addr).or_insert(token);
        }
    }
    if by_contract.is_empty() {
        return Vec::new();
    }
    let contracts: Vec<Address> = by_contract.keys().copied().collect();

    let incoming_filter = Filter::new()
        .address(contracts.clone())
        .event_signature(TRANSFER_TOPIC)
        .from_block(from_block)
        .topic2(address_topic(account));
    let outgoing_filter = Filter::new()
        .address(contracts)
        .event_signature(TRANSFER_TOPIC)
        .from_block(from_block)
        .topic1(address_topic(account));

    let mut items = Vec::new();
    for (filter, incoming) in [(incoming_filter, true), (outgoing_filter, false)] {
        let Ok(logs) = provider.get_logs(&filter).await else {
            continue;
        };
        for log in logs {
            let Some(token) = by_contract.get(&log.address()) else {
                continue;
            };
            let Some(topics) = (log.topics().len() >= 3).then_some(log.topics()) else {
                continue;
            };
            let from = Address::from_slice(&topics[1].as_slice()[12..]);
            let to = Address::from_slice(&topics[2].as_slice()[12..]);
            let amount = U256::from_be_slice(log.data().data.as_ref());
            let confirmations = log
                .block_number
                .map(|block| latest.saturating_sub(block).saturating_add(1) as u32)
                .unwrap_or(0);
            items.push(EthHistoryItem {
                txid: log.transaction_hash.map(|h| format!("{h:#x}")).unwrap_or_default(),
                from: format!("{from:?}"),
                to: format!("{to:?}"),
                symbol: token.symbol.clone(),
                amount: format_units_trimmed(amount, token.decimals.max(0) as u8),
                incoming,
                confirmations,
            });
        }
    }
    items
}

/// Blockscout instance serving each network's Etherscan-compatible `txlist` API.
///
/// Used when no Etherscan key is configured. Blockscout answers unauthenticated and returns
/// the same JSON shape, so the parser below is shared. `None` where no public instance is
/// known, in which case native history is simply unavailable and says so.
pub fn blockscout_base(network: EthNetwork) -> Option<&'static str> {
    match network {
        EthNetwork::Mainnet => Some("https://eth.blockscout.com"),
        EthNetwork::Sepolia => Some("https://eth-sepolia.blockscout.com"),
        EthNetwork::ArbitrumOne => Some("https://arbitrum.blockscout.com"),
        EthNetwork::Base => Some("https://base.blockscout.com"),
        EthNetwork::Optimism => Some("https://optimism.blockscout.com"),
        EthNetwork::PolygonPos => Some("https://polygon.blockscout.com"),
        // Blockscout has no public instance this wallet can rely on for these two.
        EthNetwork::BnbSmartChain | EthNetwork::AvalancheCChain => None,
    }
}

fn native_history_blockscout(
    account: Address,
    network: EthNetwork,
    native_symbol: &str,
) -> Result<Vec<EthHistoryItem>, block_error::Error> {
    let base = blockscout_base(network).ok_or_else(|| {
        block_error::Error::new(
            "no keyless history source for this network; add an Etherscan API key in Settings"
                .to_string(),
        )
    })?;
    let url = format!(
        "{base}/api?module=account&action=txlist&address={account:?}&page=1&offset=20&sort=desc"
    );
    let text = crate::configuration::http::get_text(&url)?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    Ok(parse_txlist(&json, account, native_symbol))
}

fn native_history_etherscan(
    account: Address,
    chain_id: u64,
    api_key: &str,
    native_symbol: &str,
) -> Result<Vec<EthHistoryItem>, block_error::Error> {
    let url = format!(
        "https://api.etherscan.io/v2/api?chainid={chain_id}&module=account&action=txlist&address={account:?}&page=1&offset=20&sort=desc&apikey={api_key}"
    );
    let text = crate::configuration::http::get_text(&url)?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    Ok(parse_txlist(&json, account, native_symbol))
}

/// Parse the Etherscan-compatible `txlist` response shared by Etherscan and Blockscout.
fn parse_txlist(
    json: &serde_json::Value,
    account: Address,
    native_symbol: &str,
) -> Vec<EthHistoryItem> {
    let Some(rows) = json.get("result").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let account_lower = format!("{account:?}").to_ascii_lowercase();
    let mut items = Vec::new();
    for row in rows {
        let hash = row.get("hash").and_then(|v| v.as_str()).unwrap_or_default();
        let from = row.get("from").and_then(|v| v.as_str()).unwrap_or_default();
        let to = row.get("to").and_then(|v| v.as_str()).unwrap_or_default();
        let value = row
            .get("value")
            .and_then(|v| v.as_str())
            .and_then(|v| U256::from_str(v).ok())
            .unwrap_or(U256::ZERO);
        if value.is_zero() {
            continue;
        }
        let confirmations = row
            .get("confirmations")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let incoming = to.to_ascii_lowercase() == account_lower;
        items.push(EthHistoryItem {
            txid: hash.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            symbol: native_symbol.to_string(),
            amount: format_units_trimmed(value, 18),
            incoming,
            confirmations,
        });
    }
    items
}

pub fn fetch_fee_tiers(eth_node: &str, network_name: &str, infura_key: &str) -> FeeTiers {
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    block_on(fetch_fee_tiers_async(rpc))
        .ok()
        .flatten()
        .unwrap_or_default()
}

async fn fetch_fee_tiers_async(rpc: String) -> Option<FeeTiers> {
    let provider = http_provider(&rpc).ok()?;
    if let Ok(est) = provider.estimate_eip1559_fees().await {
        let medium = est.max_fee_per_gas.max(1);
        let priority = est.max_priority_fee_per_gas.max(1);
        return Some(FeeTiers {
            low: medium.saturating_mul(80) / 100,
            medium,
            high: medium.saturating_mul(130) / 100,
            priority,
        });
    }
    let gas = provider.get_gas_price().await.ok()?;
    Some(FeeTiers {
        low: gas.saturating_mul(80) / 100,
        medium: gas,
        high: gas.saturating_mul(130) / 100,
        priority: gas / 10,
    })
}

pub fn prepare_send(
    from: &str,
    to: &str,
    amount_text: &str,
    token: &Token,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
    fee_label: &str,
) -> Result<PreparedSend, block_error::Error> {
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    let from = validate_address(from)?;
    let to = validate_address(to)?;
    let decimals = if is_native_token(token) {
        18
    } else {
        token.decimals.max(0) as u8
    };
    let amount = parse_token_amount(amount_text, decimals)?;
    if amount.is_zero() {
        return Err(block_error::Error::new("amount must be greater than 0".into()));
    }
    let token = token.clone();
    match block_on(prepare_send_async(
        from,
        to,
        amount,
        token,
        rpc,
        chain_id(network),
        fee_label.to_string(),
        native_symbol(network).to_string(),
    )) {
        Ok(Ok(plan)) => Ok(plan),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

async fn prepare_send_async(
    from: Address,
    to: Address,
    amount: U256,
    token: Token,
    rpc: String,
    chain_id: u64,
    fee_label: String,
    fee_symbol: String,
) -> Result<PreparedSend, block_error::Error> {
    let provider = http_provider(&rpc)?;
    let native = is_native_token(&token);
    let nonce = provider
        .get_transaction_count(from)
        .pending()
        .await
        .map_err(|e| block_error::Error::new(format!("could not read nonce: {e}")))?;
    let eth_balance = provider
        .get_balance(from)
        .await
        .map_err(|_| block_error::Error::new("ethereum node is unreachable".into()))?;

    let mut tx = TransactionRequest::default()
        .with_from(from)
        .with_chain_id(chain_id)
        .with_nonce(nonce);
    if native {
        tx = tx.with_to(to).with_value(amount);
    } else {
        let contract = Address::from_str(token.address.trim())
            .map_err(|e| block_error::Error::new(format!("invalid token contract: {e}")))?;
        tx = tx.with_to(contract).with_input(encode_transfer(to, amount));
        let raw = eth_call(&provider, contract, encode_balance_of(from)).await?;
        let token_balance = decode_u256(raw.as_ref());
        if token_balance < amount {
            return Err(block_error::Error::new(format!(
                "not enough {} to send",
                token.symbol
            )));
        }
    }

    let gas_limit = provider
        .estimate_gas(tx.clone())
        .await
        .unwrap_or(if native { 21_000 } else { 65_000 });
    let gas_limit = gas_limit.saturating_add(gas_limit / 5).max(21_000);

    let tiers = fetch_fee_tiers_async(rpc.clone()).await.unwrap_or_default();
    let (max_fee_per_gas, max_priority_fee_per_gas) = fee_from_tier(&tiers, &fee_label);
    let fee_wei = U256::from(gas_limit) * U256::from(max_fee_per_gas);
    if eth_balance < fee_wei {
        return Err(block_error::Error::new(format!(
            "not enough {fee_symbol} to cover the network fee"
        )));
    }
    if native && eth_balance < amount.saturating_add(fee_wei) {
        return Err(block_error::Error::new(format!("not enough {fee_symbol} to send")));
    }

    // A native send is directly comparable: both sides are wei. For a token send the amount
    // is in token units and the fee is in the gas token, so there is nothing meaningful to
    // compare — the gas-price ceiling in `fee_from_tier` is the guard that applies there.
    if native {
        check_native_fee_is_sane(fee_wei, amount)?;
    }

    let decimals = if native { 18 } else { token.decimals.max(0) as u8 };
    Ok(PreparedSend {
        from: format!("{from:?}"),
        to: format!("{to:?}"),
        token_symbol: token.symbol,
        token_address: if native {
            None
        } else {
            Some(token.address)
        },
        amount,
        amount_display: format_units_trimmed(amount, decimals),
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        fee_wei,
        fee_symbol,
        chain_id,
        nonce,
    })
}

pub fn sign_and_broadcast(
    private_key: &str,
    plan: &PreparedSend,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
) -> Result<String, block_error::Error> {
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    let signer = signer_from_key(private_key)?;
    let plan = plan.clone();
    match block_on(sign_and_broadcast_async(signer, plan, rpc)) {
        Ok(Ok(hash)) => Ok(hash),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

async fn sign_and_broadcast_async(
    signer: PrivateKeySigner,
    plan: PreparedSend,
    rpc: String,
) -> Result<String, block_error::Error> {
    let from = signer.address();
    let expected_from = validate_address(&plan.from)?;
    if from != expected_from {
        return Err(block_error::Error::new(
            "this key does not belong to the account the transaction was reviewed for".to_string(),
        ));
    }
    let to = validate_address(&plan.to)?;
    if plan.token_address.is_none() {
        check_native_fee_is_sane(plan.fee_wei, plan.amount)?;
    }
    let provider = signed_provider(&rpc, signer)?;
    let mut tx = TransactionRequest::default()
        .with_from(from)
        .with_chain_id(plan.chain_id)
        .with_nonce(plan.nonce)
        .with_gas_limit(plan.gas_limit)
        .with_max_fee_per_gas(plan.max_fee_per_gas)
        .with_max_priority_fee_per_gas(plan.max_priority_fee_per_gas);
    if let Some(contract) = &plan.token_address {
        let contract = Address::from_str(contract.trim())
            .map_err(|e| block_error::Error::new(format!("invalid token contract: {e}")))?;
        tx = tx
            .with_to(contract)
            .with_input(encode_transfer(to, plan.amount));
    } else {
        tx = tx.with_to(to).with_value(plan.amount);
    }
    let pending = provider.send_transaction(tx).await.map_err(|e| {
        block_error::Error::new(format!("ethereum broadcast failed: {e}"))
    })?;
    Ok(format!("{:#x}", pending.tx_hash()))
}

/// Resolve an ENS name to an address.
///
/// Always queried against the chain that actually hosts the registry, which for every network
/// except Sepolia is mainnet, regardless of where the wallet is currently pointed. The result
/// is a plain address and is valid on any EVM chain.
pub fn resolve_ens(
    name: &str,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
) -> Result<Address, block_error::Error> {
    use crate::currencies::ens;

    let normalized = ens::normalize(name)?;
    let node = ens::namehash(&normalized);

    let wallet_network = parse_network(network_name);
    let registry_network = ens::registry_network(wallet_network);
    // A user-supplied RPC is only reused when it is already pointed at the chain the registry
    // lives on. Otherwise the built-in endpoint for that chain is used, since querying a Base
    // RPC for a mainnet registry entry would simply find nothing.
    let rpc = if registry_network == wallet_network {
        resolve_rpc(eth_node, wallet_network, infura_key)
    } else {
        default_rpc(registry_network).to_string()
    };

    let registry = Address::from_str(ens::ENS_REGISTRY)
        .map_err(|_| block_error::Error::new("bad ENS registry address".to_string()))?;

    match block_on(async move {
        let provider = http_provider(&rpc)?;

        let resolver_raw = eth_call(&provider, registry, Bytes::from(ens::encode_resolver_call(node))).await?;
        let resolver = ens::decode_address_word(resolver_raw.as_ref()).ok_or_else(|| {
            block_error::Error::new(format!("{normalized} has no ENS resolver"))
        })?;

        let addr_raw = eth_call(&provider, resolver, Bytes::from(ens::encode_addr_call(node))).await?;
        ens::decode_address_word(addr_raw.as_ref()).ok_or_else(|| {
            block_error::Error::new(format!("{normalized} does not resolve to an address"))
        })
    }) {
        Ok(Ok(address)) => Ok(address),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

/// Turn whatever was typed into a recipient address.
///
/// Accepts a raw `0x…` address unchanged, or resolves an ENS name. Returned alongside the
/// resolved address is the name it came from, when there was one, so the UI can show the user
/// what a name actually resolved to before they confirm.
pub fn resolve_recipient(
    input: &str,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
) -> Result<(Address, Option<String>), block_error::Error> {
    use crate::currencies::ens;

    if ens::looks_like_name(input) {
        let address = resolve_ens(input, eth_node, network_name, infura_key)?;
        Ok((address, Some(input.trim().to_ascii_lowercase())))
    } else {
        Ok((validate_address(input)?, None))
    }
}

/// ABI-encode `approve(address,uint256)`.
///
/// Selector is asserted against its own keccak hash in the tests, rather than trusted from
/// memory, because an approval sent to the wrong function signature either reverts or does
/// something unintended with the user's token balance.
pub fn encode_approve(spender: Address, amount: U256) -> Bytes {
    const SELECTOR_APPROVE: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&SELECTOR_APPROVE);
    data.extend_from_slice(&B256::left_padding_from(spender.as_slice())[..]);
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
}

/// Read the current ERC-20 allowance from `owner` to `spender`.
pub fn fetch_allowance(
    token: &str,
    owner: &str,
    spender: &str,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
) -> Result<U256, block_error::Error> {
    const SELECTOR_ALLOWANCE: [u8; 4] = [0xdd, 0x62, 0xed, 0x3e];
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    let contract = Address::from_str(token.trim())
        .map_err(|e| block_error::Error::new(format!("invalid token contract: {e}")))?;
    let owner = validate_address(owner)?;
    let spender = validate_address(spender)?;

    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&SELECTOR_ALLOWANCE);
    data.extend_from_slice(&B256::left_padding_from(owner.as_slice())[..]);
    data.extend_from_slice(&B256::left_padding_from(spender.as_slice())[..]);

    match block_on(async move {
        let provider = http_provider(&rpc)?;
        let raw = eth_call(&provider, contract, Bytes::from(data)).await?;
        Ok::<U256, block_error::Error>(decode_u256(raw.as_ref()))
    }) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

/// Send an arbitrary contract call, signed locally.
///
/// This is what executes a swap: an aggregator or the THORChain router hands back calldata
/// this wallet cannot read, and the protections around it are the checks in
/// `swap::safety` plus the caller's own gas ceiling, not any inspection of `data`.
///
/// `chain_id` is taken from the plan rather than the current network setting, so a
/// transaction built for one chain can never be replayed onto another.
#[allow(clippy::too_many_arguments)]
pub fn send_contract_call(
    private_key: &str,
    to: &str,
    data: &str,
    value: U256,
    gas_limit: u64,
    chain_id: u64,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
) -> Result<String, block_error::Error> {
    let network = parse_network(network_name);
    if chain_id != self::chain_id(network) {
        return Err(block_error::Error::new(
            "this transaction was built for a different network than the wallet is on"
                .to_string(),
        ));
    }
    let rpc = resolve_rpc(eth_node, network, infura_key);
    let signer = signer_from_key(private_key)?;
    let target = validate_address(to)?;
    let payload = decode_hex_payload(data)?;

    match block_on(send_contract_call_async(
        signer, target, payload, value, gas_limit, chain_id, rpc,
    )) {
        Ok(Ok(hash)) => Ok(hash),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

/// Block until `txid` is mined, failing if it reverted or is still unmined after `timeout`.
///
/// Read errors while polling are retried rather than returned: a flaky RPC is not evidence
/// that the transaction failed.
pub fn wait_for_success(
    txid: &str,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
    timeout: std::time::Duration,
) -> Result<(), block_error::Error> {
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    let hash = B256::from_str(txid.trim())
        .map_err(|_| block_error::Error::new(format!("invalid transaction hash {txid}")))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let status = block_on(async {
            let provider = http_provider(&rpc).ok()?;
            let receipt = provider.get_transaction_receipt(hash).await.ok()??;
            Some(receipt.status())
        })
        .ok()
        .flatten();
        match status {
            Some(true) => return Ok(()),
            Some(false) => {
                return Err(block_error::Error::new("it was mined but reverted".to_string()))
            }
            None if std::time::Instant::now() >= deadline => {
                return Err(block_error::Error::new(
                    "it has not been mined yet".to_string(),
                ))
            }
            None => std::thread::sleep(std::time::Duration::from_secs(3)),
        }
    }
}

fn decode_hex_payload(data: &str) -> Result<Bytes, block_error::Error> {
    let text = data.trim();
    let stripped = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).unwrap_or(text);
    if stripped.is_empty() {
        return Ok(Bytes::new());
    }
    let bytes = hex::decode(stripped)
        .map_err(|_| block_error::Error::new("provider returned unreadable calldata".to_string()))?;
    Ok(Bytes::from(bytes))
}

async fn send_contract_call_async(
    signer: PrivateKeySigner,
    to: Address,
    data: Bytes,
    value: U256,
    gas_limit: u64,
    chain_id: u64,
    rpc: String,
) -> Result<String, block_error::Error> {
    let from = signer.address();
    let provider = signed_provider(&rpc, signer)?;
    // `pending`, not the default `latest`: a transaction this wallet sent moments ago (the
    // approval ahead of a swap) must count, or this one reuses its nonce and is rejected.
    let nonce = provider
        .get_transaction_count(from)
        .pending()
        .await
        .map_err(|e| block_error::Error::new(format!("could not read nonce: {e}")))?;
    let tiers = fetch_fee_tiers_async(rpc.clone()).await.unwrap_or_default();
    let (max_fee_per_gas, max_priority_fee_per_gas) = fee_from_tier(&tiers, "medium");

    let tx = TransactionRequest::default()
        .with_from(from)
        .with_to(to)
        .with_chain_id(chain_id)
        .with_nonce(nonce)
        .with_gas_limit(gas_limit)
        .with_max_fee_per_gas(max_fee_per_gas)
        .with_max_priority_fee_per_gas(max_priority_fee_per_gas)
        .with_value(value)
        .with_input(data);

    let pending = provider
        .send_transaction(tx)
        .await
        .map_err(|e| block_error::Error::new(format!("broadcast failed: {e}")))?;
    Ok(format!("{:#x}", pending.tx_hash()))
}

pub fn fetch_token_metadata(
    contract: &str,
    eth_node: &str,
    network_name: &str,
    infura_key: &str,
) -> Result<RegistryToken, block_error::Error> {
    let address = validate_address(contract)?;
    let network = parse_network(network_name);
    let rpc = resolve_rpc(eth_node, network, infura_key);
    match block_on(fetch_token_metadata_async(address, rpc)) {
        Ok(Ok(token)) => Ok(token),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err),
    }
}

async fn fetch_token_metadata_async(
    address: Address,
    rpc: String,
) -> Result<RegistryToken, block_error::Error> {
    let provider = http_provider(&rpc)?;
    let decimals_raw = eth_call(&provider, address, encode_selector(SELECTOR_DECIMALS)).await?;
    let decimals: u8 = decode_u256(decimals_raw.as_ref())
        .try_into()
        .map_err(|_| block_error::Error::new("token decimals() was not a u8".into()))?;
    let symbol = eth_call(&provider, address, encode_selector(SELECTOR_SYMBOL))
        .await
        .ok()
        .and_then(|raw| decode_string(raw.as_ref()))
        .unwrap_or_else(|| format!("{address:?}")[..10].to_string());
    let name = eth_call(&provider, address, encode_selector(SELECTOR_NAME))
        .await
        .ok()
        .and_then(|raw| decode_string(raw.as_ref()))
        .unwrap_or_else(|| symbol.clone());
    Ok(RegistryToken {
        symbol,
        name,
        address: format!("{address:?}"),
        decimals,
        native: false,
    })
}

pub fn apply_bundled_tokens(tokens: &mut crate::currencies::tokens::Tokens, network: EthNetwork) {
    for item in bundled_tokens(network) {
        let logo = crate::configuration::paths::token_icon_path(&item.symbol);
        tokens.eth_tokens.insert(
            format!("eth:{}", item.symbol),
            Token {
                name: item.name,
                symbol: item.symbol,
                address: item.address,
                logo,
                decimals: item.decimals as i32,
                chain: "eth".to_string(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_networks_and_default_rpcs() {
        assert_eq!(parse_network("sepolia"), EthNetwork::Sepolia);
        assert_eq!(parse_network(""), EthNetwork::Mainnet);
        assert_eq!(chain_id(EthNetwork::Sepolia), 11155111);
        // Asserts the shape rather than one vendor's hostname; the previous version pinned
        // "llama" and so had to be edited when that endpoint died.
        for network in [EthNetwork::Mainnet, EthNetwork::Sepolia] {
            let rpc = resolve_rpc("", network, "");
            assert!(rpc.starts_with("https://"), "{network:?} default is not https: {rpc}");
        }
        assert!(resolve_rpc("", EthNetwork::Sepolia, "").contains("sepolia"));
        assert_eq!(
            resolve_rpc("https://my.node", EthNetwork::Mainnet, "abc"),
            "https://my.node"
        );
        assert!(resolve_rpc("", EthNetwork::Mainnet, "abc").contains("infura.io/v3/abc"));
    }

    #[test]
    fn l2_networks_have_correct_chain_ids_and_native_symbols() {
        let cases = [
            ("arbitrum", EthNetwork::ArbitrumOne, 42161u64, "ETH"),
            ("base", EthNetwork::Base, 8453, "ETH"),
            ("optimism", EthNetwork::Optimism, 10, "ETH"),
            ("polygon", EthNetwork::PolygonPos, 137, "POL"),
            ("bsc", EthNetwork::BnbSmartChain, 56, "BNB"),
            ("avalanche", EthNetwork::AvalancheCChain, 43114, "AVAX"),
        ];
        for (name, network, expected_chain_id, expected_symbol) in cases {
            assert_eq!(parse_network(name), network, "{name}");
            assert_eq!(chain_id(network), expected_chain_id, "{name}");
            assert_eq!(native_symbol(network), expected_symbol, "{name}");
            assert_eq!(network_name(network), name);
            assert!(!is_testnet(network), "{name} must not be treated as a testnet");
            assert!(!default_rpc(network).is_empty(), "{name}");
        }
        assert!(is_testnet(EthNetwork::Sepolia));
        assert!(!is_testnet(EthNetwork::Mainnet));
    }

    #[test]
    fn l2_bundled_tokens_use_native_symbol_and_no_evm_collides_with_another() {
        for network in [
            EthNetwork::ArbitrumOne,
            EthNetwork::Base,
            EthNetwork::Optimism,
            EthNetwork::PolygonPos,
            EthNetwork::BnbSmartChain,
            EthNetwork::AvalancheCChain,
        ] {
            let tokens = bundled_tokens(network);
            let native = tokens.iter().find(|t| t.native).expect("native entry");
            assert_eq!(native.symbol, native_symbol(network));
            assert_eq!(native.address, NATIVE_SENTINEL);
            // Every non-native bundled entry must be a distinct, non-native-sentinel contract.
            for t in tokens.iter().filter(|t| !t.native) {
                assert_ne!(t.address, NATIVE_SENTINEL);
            }
        }
        // Binance-Peg stablecoins use 18 decimals, unlike the 6 that USDC and USDT carry
        // nearly everywhere else. Looked up by symbol rather than by position: the lists are
        // generated in alphabetical order, so indexing into one asserts nothing useful and
        // breaks whenever a token is added ahead of it.
        let bsc = bundled_tokens(EthNetwork::BnbSmartChain);
        for symbol in ["USDT", "USDC"] {
            let stable = bsc
                .iter()
                .find(|t| t.symbol == symbol)
                .unwrap_or_else(|| panic!("BSC should bundle {symbol}"));
            assert_eq!(stable.decimals, 18, "Binance-Peg {symbol} is an 18-decimal token");
        }
        // The 6-decimal assumption still holds where it should.
        let mainnet = bundled_tokens(EthNetwork::Mainnet);
        let usdc = mainnet.iter().find(|t| t.symbol == "USDC").expect("mainnet USDC");
        assert_eq!(usdc.decimals, 6);
    }

    #[test]
    fn validates_addresses_and_rejects_ens() {
        let addr = validate_address("0x9858Eff28F61CF0aDe1AC00482789d2EF5e6d47E").unwrap();
        assert_eq!(format!("{addr:?}").len(), 42);
        assert!(validate_address("not-an-address").is_err());
        assert!(validate_address("vitalik.eth").is_err());
        assert!(validate_address("").is_err());
    }

    #[test]
    fn parses_and_formats_token_amounts() {
        assert_eq!(parse_token_amount("1", 18).unwrap(), U256::from(10).pow(U256::from(18)));
        assert_eq!(
            parse_token_amount("1.5", 6).unwrap(),
            U256::from(1_500_000u64)
        );
        assert_eq!(parse_token_amount(".5", 2).unwrap(), U256::from(50u64));
        assert!(parse_token_amount("1.2345678", 6).is_err());
        assert!(parse_token_amount("", 18).is_err());
        assert_eq!(format_units_trimmed(U256::from(1_500_000u64), 6), "1.5");
        assert_eq!(format_units_trimmed(U256::from(10).pow(U256::from(18)), 18), "1");
    }

    #[test]
    fn bundled_lists_differ_by_network() {
        let main = bundled_tokens(EthNetwork::Mainnet);
        let sepolia = bundled_tokens(EthNetwork::Sepolia);
        assert!(main.iter().any(|t| t.symbol == "DAI"));
        assert!(!sepolia.iter().any(|t| t.symbol == "DAI"));
        assert!(sepolia.iter().any(|t| t.symbol == "USDC"));
        assert!(main.iter().any(|t| t.native));
    }

    #[test]
    fn prepared_send_summary_includes_symbol_and_fee() {
        let native = PreparedSend {
            from: "0x9858EfFD232B4033E47d90003D41EC34EcaEda94".into(),
            to: "0x9858Eff28F61CF0aDe1AC00482789d2EF5e6d47E".into(),
            token_symbol: "ETH".into(),
            token_address: None,
            amount: U256::from(10).pow(U256::from(16)),
            amount_display: "0.01".into(),
            gas_limit: 21_000,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 100_000_000,
            fee_wei: U256::from(21_000) * U256::from(1_000_000_000u64),
            fee_symbol: "ETH".into(),
            chain_id: 11155111,
            nonce: 0,
        };
        let text = native.summary();
        assert!(text.contains("0.01 ETH"));
        assert!(text.contains("chain 11155111"));
        let erc20 = PreparedSend {
            token_symbol: "USDC".into(),
            token_address: Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into()),
            amount_display: "2.5".into(),
            ..native
        };
        assert!(erc20.summary().contains("2.5 USDC"));
        assert!(erc20.summary().contains("paid in ETH"));
    }

    #[test]
    fn transfer_calldata_is_nonempty() {
        let to = Address::from_str("0x9858Eff28F61CF0aDe1AC00482789d2EF5e6d47E").unwrap();
        let data = encode_transfer(to, U256::from(1u64));
        assert_eq!(&data[..4], &SELECTOR_TRANSFER);
        assert_eq!(data.len(), 68);
        let balance_of = encode_balance_of(to);
        assert_eq!(&balance_of[..4], &SELECTOR_BALANCE_OF);
        assert_eq!(balance_of.len(), 36);
    }

    /// The bundled list is hand-maintained, so the mistakes worth guarding against are the
    /// clerical ones: a symbol listed twice on one network (the second silently overwrites the
    /// first in the registry, so the wallet would show one token and spend another), an
    /// address that is not a valid contract address, or a copy-paste that leaves two symbols
    /// pointing at the same contract.
    ///
    /// It cannot check that an address is the *right* contract. That was done separately by
    /// calling `symbol()` and `decimals()` on each one before it went in.
    #[test]
    fn every_bundled_token_list_is_internally_consistent() {
        for network in [
            EthNetwork::Mainnet,
            EthNetwork::Sepolia,
            EthNetwork::ArbitrumOne,
            EthNetwork::Base,
            EthNetwork::Optimism,
            EthNetwork::PolygonPos,
            EthNetwork::BnbSmartChain,
            EthNetwork::AvalancheCChain,
        ] {
            let tokens = bundled_tokens(network);
            let name = network_name(network);
            assert!(!tokens.is_empty(), "{name} bundles no tokens at all");

            let natives = tokens.iter().filter(|t| t.native).count();
            assert_eq!(natives, 1, "{name} must bundle exactly one native asset");

            let mut symbols: Vec<&str> = tokens.iter().map(|t| t.symbol.as_str()).collect();
            symbols.sort_unstable();
            let before = symbols.len();
            symbols.dedup();
            assert_eq!(before, symbols.len(), "{name} bundles a duplicate symbol");

            let mut addresses: Vec<String> = Vec::new();
            for token in &tokens {
                if token.native {
                    continue;
                }
                assert!(
                    validate_address(&token.address).is_ok(),
                    "{name} bundles {} with an invalid address {}",
                    token.symbol,
                    token.address
                );
                assert!(
                    token.decimals <= 18,
                    "{name} bundles {} with implausible decimals {}",
                    token.symbol,
                    token.decimals
                );
                let lower = token.address.to_ascii_lowercase();
                assert!(
                    !addresses.contains(&lower),
                    "{name} bundles two symbols at address {}",
                    token.address
                );
                addresses.push(lower);
            }
        }
    }

    #[test]
    fn fee_tiers_map_labels() {
        let tiers = FeeTiers {
            low: 10,
            medium: 20,
            high: 30,
            priority: 4,
        };
        assert_eq!(fee_from_tier(&tiers, "low").0, 10);
        assert_eq!(fee_from_tier(&tiers, "High").0, 30);
        assert_eq!(fee_from_tier(&tiers, "medium").1, 4);
    }

    #[test]
    fn an_erc20_reporting_eth_is_not_the_native_token() {
        let mut tokens = crate::currencies::tokens::Tokens { eth_tokens: Default::default(), token_transfers: Default::default() };
        apply_bundled_tokens(&mut tokens, EthNetwork::BnbSmartChain);
        let eth = tokens.eth_tokens.get("eth:ETH").expect("BSC bundles an ETH ERC-20");
        assert!(!is_native_token(eth));
        assert!(is_native_token(tokens.eth_tokens.get("eth:BNB").unwrap()));
    }

    #[test]
    fn the_tip_never_exceeds_the_max_fee() {
        // BNB Smart Chain: base fee 0, so the estimate's max fee equals its tip, and Low
        // takes 80% of the max fee only.
        let tiers = FeeTiers {
            low: 800,
            medium: 1_000,
            high: 1_300,
            priority: 1_000,
        };
        for label in ["low", "medium", "high"] {
            let (max_fee, tip) = fee_from_tier(&tiers, label);
            assert!(tip <= max_fee, "{label}: tip {tip} > max fee {max_fee}");
        }
        assert_eq!(fee_from_tier(&tiers, "low"), (800, 800));
    }
}

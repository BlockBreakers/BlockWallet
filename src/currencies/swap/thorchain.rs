//! THORChain: the only non-custodial route this wallet has for swapping BTC or LTC.
//!
//! Neither Bitcoin nor Litecoin has an on-chain DEX, so a same-chain aggregator cannot help
//! them. THORChain settles cross-chain by holding liquidity in vaults on each chain: you pay
//! the inbound vault on the source chain with a memo describing what you want, and the network
//! pays your address on the destination chain. No company takes ownership, but the funds are
//! genuinely out of your control between the two legs, which is why [`Custody::ProtocolVault`]
//! is reported and shown to the user.
//!
//! # Failure modes this guards against
//!
//! THORChain forfeits, rather than refunds, several classes of mistake:
//!
//! * paying a vault while the chain or the network is halted;
//! * paying less than the inbound minimum;
//! * paying a stale vault address after a churn.
//!
//! So `inbound_addresses` is re-read at quote time and never cached, the halt flags are treated
//! as hard refusals, and the minimum is enforced in [`super::safety`] rather than displayed.
//!
//! As of this writing the network's global `HALTTRADING` mimir is set, so `halted` comes back
//! true for every chain and quoting refuses. That is the correct behaviour, not a bug.

use serde_json::Value;

use crate::configuration::block_error;
use crate::configuration::http;
use crate::currencies::swap::{
    Custody, SwapAsset, SwapExecution, SwapProvider, SwapQuote, SwapRequest,
};

/// Which affiliate payout address a venue draws on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeChain {
    Thorchain,
    Maya,
}

/// One THORChain-shaped network.
///
/// Maya Protocol is a THORChain fork and speaks the same API under a different path prefix,
/// so it is a second instance of this provider rather than a second copy of it. Two venues
/// that share a wire format should share the halt checks, the vault-agreement check and the
/// memo handling: those are exactly the parts that lose money when they are subtly different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Venue {
    pub id: &'static str,
    pub name: &'static str,
    /// Gateways tried in order. A list rather than a single host because a public gateway
    /// going away is not hypothetical: `thornode.ninerealms.com`, the long-standing default,
    /// no longer resolves at all, and with one hardcoded host that took the whole venue down
    /// while looking like the user being offline.
    pub gateways: &'static [&'static str],
    /// Path prefix the REST API is mounted under.
    pub api: &'static str,
    /// Source chains it can take an inbound payment on, as internal chain keys.
    pub inbound_chains: &'static [&'static str],
    /// Which payout address in [`super::FeePayout`] belongs to this network. They are
    /// separate chains, and paying a `thor1…` address on Maya would simply lose the fee.
    pub payout: FeeChain,
    /// Whether the user's configured THORNode URL applies. It does not for Maya, which is a
    /// separate network: pointing a Maya quote at a THORNode would ask the wrong chain for
    /// vaults, and could hand back an address belonging to neither.
    pub honours_node_override: bool,
}

/// Public THORNode gateway used when the user has not set their own.
///
/// THORChain is a permissionless network but its public API hosts are not, and they rate-limit
/// to roughly one request a second, so the Settings field for this exists.
///
/// Every public gateway below was probed during this pass and none answered: ninerealms has
/// no DNS record, thorswap sits behind a Cloudflare challenge, and liquify serves an expired
/// certificate. They are kept and tried in turn because gateways come back, and because a
/// wrong-but-plausible replacement would be worse than an honest failure. When they are all
/// down the provider says so and Maya carries the cross-chain routes.
pub const DEFAULT_THORNODE: &str = "https://thornode.ninerealms.com";

pub const THORCHAIN: Venue = Venue {
    id: "thorchain",
    name: "THORChain",
    gateways: &[
        DEFAULT_THORNODE,
        "https://thornode.thorswap.net",
        "https://thornode.thorchain.liquify.com",
    ],
    api: "thorchain",
    payout: FeeChain::Thorchain,
    inbound_chains: &["btc", "ltc", "eth"],
    honours_node_override: true,
};

/// Maya Protocol: a THORChain fork with its own validator set, vaults and liquidity.
///
/// Worth quoting alongside THORChain rather than instead of it. The pools are different
/// sizes, so the better price genuinely varies by pair, and the two networks halt
/// independently: during this pass Maya was the only one of the two whose API was reachable
/// at all.
///
/// No Litecoin: Maya does not pool it, and `supports` says so before a pointless round trip.
pub const MAYA: Venue = Venue {
    id: "maya",
    name: "Maya Protocol",
    gateways: &["https://mayanode.mayachain.info"],
    api: "mayachain",
    payout: FeeChain::Maya,
    inbound_chains: &["btc", "eth"],
    honours_node_override: false,
};

pub struct ThorChain {
    venue: &'static Venue,
}

impl ThorChain {
    pub fn new() -> Self {
        Self { venue: &THORCHAIN }
    }

    pub fn maya() -> Self {
        Self { venue: &MAYA }
    }
}

impl Default for ThorChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Gateways to try, in order: the user's own first when this venue honours the setting.
fn gateways(venue: &Venue, request: &SwapRequest) -> Vec<String> {
    let mut out = Vec::new();
    if venue.honours_node_override {
        let node = request.thornode_url.trim();
        if !node.is_empty() {
            out.push(node.trim_end_matches('/').to_string());
        }
    }
    for gateway in venue.gateways {
        let gateway = gateway.trim_end_matches('/').to_string();
        if !out.contains(&gateway) {
            out.push(gateway);
        }
    }
    out
}

/// One chain's entry from `/thorchain/inbound_addresses`.
#[derive(Clone, Debug, PartialEq)]
pub struct InboundAddress {
    pub chain: String,
    pub address: String,
    pub router: Option<String>,
    pub halted: bool,
    pub global_trading_paused: bool,
    pub chain_trading_paused: bool,
    pub gas_rate: Option<f32>,
    pub dust_threshold: Option<u64>,
}

impl InboundAddress {
    /// Any of the three flags is a refusal. They mean slightly different things, but from the
    /// wallet's side the consequence is identical: coins paid in now may not come back.
    pub fn is_tradable(&self) -> bool {
        !self.halted && !self.global_trading_paused && !self.chain_trading_paused
    }

    /// Phrased without a venue name so the caller can prefix whichever network it asked.
    pub fn halt_reason(&self) -> &'static str {
        if self.global_trading_paused {
            "has paused trading network-wide"
        } else if self.chain_trading_paused {
            "has paused trading on this chain"
        } else {
            "has halted this chain"
        }
    }
}

/// Parse the `inbound_addresses` array.
///
/// Split out from the fetch so the halt logic is testable against captured responses without
/// depending on the live network state, which by its nature changes.
pub fn parse_inbound_addresses(json: &Value) -> Vec<InboundAddress> {
    let Some(entries) = json.as_array() else { return Vec::new() };
    entries
        .iter()
        .filter_map(|entry| {
            let chain = entry.get("chain")?.as_str()?.to_string();
            let address = entry.get("address")?.as_str()?.to_string();
            let flag = |key: &str| entry.get(key).and_then(Value::as_bool).unwrap_or(false);
            Some(InboundAddress {
                chain,
                address,
                router: entry
                    .get("router")
                    .and_then(Value::as_str)
                    .filter(|r| !r.is_empty())
                    .map(str::to_string),
                halted: flag("halted"),
                global_trading_paused: flag("global_trading_paused"),
                chain_trading_paused: flag("chain_trading_paused"),
                gas_rate: entry
                    .get("gas_rate")
                    .and_then(Value::as_str)
                    .and_then(|g| g.parse::<f32>().ok()),
                dust_threshold: entry
                    .get("dust_threshold")
                    .and_then(Value::as_str)
                    .and_then(|d| d.parse::<u64>().ok()),
            })
        })
        .collect()
}

/// Read the vault listing, trying each gateway until one answers.
///
/// The chain argument is the venue's own spelling ("BTC", "ETH"), matched case-insensitively
/// against what the listing returns.
fn fetch_inbound(
    venue: &Venue,
    gateways: &[String],
    chain: &str,
) -> Result<(String, InboundAddress), block_error::Error> {
    let mut last: Option<block_error::Error> = None;
    for base in gateways {
        let text = match http::get_text(&format!("{base}/{}/inbound_addresses", venue.api)) {
            Ok(text) => text,
            Err(why) => {
                last = Some(why);
                continue;
            }
        };
        let json: Value = match serde_json::from_str(&text) {
            Ok(json) => json,
            Err(e) => {
                last = Some(block_error::Error::new(format!(
                    "invalid inbound_addresses response: {e}"
                )));
                continue;
            }
        };
        return parse_inbound_addresses(&json)
            .into_iter()
            .find(|entry| entry.chain.eq_ignore_ascii_case(chain))
            .map(|entry| (base.clone(), entry))
            .ok_or_else(|| {
                block_error::Error::new(format!(
                    "{} does not currently serve the {chain} chain",
                    venue.name
                ))
            });
    }
    // Every gateway failed. Report that as one line naming how many were tried and why the
    // last one refused, rather than surfacing a raw transport error as if a single host had
    // been asked. The count is the useful part: it distinguishes "this gateway is down" from
    // "the whole public gateway layer is down", which is the actual situation for THORChain.
    let tried = gateways.len();
    Err(block_error::Error::new(match last {
        Some(why) => format!(
            "no {} gateway answered ({tried} tried); last said: {why}",
            venue.name
        ),
        None => format!("no {} gateway answered ({tried} tried)", venue.name),
    }))
}

/// The subset of `/thorchain/quote/swap` this wallet acts on.
#[derive(Clone, Debug, PartialEq)]
pub struct ThorQuote {
    pub inbound_address: String,
    pub memo: String,
    pub expected_amount_out: u128,
    pub expiry: Option<u64>,
    pub total_swap_seconds: Option<u64>,
    pub recommended_min_amount_in: Option<u128>,
    pub recommended_gas_rate: Option<f32>,
    pub fees_total: Option<u128>,
    pub router: Option<String>,
}

/// Parse a quote response, or surface the node's own error message.
pub fn parse_quote(json: &Value, venue_name: &str) -> Result<ThorQuote, block_error::Error> {
    // The node reports refusals as `{"error": "..."}` with a 400, and those messages are
    // genuinely useful ("swapping is halted", "amount is less than fee"), so they are passed
    // through rather than replaced with something generic.
    if let Some(message) = json.get("error").and_then(Value::as_str) {
        return Err(block_error::Error::new(format!("{venue_name} declined: {message}")));
    }

    let str_field = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_string);
    let u128_field = |key: &str| {
        json.get(key)
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<u128>().ok())
    };

    let inbound_address = str_field("inbound_address")
        .ok_or_else(|| block_error::Error::new("quote gave no inbound address".to_string()))?;
    let memo = str_field("memo").ok_or_else(|| block_error::Error::new("quote gave no memo".to_string()))?;
    let expected_amount_out = u128_field("expected_amount_out")
        .ok_or_else(|| block_error::Error::new("quote gave no expected output".to_string()))?;

    Ok(ThorQuote {
        inbound_address,
        memo,
        expected_amount_out,
        expiry: json.get("expiry").and_then(Value::as_u64),
        total_swap_seconds: json.get("total_swap_seconds").and_then(Value::as_u64),
        recommended_min_amount_in: u128_field("recommended_min_amount_in"),
        recommended_gas_rate: json
            .get("recommended_gas_rate")
            .and_then(Value::as_str)
            .and_then(|g| g.parse::<f32>().ok()),
        fees_total: json
            .get("fees")
            .and_then(|f| f.get("total"))
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<u128>().ok()),
        router: str_field("router").filter(|r| !r.is_empty()),
    })
}

impl SwapProvider for ThorChain {
    fn id(&self) -> &'static str {
        self.venue.id
    }

    fn display_name(&self) -> &'static str {
        self.venue.name
    }

    fn custody(&self) -> Custody {
        Custody::ProtocolVault
    }

    fn supports(&self, from: &SwapAsset, to: &SwapAsset) -> bool {
        // Both sides must be assets the venue pools, both chains must be ones it serves, and
        // there is no point routing a same-chain same-asset pair through a cross-chain
        // protocol.
        from.thorchain_notation().is_some()
            && to.thorchain_notation().is_some()
            && self.venue.inbound_chains.contains(&from.chain.as_str())
            && self.venue.inbound_chains.contains(&to.chain.as_str())
            && !(from.chain == to.chain && from.symbol == to.symbol)
    }

    fn quote(&self, request: &SwapRequest) -> Result<SwapQuote, block_error::Error> {
        let venue = self.venue;
        let from_asset = request.from.thorchain_notation().ok_or_else(|| {
            block_error::Error::new(format!("{} does not pool this input asset", venue.name))
        })?;
        let to_asset = request.to.thorchain_notation().ok_or_else(|| {
            block_error::Error::new(format!("{} does not pool this output asset", venue.name))
        })?;

        let source_chain = match request.from.chain.as_str() {
            "btc" => "BTC",
            "ltc" => "LTC",
            "eth" => "ETH",
            other => {
                return Err(block_error::Error::new(format!(
                    "{} cannot take an inbound payment on {other}",
                    venue.name
                )))
            }
        };
        if !venue.inbound_chains.contains(&request.from.chain.as_str()) {
            return Err(block_error::Error::new(format!(
                "{} has no {source_chain} pool",
                venue.name
            )));
        }

        // Read before quoting: a halted chain must be refused before the user is shown a
        // price they might act on. Never cached, because vaults churn. The gateway that
        // answered is the one the quote is then asked of, so both halves of the check come
        // from the same node rather than from two that might disagree.
        let (base, inbound) = fetch_inbound(venue, &gateways(venue, request), source_chain)?;
        if !inbound.is_tradable() {
            return Err(block_error::Error::new(format!(
                "{} {} right now, so this swap cannot be made safely",
                venue.name,
                inbound.halt_reason()
            )));
        }

        let thor_amount = request.from.to_thorchain_units(request.amount_in_base);
        if thor_amount == 0 {
            return Err(block_error::Error::new(format!(
                "amount is too small for {} to represent",
                venue.name
            )));
        }

        // `liquidity_tolerance_bps` makes the node embed a real limit in the memo. Without it
        // the memo comes back with a limit of 0, which means the swap will execute at any
        // price at all.
        let tolerance = request.slippage_bps.clamp(1, super::safety::MAX_SLIPPAGE_BPS);
        // The affiliate is handed to the node, which builds it into the memo it returns.
        // Doing it this way rather than editing the memo ourselves means the fee the user is
        // shown is the one the network will actually apply, and `safety::memo_destination`
        // already ignores the trailing affiliate fields when it checks where proceeds land.
        let payout = match venue.payout {
            FeeChain::Thorchain => request.fee.thorchain.trim(),
            FeeChain::Maya => request.fee.maya.trim(),
        };
        let fee_bps = request.fee.bps_for(payout);
        let affiliate = if fee_bps > 0 {
            format!("&affiliate={payout}&affiliate_bps={fee_bps}")
        } else {
            String::new()
        };
        let url = format!(
            "{base}/{}/quote/swap?from_asset={from_asset}&to_asset={to_asset}\
             &amount={thor_amount}&destination={}&liquidity_tolerance_bps={tolerance}{affiliate}",
            venue.api,
            request.destination.trim()
        );

        let text = http::get_text(&url)?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|e| block_error::Error::new(format!("invalid quote response: {e}")))?;
        let quote = parse_quote(&json, venue.name)?;

        // The vault to pay comes from the quote, but it must agree with the inbound listing
        // read moments ago. A disagreement means a churn happened mid-quote, or one of the two
        // responses is not to be trusted; either way, paying is the wrong move.
        if !quote.inbound_address.eq_ignore_ascii_case(&inbound.address) {
            return Err(block_error::Error::new(format!(
                "the quote and vault listing from {} disagree on the inbound address; \
                 try again in a moment",
                venue.name
            )));
        }

        let expected_out_base = request.to.from_thorchain_units(quote.expected_amount_out);
        // The network enforces the limit embedded in the memo, so the floor the wallet shows
        // is derived from the tolerance that was actually requested.
        let min_out_base = expected_out_base
            .saturating_mul(u128::from(10_000u32.saturating_sub(tolerance)))
            / 10_000;

        let min_in_base = quote
            .recommended_min_amount_in
            .map(|min| request.from.from_thorchain_units(min));

        let execution = match request.from.chain.as_str() {
            "btc" | "ltc" => SwapExecution::UtxoWithMemo {
                vault: quote.inbound_address.clone(),
                amount_base: u64::try_from(request.amount_in_base).map_err(|_| {
                    block_error::Error::new("amount is too large for this chain".to_string())
                })?,
                memo: quote.memo.clone(),
                gas_rate: quote.recommended_gas_rate.or(inbound.gas_rate),
            },
            "eth" => {
                let router = quote
                    .router
                    .clone()
                    .or_else(|| inbound.router.clone())
                    .ok_or_else(|| {
                        block_error::Error::new(format!(
                            "{} gave no router contract for the Ethereum leg",
                            venue.name
                        ))
                    })?;
                build_eth_deposit(request, &router, &quote)?
            }
            other => {
                return Err(block_error::Error::new(format!(
                    "{} cannot take an inbound payment on {other}",
                    venue.name
                )))
            }
        };

        Ok(SwapQuote {
            provider_id: self.id(),
            provider_name: self.display_name(),
            custody: self.custody(),
            from: request.from.clone(),
            to: request.to.clone(),
            amount_in_base: request.amount_in_base,
            expected_out_base,
            min_out_base,
            destination: request.destination.clone(),
            expiry: quote.expiry,
            eta_seconds: quote.total_swap_seconds,
            route_note: None,
            // Already inclusive of the affiliate cut: THORChain reports fees.total as
            // outbound + liquidity + affiliate.
            fee_total_base: quote
                .fees_total
                .map(|total| request.to.from_thorchain_units(total)),
            min_in_base,
            fee_bps,
            execution,
        })
    }
}

/// ABI-encode `depositWithExpiry(address,address,uint256,string,uint256)` for the THORChain
/// router.
///
/// Hand-rolled to match the style already used for ERC-20 calls in `eth_chain`, and because
/// pulling in a full ABI codec for one function would be a large dependency for a small job.
/// The dynamic `string` argument is why this is not just four static words: its offset goes in
/// the head, and its length-prefixed, 32-byte-padded body goes in the tail.
pub fn encode_deposit_with_expiry(
    vault: &str,
    asset: &str,
    amount: u128,
    memo: &str,
    expiry: u64,
) -> Result<String, block_error::Error> {
    fn address_word(value: &str) -> Result<[u8; 32], block_error::Error> {
        let hex = value.trim().trim_start_matches("0x").trim_start_matches("0X");
        let bytes = hex::decode(hex)
            .map_err(|_| block_error::Error::new(format!("invalid address {value:?}")))?;
        if bytes.len() != 20 {
            return Err(block_error::Error::new(format!("invalid address {value:?}")));
        }
        let mut word = [0u8; 32];
        word[12..].copy_from_slice(&bytes);
        Ok(word)
    }

    fn u256_word(value: u128) -> [u8; 32] {
        let mut word = [0u8; 32];
        word[16..].copy_from_slice(&value.to_be_bytes());
        word
    }

    // keccak256("depositWithExpiry(address,address,uint256,string,uint256)")[0..4]
    const SELECTOR: [u8; 4] = [0x44, 0xbc, 0x93, 0x7b];

    let mut out = Vec::with_capacity(4 + 32 * 8);
    out.extend_from_slice(&SELECTOR);
    out.extend_from_slice(&address_word(vault)?);
    out.extend_from_slice(&address_word(asset)?);
    out.extend_from_slice(&u256_word(amount));
    // Head slot for the dynamic string: offset from the start of the argument block. Five
    // arguments precede the tail, so the body begins at 5 * 32 bytes.
    out.extend_from_slice(&u256_word(5 * 32));
    out.extend_from_slice(&u256_word(u128::from(expiry)));
    // Tail: length, then the bytes, right-padded to a 32-byte boundary.
    let memo_bytes = memo.as_bytes();
    out.extend_from_slice(&u256_word(memo_bytes.len() as u128));
    out.extend_from_slice(memo_bytes);
    let padding = (32 - (memo_bytes.len() % 32)) % 32;
    out.extend(std::iter::repeat(0u8).take(padding));

    Ok(format!("0x{}", hex::encode(out)))
}

fn build_eth_deposit(
    request: &SwapRequest,
    router: &str,
    quote: &ThorQuote,
) -> Result<SwapExecution, block_error::Error> {
    // The router takes the zero address to mean "the chain's native asset".
    const NATIVE_ASSET: &str = "0x0000000000000000000000000000000000000000";
    let asset = if request.from.native {
        NATIVE_ASSET.to_string()
    } else {
        request.from.address.clone()
    };

    // The router's own expiry, distinct from the quote's: it makes the deposit revert rather
    // than sit in the vault if it is mined long after it was built.
    let expiry = quote
        .expiry
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                + 900
        });

    let data = encode_deposit_with_expiry(
        &quote.inbound_address,
        &asset,
        request.amount_in_base,
        &quote.memo,
        expiry,
    )?;

    Ok(SwapExecution::EvmCall {
        chain_id: 1,
        to: router.to_string(),
        data,
        value: if request.from.native {
            request.amount_in_base.to_string()
        } else {
            "0".to_string()
        },
        // Deposits into the router cost meaningfully more than a plain transfer, and the
        // node's own outbound_tx_size guidance for ETH is 100k.
        gas_limit: 150_000,
        approval_spender: if request.from.native { None } else { Some(router.to_string()) },
        approval_amount: if request.from.native {
            None
        } else {
            Some(request.amount_in_base.to_string())
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `inbound_addresses` response, captured while the network's global trading halt
    /// was in force. Kept verbatim so the halt handling is tested against what the API
    /// actually returns rather than what it is assumed to return.
    const CAPTURED_INBOUND: &str = r#"[
      {"chain":"BTC","pub_key":"thorpub1addwnpepq2w263ppn263cgjtjy583g0qsx3gdwq0qee304cfm8v03hzx86kvvqve29d",
       "address":"bc1qp6yzmq5kjr8yvyw7453gxvq4z3tvkdyadqm794","halted":true,"global_trading_paused":true,
       "chain_trading_paused":false,"chain_lp_actions_paused":true,"gas_rate":"3","gas_rate_units":"satsperbyte",
       "outbound_tx_size":"1000","outbound_fee":"906","dust_threshold":"1000"},
      {"chain":"LTC","pub_key":"thorpub1addwnpepq2w263ppn263cgjtjy583g0qsx3gdwq0qee304cfm8v03hzx86kvvqve29d",
       "address":"ltc1qp6yzmq5kjr8yvyw7453gxvq4z3tvkdyafup6a9","halted":true,"global_trading_paused":true,
       "chain_trading_paused":false,"chain_lp_actions_paused":true,"gas_rate":"813","gas_rate_units":"satsperbyte",
       "outbound_tx_size":"250","outbound_fee":"1355","dust_threshold":"100000"},
      {"chain":"ETH","pub_key":"thorpub1addwnpepq2w263ppn263cgjtjy583g0qsx3gdwq0qee304cfm8v03hzx86kvvqve29d",
       "address":"0xed23d1bf7ac2ce8b4c09a090e252ea6ee5145e6e","router":"0xD37BbE5744D730a1d98d8DC97c42F0Ca46aD7146",
       "halted":true,"global_trading_paused":true,"chain_trading_paused":false,"chain_lp_actions_paused":true,
       "gas_rate":"15","gas_rate_units":"gwei","outbound_tx_size":"100000","outbound_fee":"1000",
       "dust_threshold":"1000"}
    ]"#;

    #[test]
    fn captured_inbound_response_parses_every_field_we_rely_on() {
        let json: Value = serde_json::from_str(CAPTURED_INBOUND).unwrap();
        let entries = parse_inbound_addresses(&json);
        assert_eq!(entries.len(), 3);

        let btc = entries.iter().find(|e| e.chain == "BTC").unwrap();
        assert_eq!(btc.address, "bc1qp6yzmq5kjr8yvyw7453gxvq4z3tvkdyadqm794");
        assert_eq!(btc.gas_rate, Some(3.0));
        assert_eq!(btc.dust_threshold, Some(1_000));
        assert!(btc.router.is_none());

        let eth = entries.iter().find(|e| e.chain == "ETH").unwrap();
        assert_eq!(eth.router.as_deref(), Some("0xD37BbE5744D730a1d98d8DC97c42F0Ca46aD7146"));
        assert_eq!(eth.gas_rate, Some(15.0));
    }

    #[test]
    fn a_halted_network_is_never_tradable() {
        // This is the live state of the network as captured. Quoting must refuse, because
        // coins paid into a halted vault are not reliably recoverable.
        let json: Value = serde_json::from_str(CAPTURED_INBOUND).unwrap();
        for entry in parse_inbound_addresses(&json) {
            assert!(!entry.is_tradable(), "{} should not be tradable", entry.chain);
            assert_eq!(entry.halt_reason(), "has paused trading network-wide");
        }
    }

    #[test]
    fn each_halt_flag_independently_blocks_trading() {
        let base = InboundAddress {
            chain: "BTC".into(),
            address: "bc1q".into(),
            router: None,
            halted: false,
            global_trading_paused: false,
            chain_trading_paused: false,
            gas_rate: None,
            dust_threshold: None,
        };
        assert!(base.is_tradable());

        let halted = InboundAddress { halted: true, ..base.clone() };
        assert!(!halted.is_tradable());
        assert_eq!(halted.halt_reason(), "has halted this chain");

        let chain_paused = InboundAddress { chain_trading_paused: true, ..base.clone() };
        assert!(!chain_paused.is_tradable());
        assert_eq!(chain_paused.halt_reason(), "has paused trading on this chain");

        let global = InboundAddress { global_trading_paused: true, ..base };
        assert!(!global.is_tradable());
    }

    #[test]
    fn quote_response_parses_the_documented_shape() {
        let body = r#"{
          "inbound_address":"bc1qt9723ak9t7lu7a97lt9kelq4gnrlmyvk4yhzwr",
          "outbound_delay_seconds":1074,
          "fees":{"asset":"ETH.ETH","affiliate":"0","outbound":"54840","liquidity":"2037232",
                  "total":"2092072","slippage_bps":9,"total_bps":10},
          "expiry":1722575316,
          "dust_threshold":"10000",
          "recommended_min_amount_in":"10760",
          "recommended_gas_rate":"4",
          "gas_rate_units":"satsperbyte",
          "memo":"=:ETH.ETH:0x86d526d6624AbC0178cF7296cD538Ecc080A95F1:0/1/0",
          "expected_amount_out":"2035299208",
          "total_swap_seconds":1674
        }"#;
        let quote = parse_quote(&serde_json::from_str(body).unwrap(), "THORChain").unwrap();
        assert_eq!(quote.inbound_address, "bc1qt9723ak9t7lu7a97lt9kelq4gnrlmyvk4yhzwr");
        assert_eq!(quote.expected_amount_out, 2_035_299_208);
        assert_eq!(quote.expiry, Some(1_722_575_316));
        assert_eq!(quote.recommended_min_amount_in, Some(10_760));
        assert_eq!(quote.recommended_gas_rate, Some(4.0));
        assert_eq!(quote.fees_total, Some(2_092_072));
        assert_eq!(quote.total_swap_seconds, Some(1674));
    }

    #[test]
    fn a_node_error_is_surfaced_rather_than_swallowed() {
        let body = r#"{"error":"swapping is halted"}"#;
        let err = parse_quote(&serde_json::from_str(body).unwrap(), "THORChain").unwrap_err();
        assert!(format!("{err}").contains("swapping is halted"));
    }

    #[test]
    fn supported_pairs_exclude_solana_and_no_op_swaps() {
        let thor = ThorChain::new();
        let btc = SwapAsset { chain: "btc".into(), symbol: "BTC".into(), address: String::new(), decimals: 8, native: true };
        let eth = SwapAsset { chain: "eth".into(), symbol: "ETH".into(), address: String::new(), decimals: 18, native: true };
        let sol = SwapAsset { chain: "sol".into(), symbol: "SOL".into(), address: String::new(), decimals: 9, native: true };

        assert!(thor.supports(&btc, &eth));
        assert!(thor.supports(&eth, &btc));
        assert!(!thor.supports(&btc, &sol));
        assert!(!thor.supports(&sol, &eth));
        assert!(!thor.supports(&btc, &btc));
    }

    #[test]
    fn the_router_selector_is_the_real_keccak_hash() {
        // Hardcoding a function selector from memory is exactly the kind of thing that
        // silently sends funds into a call that reverts, or worse, hits a different function.
        // Derived here from the signature rather than trusted.
        let hash = alloy::primitives::keccak256(
            b"depositWithExpiry(address,address,uint256,string,uint256)",
        );
        assert_eq!(&hash[0..4], &[0x44, 0xbc, 0x93, 0x7b]);
    }

    #[test]
    fn deposit_calldata_is_abi_correct() {
        let vault = "0xed23d1bf7ac2ce8b4c09a090e252ea6ee5145e6e";
        let memo = "=:BTC.BTC:bc1qtest";
        let data = encode_deposit_with_expiry(
            vault,
            "0x0000000000000000000000000000000000000000",
            1_000_000_000_000_000_000,
            memo,
            1_722_575_316,
        )
        .unwrap();
        let bytes = hex::decode(data.trim_start_matches("0x")).unwrap();

        // selector + 5 head words + length word + one padded 32-byte memo chunk
        assert_eq!(bytes.len(), 4 + 32 * 5 + 32 + 32);
        assert_eq!(&bytes[0..4], &[0x44, 0xbc, 0x93, 0x7b]);
        // Vault address is right-aligned in its word.
        assert_eq!(hex::encode(&bytes[4 + 12..4 + 32]), vault.trim_start_matches("0x"));
        // The dynamic string's head slot holds the tail offset, 5 * 32 = 160.
        assert_eq!(bytes[4 + 32 * 3 + 31], 160);
        // Tail begins with the memo length.
        let len_word_start = 4 + 32 * 5;
        assert_eq!(bytes[len_word_start + 31] as usize, memo.len());
        assert_eq!(&bytes[len_word_start + 32..len_word_start + 32 + memo.len()], memo.as_bytes());
    }

    #[test]
    fn deposit_calldata_pads_a_memo_that_lands_on_a_word_boundary() {
        // Exactly 32 bytes: no padding should be added, and the length must still be present.
        let memo = "a".repeat(32);
        let data = encode_deposit_with_expiry(
            "0xed23d1bf7ac2ce8b4c09a090e252ea6ee5145e6e",
            "0x0000000000000000000000000000000000000000",
            1,
            &memo,
            0,
        )
        .unwrap();
        let bytes = hex::decode(data.trim_start_matches("0x")).unwrap();
        assert_eq!(bytes.len(), 4 + 32 * 5 + 32 + 32);
    }

    #[test]
    fn deposit_calldata_rejects_a_malformed_address() {
        assert!(encode_deposit_with_expiry("0xnope", "0x0", 1, "m", 0).is_err());
        assert!(encode_deposit_with_expiry(
            "0xed23d1bf7ac2ce8b4c09a090e252ea6ee5145e6e",
            "0x1234",
            1,
            "m",
            0
        )
        .is_err());
    }
}

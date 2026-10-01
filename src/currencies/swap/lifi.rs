//! LI.FI: same-chain DEX aggregation for Ethereum and the L2s this wallet supports.
//!
//! Chosen over 0x and 1inch because both of those now require an API key, and shipping a key
//! in an open-source wallet means shipping a key that gets extracted and rate-limited into
//! uselessness within a week. LI.FI answers unauthenticated, and its response carries
//! everything needed to execute without a second round trip: the router call, the minimum
//! output, and the address to approve.
//!
//! Restricted here to **same-chain** routes. LI.FI will happily quote a bridge, but a bridged
//! swap is a multi-transaction flow with its own failure and refund semantics, and this wallet
//! does not have the machinery to track one. Cross-chain goes through THORChain instead, where
//! the whole thing is a single outbound payment.

use serde_json::Value;

use crate::configuration::block_error;
use crate::configuration::http;
use crate::currencies::eth_chain::{self, EthNetwork};
use crate::currencies::swap::{
    Custody, SwapAsset, SwapExecution, SwapProvider, SwapQuote, SwapRequest,
};

const API: &str = "https://li.quest/v1/quote";

/// LI.FI's spelling of "the chain's native asset".
const NATIVE_SENTINEL: &str = "0x0000000000000000000000000000000000000000";

pub struct LiFi;

impl LiFi {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LiFi {
    fn default() -> Self {
        Self::new()
    }
}

/// The chains LI.FI is asked about, keyed by the wallet's own network selection.
///
/// Sepolia is deliberately absent: aggregators have no meaningful testnet liquidity, and a
/// quote against a testnet route would be noise rather than a rehearsal.
pub fn chain_id_for(network: EthNetwork) -> Option<u64> {
    match network {
        EthNetwork::Sepolia => None,
        other => Some(eth_chain::chain_id(other)),
    }
}

fn token_param(asset: &SwapAsset) -> String {
    if asset.native {
        NATIVE_SENTINEL.to_string()
    } else {
        asset.address.clone()
    }
}

/// Pull the fields this wallet acts on out of a LI.FI quote.
pub fn parse_quote(json: &Value) -> Result<ParsedLiFi, block_error::Error> {
    if let Some(message) = json.get("message").and_then(Value::as_str) {
        if json.get("transactionRequest").is_none() {
            return Err(block_error::Error::new(format!("LI.FI declined: {message}")));
        }
    }

    let estimate = json
        .get("estimate")
        .ok_or_else(|| block_error::Error::new("LI.FI quote has no estimate".to_string()))?;
    let tx = json
        .get("transactionRequest")
        .ok_or_else(|| block_error::Error::new("LI.FI quote has no transaction to send".to_string()))?;

    let str_at = |parent: &Value, key: &str| {
        parent.get(key).and_then(Value::as_str).map(str::to_string)
    };

    let to_amount = str_at(estimate, "toAmount")
        .and_then(|v| v.parse::<u128>().ok())
        .ok_or_else(|| block_error::Error::new("LI.FI quote has no output amount".to_string()))?;
    let to_amount_min = str_at(estimate, "toAmountMin")
        .and_then(|v| v.parse::<u128>().ok())
        .ok_or_else(|| block_error::Error::new("LI.FI quote has no minimum output".to_string()))?;

    Ok(ParsedLiFi {
        to_amount,
        to_amount_min,
        approval_address: str_at(estimate, "approvalAddress"),
        execution_duration: estimate
            .get("executionDuration")
            .and_then(Value::as_u64),
        to: str_at(tx, "to")
            .ok_or_else(|| block_error::Error::new("LI.FI quote has no call target".to_string()))?,
        data: str_at(tx, "data")
            .ok_or_else(|| block_error::Error::new("LI.FI quote has no calldata".to_string()))?,
        value: str_at(tx, "value").unwrap_or_else(|| "0x0".to_string()),
        gas_limit: str_at(tx, "gasLimit")
            .and_then(|g| parse_maybe_hex(&g))
            .unwrap_or(350_000),
        chain_id: tx.get("chainId").and_then(Value::as_u64).unwrap_or(0),
        tool: str_at(json, "tool"),
    })
}

fn parse_maybe_hex(value: &str) -> Option<u64> {
    let text = value.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse::<u64>().ok()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedLiFi {
    pub to_amount: u128,
    pub to_amount_min: u128,
    pub approval_address: Option<String>,
    pub execution_duration: Option<u64>,
    pub to: String,
    pub data: String,
    pub value: String,
    pub gas_limit: u64,
    pub chain_id: u64,
    pub tool: Option<String>,
}

impl SwapProvider for LiFi {
    fn id(&self) -> &'static str {
        "lifi"
    }

    fn display_name(&self) -> &'static str {
        "LI.FI"
    }

    fn custody(&self) -> Custody {
        Custody::AtomicOnChain
    }

    fn supports(&self, from: &SwapAsset, to: &SwapAsset) -> bool {
        // Same EVM chain, and not the same asset on both sides.
        from.chain == "eth"
            && to.chain == "eth"
            && !(from.native && to.native)
            && !from.address.eq_ignore_ascii_case(&to.address)
    }

    fn quote(&self, request: &SwapRequest) -> Result<SwapQuote, block_error::Error> {
        // The aggregator is asked about whichever network the wallet is pointed at, so a user
        // on Base gets Base liquidity rather than mainnet's. A network with no aggregator
        // liquidity worth quoting arrives as `None` and is declined rather than silently
        // answered with mainnet prices.
        let chain_id = request.evm_chain_id.ok_or_else(|| {
            block_error::Error::new(
                "no aggregator liquidity on the selected Ethereum network".to_string(),
            )
        })?;

        // LI.FI only honours a fee for a registered integrator, so an unregistered build that
        // sent one would have its quotes rejected and lose the venue entirely. The parameter
        // is therefore only added when a payout address is configured, and even then it is
        // LI.FI that decides whether to apply it. `fee` is a fraction here, not bps.
        let integrator = request.fee.lifi_integrator.trim();
        let fee_bps = request.fee.bps_for(integrator);
        let fee_param = if fee_bps > 0 {
            format!("&integrator={integrator}&fee={}", fee_bps as f64 / 10_000.0)
        } else {
            String::new()
        };
        let url = format!(
            "{API}?fromChain={chain_id}&toChain={chain_id}&fromToken={}&toToken={}\
             &fromAddress={}&toAddress={}&fromAmount={}&slippage={}{fee_param}",
            token_param(&request.from),
            token_param(&request.to),
            request.from_address.trim(),
            request.destination.trim(),
            request.amount_in_base,
            // LI.FI takes slippage as a fraction, not basis points.
            request.slippage_bps as f64 / 10_000.0,
        );

        let text = http::get_text(&url)?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|e| block_error::Error::new(format!("invalid LI.FI response: {e}")))?;
        let parsed = parse_quote(&json)?;

        if parsed.chain_id != 0 && parsed.chain_id != chain_id {
            return Err(block_error::Error::new(
                "LI.FI returned a transaction for a different chain than requested".to_string(),
            ));
        }

        Ok(SwapQuote {
            provider_id: self.id(),
            provider_name: self.display_name(),
            custody: self.custody(),
            from: request.from.clone(),
            to: request.to.clone(),
            amount_in_base: request.amount_in_base,
            expected_out_base: parsed.to_amount,
            min_out_base: parsed.to_amount_min,
            destination: request.destination.clone(),
            // LI.FI states no expiry, so the wallet's own quote-age limit applies.
            expiry: None,
            eta_seconds: parsed.execution_duration,
            route_note: parsed.tool.map(|t| format!("routed via {t}")),
            // LI.FI reports no fee total, so the wallet fee is all that can be stated.
            fee_total_base: None,
            min_in_base: None,
            fee_bps,
            execution: SwapExecution::EvmCall {
                chain_id,
                to: parsed.to,
                data: parsed.data,
                value: parsed.value,
                gas_limit: parsed.gas_limit,
                approval_spender: if request.from.native {
                    None
                } else {
                    // Approval must go to the contract being called. `safety::check_quote`
                    // enforces that, so a provider disagreeing with itself is caught rather
                    // than papered over here.
                    parsed.approval_address
                },
                approval_amount: if request.from.native {
                    None
                } else {
                    Some(request.amount_in_base.to_string())
                },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape captured from a real unauthenticated LI.FI response for USDC to ETH on mainnet.
    const CAPTURED: &str = r#"{
      "type":"lifi","id":"abc","tool":"sushiswap",
      "action":{"fromChainId":1,"toChainId":1},
      "estimate":{
        "toAmount":"407740323797385",
        "toAmountMin":"405701622178398",
        "approvalAddress":"0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE",
        "executionDuration":30
      },
      "transactionRequest":{
        "value":"0x0",
        "to":"0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE",
        "data":"0x2c57e8844636f518",
        "gasPrice":"0x6e433bb",
        "gasLimit":"0xaa32e",
        "chainId":1,
        "from":"0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
      }
    }"#;

    #[test]
    fn captured_response_parses_every_field_we_rely_on() {
        let parsed = parse_quote(&serde_json::from_str(CAPTURED).unwrap()).unwrap();
        assert_eq!(parsed.to_amount, 407_740_323_797_385);
        assert_eq!(parsed.to_amount_min, 405_701_622_178_398);
        assert_eq!(
            parsed.approval_address.as_deref(),
            Some("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE")
        );
        assert_eq!(parsed.to, "0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE");
        assert_eq!(parsed.value, "0x0");
        // 0xaa32e is 697134 decimal; hex gas limits must not be read as decimal.
        assert_eq!(parsed.gas_limit, 697_134);
        assert_eq!(parsed.chain_id, 1);
        assert_eq!(parsed.tool.as_deref(), Some("sushiswap"));
    }

    #[test]
    fn a_response_without_a_transaction_is_an_error_not_a_quote() {
        let body = r#"{"message":"No available quotes for the requested transfer"}"#;
        let err = parse_quote(&serde_json::from_str(body).unwrap()).unwrap_err();
        assert!(format!("{err}").contains("No available quotes"));
    }

    #[test]
    fn a_quote_missing_its_minimum_output_is_refused() {
        // Without toAmountMin there is no floor, and the swap would execute at any price.
        let body = r#"{"estimate":{"toAmount":"100"},"transactionRequest":{"to":"0x1","data":"0x2"}}"#;
        let err = parse_quote(&serde_json::from_str(body).unwrap()).unwrap_err();
        assert!(format!("{err}").contains("minimum output"));
    }

    #[test]
    fn hex_and_decimal_gas_limits_are_both_understood() {
        assert_eq!(parse_maybe_hex("0xaa32e"), Some(697_134));
        assert_eq!(parse_maybe_hex("697134"), Some(697_134));
        assert_eq!(parse_maybe_hex("nonsense"), None);
    }

    #[test]
    fn only_same_chain_evm_pairs_are_offered() {
        let lifi = LiFi::new();
        let eth = SwapAsset { chain: "eth".into(), symbol: "ETH".into(), address: String::new(), decimals: 18, native: true };
        let usdc = SwapAsset {
            chain: "eth".into(),
            symbol: "USDC".into(),
            address: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".into(),
            decimals: 6,
            native: false,
        };
        let btc = SwapAsset { chain: "btc".into(), symbol: "BTC".into(), address: String::new(), decimals: 8, native: true };

        assert!(lifi.supports(&eth, &usdc));
        assert!(lifi.supports(&usdc, &eth));
        // Cross-chain is THORChain's job, not this provider's.
        assert!(!lifi.supports(&btc, &eth));
        assert!(!lifi.supports(&eth, &btc));
        // Same asset both sides is not a swap.
        assert!(!lifi.supports(&eth, &eth));
        assert!(!lifi.supports(&usdc, &usdc));
    }

    #[test]
    fn testnets_are_not_offered_a_route() {
        assert!(chain_id_for(EthNetwork::Sepolia).is_none());
        assert_eq!(chain_id_for(EthNetwork::Mainnet), Some(1));
        assert_eq!(chain_id_for(EthNetwork::Base), Some(8453));
        assert_eq!(chain_id_for(EthNetwork::ArbitrumOne), Some(42161));
    }
}

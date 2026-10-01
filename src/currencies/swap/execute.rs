//! Turning an approved quote into signed, broadcast transactions.
//!
//! Everything here runs *after* [`super::safety::check_quote`] has passed, and re-runs that
//! check first regardless. A quote may have sat in the UI for a while, and the value of the
//! validation is that it happens immediately before signing rather than at some earlier point
//! the user has since navigated away from.
//!
//! Execution deliberately reuses each chain's ordinary send path. A THORChain swap on Bitcoin
//! is a normal payment to the vault with an `OP_RETURN` attached, so it inherits the coin
//! selection, the fee ceiling from `currencies::fees`, and the change handling that path
//! already has, rather than acquiring a parallel implementation that could drift away from it.

use crate::configuration::block_error;
use crate::currencies::swap::{solana_tx, SwapExecution, SwapQuote, SwapRequest};

/// Everything needed to sign, gathered from the wallet before execution starts.
///
/// Passed in rather than reached for, so the secrets live for exactly as long as this call and
/// the caller decides where they come from.
pub struct SigningContext {
    /// Key for the Bitcoin account, which BDK re-derives from.
    pub btc_key: Option<crate::currencies::btc_chain::BtcKey>,
    /// WIF for the Litecoin account.
    pub ltc_private_key: Option<String>,
    /// Hex key for the Ethereum account.
    pub eth_private_key: Option<String>,
    /// Base58 key for the Solana account.
    pub sol_private_key: Option<String>,
    /// The Solana account the swap was built for.
    pub sol_address: String,
    pub btc_node: String,
    pub btc_network: String,
    pub ltc_node: String,
    pub ltc_network: String,
    pub eth_node: String,
    pub eth_network: String,
    pub infura_key: String,
    pub sol_node: String,
    pub sol_network: String,
}

/// What happened, for display.
#[derive(Clone, Debug, PartialEq)]
pub struct SwapReceipt {
    /// Approval transaction, when one was needed.
    pub approval_txid: Option<String>,
    /// The swap itself.
    pub txid: String,
    /// Whether settlement is still pending on the other chain.
    pub pending_cross_chain: bool,
}

/// Long enough for a congested mainnet block or two; past this the user is better told than
/// left watching "Signing and broadcasting…".
const APPROVAL_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

fn missing_key(chain: &str) -> block_error::Error {
    block_error::Error::new(format!("{chain} account is locked or has no key in memory"))
}

/// Execute an approved quote.
///
/// Re-validates before doing anything irreversible.
pub fn execute(
    quote: &SwapQuote,
    request: &SwapRequest,
    context: &SigningContext,
) -> Result<SwapReceipt, block_error::Error> {
    // Deliberately re-run rather than trusting that the caller checked. Expiry in particular
    // is time-dependent, so a quote that was valid when the review card appeared may not be
    // valid now.
    super::safety::check_quote(quote, request)?;

    match &quote.execution {
        SwapExecution::UtxoWithMemo { vault, amount_base, memo, gas_rate } => {
            execute_utxo(quote, request, context, vault, *amount_base, memo, *gas_rate)
        }
        SwapExecution::EvmCall {
            chain_id,
            to,
            data,
            value,
            gas_limit,
            approval_spender,
            approval_amount,
        } => execute_evm(
            quote,
            request,
            context,
            *chain_id,
            to,
            data,
            value,
            *gas_limit,
            approval_spender.as_deref(),
            approval_amount.as_deref(),
        ),
        SwapExecution::SolanaTx { transaction_b64 } => {
            execute_solana(context, transaction_b64)
        }
    }
}

fn execute_utxo(
    quote: &SwapQuote,
    request: &SwapRequest,
    context: &SigningContext,
    vault: &str,
    amount_base: u64,
    memo: &str,
    gas_rate: Option<f32>,
) -> Result<SwapReceipt, block_error::Error> {
    match quote.from.chain.as_str() {
        "btc" => {
            use crate::currencies::btc_chain;
            let key = context
                .btc_key
                .as_ref()
                .ok_or_else(|| missing_key("Bitcoin"))?;
            // The provider's recommended rate still passes through the wallet's own fee
            // ceiling rather than being taken at face value.
            let tiers = btc_chain::fetch_fee_tiers(&context.btc_node, &context.btc_network);
            let rate = gas_rate
                .map(|r| crate::currencies::fees::clamp_fee_rate(r, tiers.medium))
                .unwrap_or_else(|| btc_chain::fee_rate_from_tier(&tiers, "medium"));

            let plan = btc_chain::prepare_send_with_memo(
                key,
                &context.btc_network,
                &context.btc_node,
                vault,
                amount_base,
                rate,
                Some(memo),
            )?;
            let txid = btc_chain::sign_and_broadcast(
                key,
                &context.btc_network,
                &context.btc_node,
                &plan,
            )?;
            Ok(SwapReceipt { approval_txid: None, txid, pending_cross_chain: true })
        }
        "ltc" => {
            use crate::currencies::ltc_chain;
            let key = context
                .ltc_private_key
                .as_deref()
                .ok_or_else(|| missing_key("Litecoin"))?;
            let amount_text = ltc_chain::format_ltc(amount_base);
            let plan = ltc_chain::prepare_send_with_memo(
                &request.from_address,
                vault,
                &amount_text,
                &context.ltc_node,
                &context.ltc_network,
                "medium",
                Some(memo),
            )?;
            let txid = ltc_chain::sign_and_broadcast(
                key,
                &plan,
                &context.ltc_node,
                &context.ltc_network,
            )?;
            Ok(SwapReceipt { approval_txid: None, txid, pending_cross_chain: true })
        }
        other => Err(block_error::Error::new(format!(
            "{other} cannot make a vault payment"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_evm(
    quote: &SwapQuote,
    request: &SwapRequest,
    context: &SigningContext,
    chain_id: u64,
    to: &str,
    data: &str,
    value: &str,
    gas_limit: u64,
    approval_spender: Option<&str>,
    approval_amount: Option<&str>,
) -> Result<SwapReceipt, block_error::Error> {
    use alloy::primitives::{Address, U256};
    use std::str::FromStr;

    use crate::currencies::eth_chain;

    let key = context
        .eth_private_key
        .as_deref()
        .ok_or_else(|| missing_key("Ethereum"))?;

    let mut approval_txid = None;

    // An ERC-20 input needs an allowance before the router can pull it. Granted for exactly
    // the swap amount, and only after checking what is already there, so a repeat swap does
    // not pay gas for an approval it does not need.
    if let (Some(spender), Some(amount)) = (approval_spender, approval_amount) {
        let needed = U256::from_str(amount).map_err(|_| {
            block_error::Error::new("could not read the approval amount".to_string())
        })?;
        let current = eth_chain::fetch_allowance(
            &quote.from.address,
            &request.from_address,
            spender,
            &context.eth_node,
            &context.eth_network,
            &context.infura_key,
        )
        .unwrap_or(U256::ZERO);

        if current < needed {
            let spender_address = Address::from_str(spender.trim()).map_err(|_| {
                block_error::Error::new("invalid approval target".to_string())
            })?;
            let calldata = eth_chain::encode_approve(spender_address, needed);
            let txid = eth_chain::send_contract_call(
                key,
                &quote.from.address,
                &format!("0x{}", hex::encode(calldata)),
                U256::ZERO,
                // A bare ERC-20 approve is well under this; the headroom covers tokens with
                // unusual approve implementations.
                100_000,
                chain_id,
                &context.eth_node,
                &context.eth_network,
                &context.infura_key,
            )?;
            // The swap is only sent once the allowance is actually on chain. Sending it
            // straight away would race the approval for the same nonce, and a swap mined
            // after a reverted approval would itself revert and burn its gas.
            eth_chain::wait_for_success(
                &txid,
                &context.eth_node,
                &context.eth_network,
                &context.infura_key,
                APPROVAL_CONFIRM_TIMEOUT,
            )
            .map_err(|why| {
                block_error::Error::new(format!(
                    "the approval {txid} did not confirm ({why}), so the swap was not sent. \
                     Once it confirms, find a new rate and swap again"
                ))
            })?;
            approval_txid = Some(txid);
        }
    }

    let call_value = if let Some(hex) = value.trim().strip_prefix("0x").or_else(|| value.trim().strip_prefix("0X")) {
        U256::from_str_radix(hex, 16)
            .map_err(|_| block_error::Error::new("could not read the transaction value".to_string()))?
    } else {
        U256::from_str(value.trim())
            .map_err(|_| block_error::Error::new("could not read the transaction value".to_string()))?
    };

    let txid = eth_chain::send_contract_call(
        key,
        to,
        data,
        call_value,
        gas_limit,
        chain_id,
        &context.eth_node,
        &context.eth_network,
        &context.infura_key,
    )?;

    Ok(SwapReceipt {
        approval_txid,
        txid,
        // A same-chain swap settles in the transaction itself; a THORChain router deposit
        // does not, and is distinguished by the provider that produced it.
        pending_cross_chain: quote.provider_id == "thorchain",
    })
}

fn execute_solana(
    context: &SigningContext,
    transaction_b64: &str,
) -> Result<SwapReceipt, block_error::Error> {
    use crate::currencies::sol::signing_key_from_base58;
    use crate::currencies::sol_chain;

    let key = context
        .sol_private_key
        .as_deref()
        .ok_or_else(|| missing_key("Solana"))?;
    let signing_key = signing_key_from_base58(key)?;
    let expected = sol_chain::validate_address(&context.sol_address)?;

    let raw = solana_tx::base64::decode(transaction_b64).ok_or_else(|| {
        block_error::Error::new("provider returned an unreadable transaction".to_string())
    })?;
    let signed = solana_tx::sign_provider_transaction(&raw, &signing_key, &expected)?;

    let txid = sol_chain::broadcast_signed(&signed, &context.sol_node, &context.sol_network)?;
    Ok(SwapReceipt { approval_txid: None, txid, pending_cross_chain: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currencies::swap::{Custody, SwapAsset};

    fn context() -> SigningContext {
        SigningContext {
            btc_key: None,
            ltc_private_key: None,
            eth_private_key: None,
            sol_private_key: None,
            sol_address: String::new(),
            btc_node: String::new(),
            btc_network: "bitcoin".into(),
            ltc_node: String::new(),
            ltc_network: "litecoin".into(),
            eth_node: String::new(),
            eth_network: "mainnet".into(),
            infura_key: String::new(),
            sol_node: String::new(),
            sol_network: "mainnet".into(),
        }
    }

    fn btc() -> SwapAsset {
        SwapAsset { chain: "btc".into(), symbol: "BTC".into(), address: String::new(), decimals: 8, native: true }
    }
    fn eth() -> SwapAsset {
        SwapAsset { chain: "eth".into(), symbol: "ETH".into(), address: String::new(), decimals: 18, native: true }
    }

    const MY_ETH: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";

    fn quote_and_request() -> (SwapQuote, SwapRequest) {
        let request = SwapRequest {
            from: btc(),
            to: eth(),
            amount_in_base: 10_000_000,
            from_address: "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu".into(),
            destination: MY_ETH.into(),
            slippage_bps: 100,
            evm_chain_id: Some(1),
            thornode_url: String::new(),
            sol_node: String::new(),
            sol_network: String::new(),
            fee: crate::currencies::swap::FeePayout::default(),
        };
        let quote = SwapQuote {
            provider_id: "thorchain",
            provider_name: "THORChain",
            custody: Custody::ProtocolVault,
            from: btc(),
            to: eth(),
            amount_in_base: 10_000_000,
            expected_out_base: 2_000_000_000_000_000_000,
            min_out_base: 1_990_000_000_000_000_000,
            destination: MY_ETH.into(),
            expiry: Some(u64::MAX),
            eta_seconds: Some(1800),
            route_note: None,
            fee_total_base: None,
            min_in_base: Some(100_000),
            fee_bps: 0,
            execution: SwapExecution::UtxoWithMemo {
                vault: "bc1qp6yzmq5kjr8yvyw7453gxvq4z3tvkdyadqm794".into(),
                amount_base: 10_000_000,
                memo: format!("=:ETH.ETH:{MY_ETH}:0/1/0"),
                gas_rate: Some(3.0),
            },
        };
        (quote, request)
    }

    #[test]
    fn execution_refuses_a_quote_that_fails_validation_before_touching_the_network() {
        let (mut quote, request) = quote_and_request();
        // Redirect the proceeds. Execution must refuse without any key present and without a
        // network call, proving the check runs first.
        quote.destination = "0x00000000219ab540356cBB839Cbe05303d7705Fa".into();
        let err = execute(&quote, &request, &context()).unwrap_err();
        assert!(format!("{err}").contains("not addressed to your wallet"), "got {err:?}");
    }

    #[test]
    fn execution_refuses_an_expired_quote_even_if_it_was_valid_when_shown() {
        let (mut quote, request) = quote_and_request();
        quote.expiry = Some(1);
        let err = execute(&quote, &request, &context()).unwrap_err();
        assert!(format!("{err}").contains("expired"), "got {err:?}");
    }

    #[test]
    fn a_locked_wallet_cannot_execute() {
        let (quote, request) = quote_and_request();
        // Valid quote, but no key in the context.
        let err = execute(&quote, &request, &context()).unwrap_err();
        assert!(format!("{err}").contains("locked"), "got {err:?}");
    }
}

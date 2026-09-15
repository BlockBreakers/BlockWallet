use serde_json::Value;
use std::collections::HashMap;

use crate::configuration::block_error;

pub fn gecko_id(symbol: &str) -> Option<&'static str> {
    match symbol.to_ascii_uppercase().as_str() {
        "BTC" => Some("bitcoin"),
        "ETH" => Some("ethereum"),
        "SOL" => Some("solana"),
        "LTC" => Some("litecoin"),
        "XMR" => Some("monero"),
        "USDC" => Some("usd-coin"),
        "USDT" => Some("tether"),
        "DAI" => Some("dai"),
        "WBTC" => Some("wrapped-bitcoin"),
        _ => None,
    }
}

pub fn format_fiat(amount: f64, fiat: &str) -> String {
    match fiat.to_ascii_lowercase().as_str() {
        "eur" => format!("€{amount:.2}"),
        _ => format!("${amount:.2}"),
    }
}

pub fn fetch_prices(symbols: &[&str], fiat: &str) -> Result<HashMap<String, f64>, block_error::Error> {
    let fiat = match fiat.to_ascii_lowercase().as_str() {
        "eur" => "eur",
        _ => "usd",
    };
    let mut ids = Vec::new();
    let mut reverse = HashMap::<String, String>::new();
    for symbol in symbols {
        if let Some(id) = gecko_id(symbol) {
            ids.push(id.to_string());
            reverse.insert(id.to_string(), symbol.to_ascii_uppercase());
        }
    }
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    ids.sort();
    ids.dedup();
    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies={fiat}",
        ids.join(",")
    );
    let text = crate::configuration::http::get_text(&url)?;
    let json: Value = serde_json::from_str(&text)?;
    let mut out = HashMap::new();
    if let Some(map) = json.as_object() {
        for (id, body) in map {
            if let Some(symbol) = reverse.get(id) {
                if let Some(price) = body.get(fiat).and_then(|v| v.as_f64()) {
                    out.insert(symbol.clone(), price);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_symbols() {
        assert_eq!(gecko_id("btc"), Some("bitcoin"));
        assert_eq!(gecko_id("USDC"), Some("usd-coin"));
        assert!(gecko_id("XYZ").is_none());
        assert_eq!(format_fiat(12.5, "usd"), "$12.50");
        assert_eq!(format_fiat(12.5, "eur"), "€12.50");
    }
}

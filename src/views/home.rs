use adw::prelude::*;
use glib::{clone, ControlFlow};
use gtk::{Align, Orientation};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::currencies::eth_chain;
use crate::currencies::tokens::Token;
use crate::views::currency::currency_view;
use crate::views::nav::{self, Nav};
use crate::views::ui;
use crate::ApplicationSettings;

/// What the balance worker hands the UI thread for one asset row.
#[derive(Clone)]
pub struct RowItem {
    pub token: Token,
    /// Amount plus unit, e.g. "0.00042 BTC" — or a status word like "Syncing…".
    pub amount: String,
    /// Fiat conversion, when prices are enabled and a quote was available.
    pub fiat: Option<String>,
}

pub fn home_view(app_settings: Arc<Mutex<ApplicationSettings>>) -> (gtk::Box, Nav, Arc<Mutex<ApplicationSettings>>) {
    let list = ui::page_body(14);

    // ---- hero: portfolio total, or a plain prompt when prices are off ----
    let hero = ui::vbox(2);
    hero.add_css_class("hero-card");
    let hero_caption = gtk::Label::builder()
        .label("Total balance")
        .halign(Align::Start)
        .css_classes(["hero-label"])
        .build();
    let hero_total = gtk::Label::builder()
        .label("—")
        .halign(Align::Start)
        .ellipsize(pango::EllipsizeMode::End)
        .css_classes(["balance-hero"])
        .build();
    let hero_sub = gtk::Label::builder()
        .label("Across all chains")
        .halign(Align::Start)
        .wrap(true)
        .css_classes(["balance-hero-sub"])
        .build();
    hero.append(&hero_caption);
    hero.append(&hero_total);
    hero.append(&hero_sub);

    let banner = ui::notice("Balances come straight from the nodes you configured.");
    let rows = ui::vbox(2);

    list.append(&hero);
    list.append(&banner);
    list.append(&rows);

    let scroll = ui::scroller(&list);
    let nav = Nav::new(&scroll);
    let page = nav.clone().wrap();

    let (sender, receiver) = crate::configuration::ui_channel::unbounded();
    let app = app_settings.clone();
    thread::spawn(move || {
        let mut price_cache: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let mut price_ticks = 0u32;
        loop {
            let snapshot = app.lock().unwrap().clone();
            let units = snapshot.btc_units.clone();
            let show_prices = snapshot.show_prices;
            let fiat = snapshot.fiat.clone();
            let mut items: Vec<RowItem> = Vec::new();
            let mut offline = false;
            let mut syncing = false;

            if let Some(token) = snapshot.tokens.eth_tokens.get("btc:BTC").cloned() {
                let mut display = snapshot
                    .btc_wallets
                    .first()
                    .map(|w| w.balance.lock().unwrap().clone())
                    .unwrap_or_else(|| "0 BTC".into());
                if nav::label_is_offline(&display) {
                    offline = true;
                }
                if nav::label_is_pending_sync(&display) {
                    syncing = true;
                    display = "Syncing…".into();
                } else {
                    display = nav::format_btc_units(&display, &units);
                }
                items.push(RowItem { token, amount: display, fiat: None });
            }
            let eth_native = snapshot
                .tokens
                .eth_tokens
                .values()
                .find(|t| t.chain == "eth" && eth_chain::is_native_token(t))
                .cloned();
            if let Some(token) = eth_native {
                let fallback = format!("0 {}", token.symbol);
                let mut display = snapshot
                    .eth_wallets
                    .first()
                    .map(|w| w.balance.lock().unwrap().clone())
                    .unwrap_or(fallback);
                if nav::label_is_offline(&display) {
                    offline = true;
                }
                if nav::label_is_pending_sync(&display) {
                    syncing = true;
                    display = "Syncing…".into();
                }
                items.push(RowItem { token, amount: display, fiat: None });
            }
            if let Some(token) = snapshot.tokens.eth_tokens.get("sol:SOL").cloned() {
                let mut display = snapshot
                    .sol_wallets
                    .first()
                    .map(|w| w.balance.lock().unwrap().clone())
                    .unwrap_or_else(|| "0 SOL".into());
                if nav::label_is_offline(&display) {
                    offline = true;
                }
                if nav::label_is_pending_sync(&display) {
                    syncing = true;
                    display = "Syncing…".into();
                }
                items.push(RowItem { token, amount: display, fiat: None });
            }
            if let Some(token) = snapshot.tokens.eth_tokens.get("ltc:LTC").cloned() {
                let mut display = snapshot
                    .ltc_wallets
                    .first()
                    .map(|w| w.balance.lock().unwrap().clone())
                    .unwrap_or_else(|| "0 LTC".into());
                if nav::label_is_offline(&display) {
                    offline = true;
                }
                if nav::label_is_pending_sync(&display) {
                    syncing = true;
                    display = "Syncing…".into();
                }
                items.push(RowItem { token, amount: display, fiat: None });
            }
            if let Some(token) = snapshot.tokens.eth_tokens.get("xmr:XMR").cloned() {
                let mut display = snapshot
                    .xmr_wallets
                    .first()
                    .map(|w| w.balance.lock().unwrap().clone())
                    .unwrap_or_else(|| "0 XMR".into());
                if nav::label_is_offline(&display) {
                    offline = true;
                }
                if nav::label_is_pending_sync(&display) {
                    syncing = true;
                    // A Monero sync can run for minutes and reports how far it has got, so its
                    // own "Syncing… 42%" is kept rather than flattened to the bare word.
                    display = nav::sync_label(&display);
                }
                items.push(RowItem { token, amount: display, fiat: None });
            }

            // Fiat is optional and off by default, so the total is only meaningful when
            // every row has a quote. `total` stays None otherwise rather than showing a
            // number that silently omits chains.
            let mut total: Option<f64> = None;
            if show_prices && !fiat.is_empty() {
                if price_ticks == 0 {
                    if let Ok(prices) = crate::currencies::prices::fetch_prices(&["BTC", "ETH", "SOL", "LTC", "XMR"], &fiat) {
                        price_cache = prices;
                    }
                }
                price_ticks = (price_ticks + 1) % 15;
                let mut running = 0.0;
                let mut priced_all = !items.is_empty();
                for item in items.iter_mut() {
                    match price_cache.get(&item.token.symbol) {
                        Some(price) => {
                            let qty = nav::parse_leading_amount(&item.amount);
                            let value = qty * price;
                            running += value;
                            item.fiat = Some(crate::currencies::prices::format_fiat(value, &fiat));
                        }
                        None => priced_all = false,
                    }
                }
                if priced_all {
                    total = Some(running);
                }
            }

            let total_display = total.map(|value| crate::currencies::prices::format_fiat(value, &fiat));
            if sender
                .send_blocking((items, offline, syncing, total_display, snapshot))
                .is_err()
            {
                break;
            }
            thread::sleep(Duration::from_secs(4));
        }
    });

    crate::configuration::ui_channel::attach(
        receiver,
        clone!(
            #[weak] rows,
            #[weak] banner,
            #[weak] hero_total,
            #[weak] hero_sub,
            #[strong] nav,
            #[upgrade_or]
            ControlFlow::Break,
            move |(items, offline, syncing, total, snapshot): (Vec<RowItem>, bool, bool, Option<String>, ApplicationSettings)| {
                while let Some(child) = rows.first_child() {
                    rows.remove(&child);
                }

                // Once anything is held, show only what is held. Until then, show all four
                // chains.
                //
                // A wallet with nothing in it should still look like a wallet with four
                // chains in it, so a new user can see what they have and tap through to
                // receive. The moment a real balance lands, the empty chains are just noise
                // sitting above the thing they actually own. The rule flips itself, so there
                // is nothing to configure and no state to get stuck in.
                //
                // "Held" and "empty" are both narrower than they look: a syncing row and an
                // unreachable node both report zero without meaning it, and neither is
                // treated as empty. See `nav::is_confirmed_zero`.
                let any_held = items.iter().any(|item| nav::has_balance(&item.amount));
                let shown: Vec<RowItem> = if any_held {
                    items
                        .iter()
                        .filter(|item| !nav::is_confirmed_zero(&item.amount))
                        .cloned()
                        .collect()
                } else {
                    items.clone()
                };

                match (&total, syncing) {
                    (Some(value), _) => {
                        hero_total.set_label(value);
                        hero_sub.set_label(&asset_count_label(shown.len()));
                    }
                    (None, true) => {
                        hero_total.set_label("Syncing…");
                        hero_sub.set_label("Reading balances from your nodes");
                    }
                    (None, false) => {
                        // No fiat total available. Show the chain count instead of a
                        // fabricated figure, and say why in the subtitle.
                        hero_total.set_label(&if shown.len() == 1 {
                            "1 asset".to_string()
                        } else {
                            format!("{} assets", shown.len())
                        });
                        hero_sub.set_label(if snapshot.show_prices {
                            "Fiat total unavailable right now"
                        } else {
                            "Turn on fiat prices in Settings for a total"
                        });
                    }
                }

                let offline_text = "A node is unreachable. Receiving still works offline.";
                banner.set_label(if offline {
                    offline_text
                } else if syncing {
                    "Syncing balances from your nodes…"
                } else if any_held {
                    "Showing the assets you hold. Open Assets to see them all."
                } else {
                    "Balances come straight from the nodes you configured."
                });
                ui::set_notice_warning(&banner, offline);

                let group = ui::group("Assets");
                for item in shown {
                    let row = currency_row(&item);
                    let gesture = gtk::GestureClick::new();
                    let nav = nav.clone();
                    let token_c = item.token.clone();
                    let settings = snapshot.clone();
                    gesture.connect_released(move |gesture, _, _, _| {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        nav.push("detail", &currency_view(token_c.clone(), settings.clone(), Some(nav.clone())));
                    });
                    row.add_controller(gesture);
                    group.add(&row);
                }
                rows.append(&group);
                ControlFlow::Continue
            }
        ),
    );

    (page, nav, app_settings)
}

/// Subtitle for the hero total.
///
/// The count used to be a constant four, so "1 assets across 4 chains" was unreachable and
/// the grammar never showed. Filtering Home down to what is held makes every count from one
/// to four possible, so the singular has to exist.
fn asset_count_label(count: usize) -> String {
    match count {
        1 => "1 asset across 4 chains".to_string(),
        n => format!("{n} assets across 4 chains"),
    }
}

/// One tappable asset row: coin mark, name + chain tag, amount + fiat.
pub fn currency_row(item: &RowItem) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    row.add_css_class("currency-row");

    row.append(&ui::coin_mark(
        &item.token.logo,
        &item.token.symbol,
        &item.token.chain,
        40,
    ));

    let name_box = gtk::Box::new(Orientation::Vertical, 1);
    name_box.set_hexpand(true);
    name_box.set_valign(Align::Center);
    name_box.append(
        &gtk::Label::builder()
            .label(&item.token.name)
            .halign(Align::Start)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["currency-name"])
            .build(),
    );

    // Symbol plus, where it disambiguates, a chain tag. USDC exists as both an ERC-20 and
    // an SPL token, and without the tag those two rows are indistinguishable.
    let ticker_box = gtk::Box::new(Orientation::Horizontal, 6);
    ticker_box.append(
        &gtk::Label::builder()
            .label(&item.token.symbol)
            .halign(Align::Start)
            .css_classes(["currency-ticker"])
            .build(),
    );
    if ui::needs_chain_tag(&item.token.name, &item.token.chain) {
        ticker_box.append(
            &gtk::Label::builder()
                .label(ui::chain_display_name(&item.token.chain))
                .valign(Align::Center)
                .css_classes(["chain-tag"])
                .build(),
        );
    }
    name_box.append(&ticker_box);

    let value_box = gtk::Box::new(Orientation::Vertical, 1);
    value_box.set_valign(Align::Center);
    value_box.append(
        &gtk::Label::builder()
            .label(&item.amount)
            .halign(Align::End)
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(16)
            .css_classes(["currency-price"])
            .build(),
    );
    if let Some(fiat) = &item.fiat {
        value_box.append(
            &gtk::Label::builder()
                .label(fiat)
                .halign(Align::End)
                .css_classes(["currency-price-sub"])
                .build(),
        );
    }

    row.append(&name_box);
    row.append(&value_box);
    row
}

/// Back-compat shim for callers that only have a token and a rendered amount string.
pub fn generate_currency_box_static(token: &Token, display: &str) -> gtk::Box {
    currency_row(&RowItem {
        token: token.clone(),
        amount: display.to_string(),
        fiat: None,
    })
}

pub fn generate_currency_box(element: (Token, Arc<Mutex<String>>)) -> gtk::Box {
    let row = generate_currency_box_static(&element.0, &element.1.lock().unwrap());
    let (sender, receiver) = crate::configuration::ui_channel::unbounded();
    thread::spawn(move || {
        loop {
            let out_string = element.1.lock().unwrap().clone();
            if sender.send_blocking(out_string).is_err() {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
    crate::configuration::ui_channel::attach(
        receiver,
        clone!(
            #[weak] row,
            #[upgrade_or]
            ControlFlow::Break,
            move |price_text: String| {
                if let Some(label) = row.last_child().and_then(|w| w.first_child()) {
                    if let Ok(label) = label.downcast::<gtk::Label>() {
                        if price_text != "Uninitialized" {
                            label.set_label(&price_text);
                        }
                    }
                }
                ControlFlow::Continue
            }
        ),
    );
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Home rule, expressed the same way the render callback expresses it, so the
    /// behaviour can be checked without a display.
    fn shown_labels(labels: &[&str]) -> Vec<String> {
        let any_held = labels.iter().any(|l| nav::has_balance(l));
        labels
            .iter()
            .filter(|l| !any_held || !nav::is_confirmed_zero(l))
            .map(|l| l.to_string())
            .collect()
    }

    #[test]
    fn an_empty_wallet_still_shows_all_five_chains() {
        // Nothing held anywhere. Hiding here would leave a new user with a blank screen and
        // no way to tap through and receive.
        let shown = shown_labels(&["0 BTC", "0 ETH", "0 SOL", "0 LTC", "0.0 XMR"]);
        assert_eq!(shown.len(), 5);
    }

    #[test]
    fn once_anything_is_held_the_empty_chains_drop_away() {
        let shown = shown_labels(&["0.5 BTC", "0 ETH", "0 SOL", "0 LTC", "0.0 XMR"]);
        assert_eq!(shown, vec!["0.5 BTC"]);
    }

    #[test]
    fn several_holdings_all_survive() {
        let shown = shown_labels(&["0.5 BTC", "0 ETH", "12 SOL", "0 LTC"]);
        assert_eq!(shown, vec!["0.5 BTC", "12 SOL"]);
    }

    #[test]
    fn a_syncing_chain_is_never_dropped() {
        // Mid-sync, an unread balance reads as zero. Dropping it would make rows appear and
        // disappear as each chain reports in.
        let shown = shown_labels(&["0.5 BTC", "Syncing…", "0 SOL", "0 LTC"]);
        assert_eq!(shown, vec!["0.5 BTC", "Syncing…"]);
    }

    #[test]
    fn an_unreachable_chain_is_never_dropped() {
        // The important one: an offline node reports zero without meaning it, and hiding on
        // that would remove an asset the user holds at the moment they cannot check it.
        let shown = shown_labels(&["0.5 BTC", "0 ETH (offline)", "0 SOL", "0 LTC"]);
        assert_eq!(shown, vec!["0.5 BTC", "0 ETH (offline)"]);
    }

    #[test]
    fn a_holding_that_cannot_be_refreshed_counts_as_held() {
        // A carried-over balance from the last good sync is still a real holding, so it both
        // survives the filter and is enough to switch the screen out of show-everything mode.
        let shown = shown_labels(&["0 BTC", "2.5 ETH (offline)", "0 SOL", "0 LTC"]);
        assert_eq!(shown, vec!["2.5 ETH (offline)"]);
    }

    #[test]
    fn everything_syncing_shows_everything() {
        let shown = shown_labels(&["Syncing…", "Syncing…", "Syncing…", "Syncing…"]);
        assert_eq!(shown.len(), 4);
    }

    #[test]
    fn the_asset_count_subtitle_is_grammatical_at_one() {
        assert_eq!(asset_count_label(1), "1 asset across 4 chains");
        assert_eq!(asset_count_label(3), "3 assets across 4 chains");
    }
}

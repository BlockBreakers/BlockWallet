use adw::prelude::*;
use glib::{clone, ControlFlow};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::currencies::eth_chain;
use crate::currencies::tokens::Token;
use crate::views::currency::currency_view;
use crate::views::home::{currency_row, generate_currency_box_static, RowItem};
use crate::views::nav::{self, Nav};
use crate::views::ui;
use crate::ApplicationSettings;

pub fn asset_view(
    app_settings: Arc<Mutex<ApplicationSettings>>,
) -> (gtk::Box, Nav, Arc<Mutex<ApplicationSettings>>) {
    let list = ui::page_body(12);
    let banner = ui::notice("Tap an asset to send or receive. Zero balances stay listed so you can still receive.");
    let empty = ui::empty_state(
        "No assets yet",
        "Unlock a wallet to see your assets. Receiving works even offline.",
        "view-grid-symbolic",
    );
    empty.set_visible(false);
    let rows = ui::vbox(2);
    list.append(&banner);
    list.append(&empty);
    list.append(&rows);

    let scroll = ui::scroller(&list);
    let nav = Nav::new(&scroll);
    let page = nav.clone().wrap();

    let (sender, receiver) = crate::configuration::ui_channel::unbounded();
    let app = app_settings.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(4));
            let snapshot = app.lock().unwrap().clone();
            let units = snapshot.btc_units.clone();
            let mut items: Vec<(Token, String)> = Vec::new();
            let mut offline = false;

            if let Some(token) = snapshot.tokens.eth_tokens.get("btc:BTC").cloned() {
                let mut display = snapshot
                    .btc_wallets
                    .iter()
                    .map(|w| nav::parse_leading_amount(&w.balance.lock().unwrap()))
                    .sum::<f64>()
                    .to_string();
                if snapshot.btc_wallets.iter().any(|w| nav::label_is_offline(&w.balance.lock().unwrap())) {
                    offline = true;
                    display = format!("{display} BTC (offline)");
                } else if snapshot
                    .btc_wallets
                    .iter()
                    .all(|w| nav::label_is_pending_sync(&w.balance.lock().unwrap()))
                    && !snapshot.btc_wallets.is_empty()
                {
                    display = "Syncing…".into();
                } else {
                    display = nav::format_btc_units(&format!("{display} BTC"), &units);
                }
                items.push((token, display));
            }
            let eth_native = snapshot
                .tokens
                .eth_tokens
                .values()
                .find(|t| t.chain == "eth" && eth_chain::is_native_token(t))
                .cloned();
            if let Some(token) = eth_native {
                let symbol = token.symbol.clone();
                let mut display = snapshot
                    .eth_wallets
                    .iter()
                    .map(|w| nav::parse_leading_amount(&w.balance.lock().unwrap()))
                    .sum::<f64>()
                    .to_string();
                if snapshot.eth_wallets.iter().any(|w| nav::label_is_offline(&w.balance.lock().unwrap())) {
                    offline = true;
                    display = format!("{display} {symbol} (offline)");
                } else if snapshot
                    .eth_wallets
                    .iter()
                    .all(|w| nav::label_is_pending_sync(&w.balance.lock().unwrap()))
                    && !snapshot.eth_wallets.is_empty()
                {
                    display = "Syncing…".into();
                } else {
                    display = format!("{display} {symbol}");
                }
                items.push((token, display));
            }
            for wallet in &snapshot.eth_wallets {
                let erc20 = wallet.erc20_balances.lock().unwrap().clone();
                for (symbol, amount) in erc20 {
                    if amount <= 0.0 {
                        continue;
                    }
                    if let Some(token) = snapshot.tokens.eth_tokens.get(&format!("eth:{symbol}")).cloned() {
                        items.push((token, format!("{amount} {symbol}")));
                    }
                }
            }
            if let Some(token) = snapshot.tokens.eth_tokens.get("sol:SOL").cloned() {
                let mut display = snapshot
                    .sol_wallets
                    .iter()
                    .map(|w| nav::parse_leading_amount(&w.balance.lock().unwrap()))
                    .sum::<f64>()
                    .to_string();
                if snapshot.sol_wallets.iter().any(|w| nav::label_is_offline(&w.balance.lock().unwrap())) {
                    offline = true;
                    display = format!("{display} SOL (offline)");
                } else if snapshot
                    .sol_wallets
                    .iter()
                    .all(|w| nav::label_is_pending_sync(&w.balance.lock().unwrap()))
                    && !snapshot.sol_wallets.is_empty()
                {
                    display = "Syncing…".into();
                } else {
                    display = format!("{display} SOL");
                }
                items.push((token, display));
            }
            for wallet in &snapshot.sol_wallets {
                let spl = wallet.spl_balances.lock().unwrap().clone();
                for (symbol, amount) in spl {
                    if amount <= 0.0 {
                        continue;
                    }
                    if let Some(token) = snapshot.tokens.eth_tokens.get(&format!("sol:{symbol}")).cloned() {
                        items.push((token, format!("{amount} {symbol}")));
                    }
                }
            }
            if let Some(token) = snapshot.tokens.eth_tokens.get("ltc:LTC").cloned() {
                let mut display = snapshot
                    .ltc_wallets
                    .iter()
                    .map(|w| nav::parse_leading_amount(&w.balance.lock().unwrap()))
                    .sum::<f64>()
                    .to_string();
                if snapshot.ltc_wallets.iter().any(|w| nav::label_is_offline(&w.balance.lock().unwrap())) {
                    offline = true;
                    display = format!("{display} LTC (offline)");
                } else if snapshot
                    .ltc_wallets
                    .iter()
                    .all(|w| nav::label_is_pending_sync(&w.balance.lock().unwrap()))
                    && !snapshot.ltc_wallets.is_empty()
                {
                    display = "Syncing…".into();
                } else {
                    display = format!("{display} LTC");
                }
                items.push((token, display));
            }
            if let Some(token) = snapshot.tokens.eth_tokens.get("xmr:XMR").cloned() {
                let mut display = snapshot
                    .xmr_wallets
                    .iter()
                    .map(|w| nav::parse_leading_amount(&w.balance.lock().unwrap()))
                    .sum::<f64>()
                    .to_string();
                if snapshot.xmr_wallets.iter().any(|w| nav::label_is_offline(&w.balance.lock().unwrap())) {
                    offline = true;
                    display = format!("{display} XMR (offline)");
                } else if snapshot
                    .xmr_wallets
                    .iter()
                    .all(|w| nav::label_is_pending_sync(&w.balance.lock().unwrap()))
                    && !snapshot.xmr_wallets.is_empty()
                {
                    display = snapshot
                        .xmr_wallets
                        .first()
                        .map(|w| nav::sync_label(&w.balance.lock().unwrap()))
                        .unwrap_or_else(|| "Syncing…".into());
                } else {
                    display = format!("{display} XMR");
                }
                items.push((token, display));
            }
            if sender.send_blocking((items, offline, snapshot)).is_err() {
                break;
            }
        }
    });

    // A session-only override for the setting, so hidden assets are always one tap away.
    //
    // Receiving is only reachable by opening an asset, so hiding a zero-balance row also hides
    // the only route to its receive address. The footer button below flips this for the
    // current visit rather than changing the saved preference, which keeps the setting honest
    // and the receive path reachable at the same time.
    let reveal_all = Rc::new(Cell::new(false));

    crate::configuration::ui_channel::attach(
        receiver,
        clone!(
            #[weak] rows,
            #[weak] empty,
            #[weak] banner,
            #[strong] nav,
            #[strong] reveal_all,
            #[upgrade_or]
            ControlFlow::Break,
            move |(items, offline, snapshot): (Vec<(Token, String)>, bool, ApplicationSettings)| {
                while let Some(child) = rows.first_child() {
                    rows.remove(&child);
                }

                let hiding = snapshot.hide_zero_balances && !reveal_all.get();
                let shown: Vec<(Token, String)> = if hiding {
                    items
                        .iter()
                        .filter(|(_, display)| !nav::is_confirmed_zero(display))
                        .cloned()
                        .collect()
                } else {
                    items.clone()
                };
                let hidden_count = items.len() - shown.len();

                empty.set_visible(shown.is_empty() && hidden_count == 0);
                banner.set_label(if offline {
                    "A node is unreachable. You can still open an asset to receive."
                } else if snapshot.hide_zero_balances {
                    "Tap an asset to send or receive. Empty assets are hidden."
                } else {
                    "Tap an asset to send or receive. Zero balances stay listed so you can still receive."
                });
                ui::set_notice_warning(&banner, offline);

                // One boxed-list group per chain. A flat list of 5 chains' tokens reads as
                // an undifferentiated pile once ERC-20s and SPL tokens are in it, and the
                // chain is the thing that decides where a send actually goes.
                for chain in ["btc", "eth", "sol", "ltc", "xmr"] {
                    let chain_items: Vec<&(Token, String)> =
                        shown.iter().filter(|(token, _)| token.chain == chain).collect();
                    if chain_items.is_empty() {
                        continue;
                    }
                    let group = ui::group(ui::chain_display_name(chain));
                    for (token, display) in chain_items {
                        let row = currency_row(&RowItem {
                            token: token.clone(),
                            amount: display.clone(),
                            fiat: None,
                        });
                        let gesture = gtk::GestureClick::new();
                        let nav = nav.clone();
                        let token_c = token.clone();
                        let settings = snapshot.clone();
                        gesture.connect_released(move |gesture, _, _, _| {
                            gesture.set_state(gtk::EventSequenceState::Claimed);
                            nav.push(
                                "detail",
                                &currency_view(token_c.clone(), settings.clone(), Some(nav.clone())),
                            );
                        });
                        row.add_controller(gesture);
                        group.add(&row);
                    }
                    rows.append(&group);
                }

                // Everything is empty and the filter has swallowed the whole list. Say so
                // rather than presenting a blank screen that looks like a failure to load.
                if shown.is_empty() && hidden_count > 0 {
                    rows.append(&ui::empty_state(
                        "Nothing with a balance yet",
                        "Every asset is empty, so they are all hidden. Show them to receive.",
                        "view-grid-symbolic",
                    ));
                }

                if hidden_count > 0 {
                    let label = if hidden_count == 1 {
                        "Show 1 empty asset".to_string()
                    } else {
                        format!("Show {hidden_count} empty assets")
                    };
                    let show = gtk::Button::builder()
                        .label(label)
                        .halign(gtk::Align::Center)
                        .css_classes(["flat"])
                        .build();
                    let reveal = Rc::clone(&reveal_all);
                    show.connect_clicked(move |button| {
                        reveal.set(true);
                        // The list rebuilds on the next poll; disable in the meantime so a
                        // second press cannot queue a second rebuild.
                        button.set_sensitive(false);
                    });
                    rows.append(&show);
                } else if reveal_all.get() && snapshot.hide_zero_balances {
                    let hide = gtk::Button::builder()
                        .label("Hide empty assets again")
                        .halign(gtk::Align::Center)
                        .css_classes(["flat"])
                        .build();
                    let reveal = Rc::clone(&reveal_all);
                    hide.connect_clicked(move |button| {
                        reveal.set(false);
                        button.set_sensitive(false);
                    });
                    rows.append(&hide);
                }
                ControlFlow::Continue
            }
        ),
    );

    (page, nav, app_settings)
}

pub fn generate_currency_box(balance: f64, token: Token) -> gtk::Box {
    generate_currency_box_static(&token, &balance.to_string())
}

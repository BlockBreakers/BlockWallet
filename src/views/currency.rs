use adw::prelude::*;
use gtk::{Orientation, Align};
use pango::WrapMode;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::thread;
use glib::{clone, ControlFlow};

use crate::configuration::clipboard;
use crate::currencies::btc::BitcoinWallet;
use crate::currencies::btc_chain;
use crate::currencies::tokens::Token;
use crate::currencies::eth::EthereumWallet;
use crate::currencies::eth_chain;
use crate::currencies::ltc::LitecoinWallet;
use crate::currencies::ltc_chain;
use crate::currencies::sol::SolanaWallet;
use crate::currencies::xmr::MoneroWallet;
use crate::currencies::xmr_chain;
use crate::ApplicationSettings;
use crate::views::nav::{self, Nav};
use crate::views::transactions;
use crate::views::ui;

pub fn currency_view(token: Token, app_settings: ApplicationSettings, nav: Option<Nav>) -> gtk::Box {
    let currency_box = gtk::Box::new(Orientation::Vertical, 0);
    let display = live_balance_label(&token, &app_settings);
    let offline = nav::label_is_offline(&display);

    // ---- balance header: coin mark, name, and the balance at hero size ----
    let header = ui::vbox(6);
    header.add_css_class("hero-card");

    let identity = gtk::Box::new(Orientation::Horizontal, 12);
    identity.append(&ui::coin_mark(&token.logo, &token.symbol, &token.chain, 44));
    let identity_text = ui::vbox(1);
    identity_text.set_valign(Align::Center);
    identity_text.set_hexpand(true);
    identity_text.append(
        &gtk::Label::builder()
            .label(&token.name)
            .halign(Align::Start)
            .css_classes(["currency-name"])
            .build(),
    );
    let tag_row = gtk::Box::new(Orientation::Horizontal, 6);
    tag_row.append(
        &gtk::Label::builder()
            .label(&token.symbol)
            .halign(Align::Start)
            .css_classes(["currency-ticker"])
            .build(),
    );
    if ui::needs_chain_tag(&token.name, &token.chain) {
        tag_row.append(
            &gtk::Label::builder()
                .label(ui::chain_display_name(&token.chain))
                .valign(Align::Center)
                .css_classes(["chain-tag"])
                .build(),
        );
    }
    identity_text.append(&tag_row);
    identity.append(&identity_text);

    let balance_label = gtk::Label::builder()
        .label(&display)
        .halign(Align::Start)
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .css_classes(["balance-hero"])
        .build();

    header.append(&identity);
    header.append(&balance_label);

    let banner = ui::notice(if offline {
        "Node unreachable. You can still receive at the address below."
    } else {
        "Sending needs a reachable node. Receiving works offline."
    });
    ui::set_notice_warning(&banner, offline);

    let transactions_box = get_transactions(token.clone(), app_settings.clone());
    let receive_box = match token.chain.as_str() {
        "btc" => generate_btc_receive_box(&app_settings.btc_wallets),
        "sol" => generate_sol_receive_box(&app_settings.sol_wallets),
        "ltc" => generate_ltc_receive_box(&app_settings.ltc_wallets),
        "xmr" => generate_xmr_receive_box(&app_settings.xmr_wallets),
        _ => generate_eth_receive_box(&app_settings.eth_wallets),
    };

    // Send and Receive sit side by side as equal-weight actions rather than stacked full
    // width, which read as a form to work through top to bottom.
    let send_button = ui::icon_button("Send", "send-to-symbolic");
    send_button.add_css_class("suggested-action");
    let receive_button = ui::icon_button("Receive", "folder-download-symbolic");
    let action_row = gtk::Box::new(Orientation::Horizontal, 10);
    action_row.set_homogeneous(true);
    action_row.append(&send_button);
    action_row.append(&receive_button);

    let currency_detail_box = ui::page_body(14);
    currency_detail_box.append(&header);
    currency_detail_box.append(&action_row);
    currency_detail_box.append(&banner);
    currency_detail_box.append(&receive_box);
    currency_detail_box.append(&transactions_box);

    let scrollable_container = ui::scroller(&currency_detail_box);
    currency_box.append(&scrollable_container);

    send_button.connect_clicked(clone!(
        #[strong] token,
        #[strong] app_settings,
        #[strong] nav,
        move |_| {
            let send = transactions::transaction_view(app_settings.clone(), token.clone()).0;
            if let Some(nav) = &nav {
                nav.push("send", &send);
            } else {
                // Fallback when opened without a tab stack.
            }
        }
    ));

    receive_button.connect_clicked(move |button| {
        let show = !receive_box.is_visible();
        receive_box.set_visible(show);
        // The button is the only affordance that reveals the address panel, so it has to
        // say which way it will go next.
        if let Some(content) = button.child().and_then(|c| c.downcast::<adw::ButtonContent>().ok()) {
            content.set_label(if show { "Hide" } else { "Receive" });
        }
    });

    let balance_arc = balance_watch_arc(&token, &app_settings);
    let units = app_settings.btc_units.clone();
    let is_btc = token.chain == "btc";
    let (sender, receiver) = crate::configuration::ui_channel::unbounded();
    thread::spawn(move || {
        loop {
            let text = balance_arc.lock().unwrap().clone();
            if sender.send_blocking(text).is_err() {
                break;
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
    crate::configuration::ui_channel::attach(
        receiver,
        clone!(
            #[weak] banner,
            #[weak] balance_label,
            #[upgrade_or]
            ControlFlow::Break,
            move |text: String| {
                let offline = nav::label_is_offline(&text);
                banner.set_label(if offline {
                    "Node unreachable. You can still receive at the address below."
                } else {
                    "Sending needs a reachable node. Receiving works offline."
                });
                ui::set_notice_warning(&banner, offline);
                if nav::label_is_pending_sync(&text) {
                    balance_label.set_label(&nav::sync_label(&text));
                } else if is_btc {
                    balance_label.set_label(&nav::format_btc_units(&text, &units));
                } else {
                    balance_label.set_label(&text);
                }
                ControlFlow::Continue
            }
        ),
    );

    currency_box
}

fn live_balance_label(token: &Token, app_settings: &ApplicationSettings) -> String {
    match token.chain.as_str() {
        "btc" => {
            return app_settings
                .btc_wallets
                .first()
                .map(|w| nav::format_btc_units(&w.balance.lock().unwrap(), &app_settings.btc_units))
                .unwrap_or_else(|| "0 BTC".into());
        }
        "sol" => {
            if token.symbol == "SOL" {
                return app_settings
                    .sol_wallets
                    .first()
                    .map(|w| w.balance.lock().unwrap().clone())
                    .unwrap_or_else(|| "0 SOL".into());
            }
            return app_settings
                .sol_wallets
                .iter()
                .find_map(|w| w.spl_balances.lock().unwrap().get(&token.symbol).copied())
                .map(|amount| format!("{amount} {}", token.symbol))
                .unwrap_or_else(|| format!("0 {}", token.symbol));
        }
        "ltc" => {
            return app_settings
                .ltc_wallets
                .first()
                .map(|w| w.balance.lock().unwrap().clone())
                .unwrap_or_else(|| "0 LTC".into());
        }
        "xmr" => {
            return app_settings
                .xmr_wallets
                .first()
                .map(|w| w.balance.lock().unwrap().clone())
                .unwrap_or_else(|| "0 XMR".into());
        }
        _ => {}
    }
    if token.chain == "eth" && eth_chain::is_native_token(token) {
        return app_settings
            .eth_wallets
            .first()
            .map(|w| w.balance.lock().unwrap().clone())
            .unwrap_or_else(|| format!("0 {}", token.symbol));
    }
    app_settings
        .eth_wallets
        .iter()
        .find_map(|w| {
            w.erc20_balances
                .lock()
                .unwrap()
                .get(&token.symbol)
                .copied()
        })
        .map(|amount| format!("{amount} {}", token.symbol))
        .unwrap_or_else(|| format!("0 {}", token.symbol))
}

fn balance_watch_arc(token: &Token, app_settings: &ApplicationSettings) -> Arc<Mutex<String>> {
    if token.chain == "btc" {
        if let Some(w) = app_settings.btc_wallets.first() {
            return Arc::clone(&w.balance);
        }
    } else if token.chain == "sol" && token.symbol == "SOL" {
        if let Some(w) = app_settings.sol_wallets.first() {
            return Arc::clone(&w.balance);
        }
    } else if token.chain == "ltc" {
        if let Some(w) = app_settings.ltc_wallets.first() {
            return Arc::clone(&w.balance);
        }
    } else if token.chain == "xmr" {
        if let Some(w) = app_settings.xmr_wallets.first() {
            return Arc::clone(&w.balance);
        }
    } else if token.chain == "eth" && eth_chain::is_native_token(token) {
        if let Some(w) = app_settings.eth_wallets.first() {
            return Arc::clone(&w.balance);
        }
    }
    Arc::new(Mutex::new(live_balance_label(token, app_settings)))
}

/*
pub fn get_balances_by_wallet(token: Token, app_settings: ApplicationSettings) -> gtk::Box {
    let wallet_balances_box = gtk::Box::new(Orientation::Vertical, 0);
    return wallet_balances_box;
}
*/

pub fn get_transactions(token: Token, app_settings: ApplicationSettings) -> gtk::Box {
    let transactions_box = ui::vbox(0);

    // Collect first, render second: all four chains produce the same (incoming, amount,
    // confirmations, txid) shape once formatted, so one row builder covers them all.
    let mut entries: Vec<(bool, String, u32, String)> = Vec::new();

    if token.chain == "btc" {
        for btcw in &app_settings.btc_wallets {
            for item in btcw.history.lock().unwrap().iter() {
                let incoming = item.amount_sats >= 0;
                let amount = if incoming {
                    btc_chain::format_btc(item.amount_sats as u64)
                } else {
                    btc_chain::format_btc(item.amount_sats.unsigned_abs())
                };
                entries.push((incoming, format!("{amount} BTC"), item.confirmations, item.txid.clone()));
            }
        }
    } else if token.chain == "ltc" {
        for ltcw in &app_settings.ltc_wallets {
            for item in ltcw.history.lock().unwrap().iter() {
                let incoming = item.amount_sats >= 0;
                let amount = if incoming {
                    ltc_chain::format_ltc(item.amount_sats as u64)
                } else {
                    ltc_chain::format_ltc(item.amount_sats.unsigned_abs())
                };
                entries.push((incoming, format!("{amount} LTC"), item.confirmations, item.txid.clone()));
            }
        }
    } else if token.chain == "xmr" {
        for xmrw in &app_settings.xmr_wallets {
            for item in xmrw.history.lock().unwrap().iter() {
                let incoming = item.amount_piconero >= 0;
                let amount = xmr_chain::format_xmr(item.amount_piconero.unsigned_abs());
                entries.push((incoming, format!("{amount} XMR"), item.confirmations, item.txid.clone()));
            }
        }
    } else {
        let wallet_history: Vec<_> = if token.chain == "sol" {
            app_settings
                .sol_wallets
                .iter()
                .flat_map(|w| w.history.lock().unwrap().clone())
                .map(|item| (item.symbol, item.amount, item.incoming, item.confirmations, item.txid))
                .collect()
        } else {
            app_settings
                .eth_wallets
                .iter()
                .flat_map(|w| w.history.lock().unwrap().clone())
                .map(|item| (item.symbol, item.amount, item.incoming, item.confirmations, item.txid))
                .collect()
        };
        for (symbol, amount, incoming, confirmations, txid) in wallet_history {
            if symbol != token.symbol {
                continue;
            }
            entries.push((incoming, format!("{amount} {symbol}"), confirmations, txid));
        }
    }

    if entries.is_empty() {
        let empty = ui::empty_state(
            "No transactions yet",
            "Transactions for this asset will show up here once you send or receive.",
            "view-list-symbolic",
        );
        empty.set_vexpand(false);
        transactions_box.append(&empty);
        return transactions_box;
    }

    entries.sort_by_key(|(_, _, confirmations, _)| *confirmations);
    let group = ui::group("Transactions");
    for (incoming, amount, confirmations, txid) in entries {
        group.add(&transaction_row(incoming, &amount, confirmations, &txid));
    }
    transactions_box.append(&group);
    transactions_box
}

fn transaction_row(incoming: bool, amount: &str, confirmations: u32, txid: &str) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    row.add_css_class("tx-row");

    let icon = gtk::Image::from_icon_name(if incoming {
        "go-down-symbolic"
    } else {
        "go-up-symbolic"
    });
    icon.set_valign(Align::Center);
    icon.add_css_class(if incoming { "tx-icon-in" } else { "tx-icon-out" });

    let text_box = ui::vbox(1);
    text_box.set_hexpand(true);
    text_box.set_valign(Align::Center);
    text_box.append(
        &gtk::Label::builder()
            .label(if incoming { "Received" } else { "Sent" })
            .halign(Align::Start)
            .css_classes(["currency-name"])
            .build(),
    );
    let meta = if confirmations == 0 {
        format!("Pending · {}", &txid[..txid.len().min(12)])
    } else {
        format!("{confirmations} conf · {}", &txid[..txid.len().min(12)])
    };
    text_box.append(
        &gtk::Label::builder()
            .label(&meta)
            .halign(Align::Start)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["tx-meta"])
            .build(),
    );

    let sign = if incoming { "+" } else { "−" };
    let amount_label = gtk::Label::builder()
        .label(&format!("{sign}{amount}"))
        .halign(Align::End)
        .valign(Align::Center)
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(16)
        .css_classes([if incoming {
            "transaction-positive"
        } else {
            "transaction-negative"
        }])
        .build();

    row.append(&icon);
    row.append(&text_box);
    row.append(&amount_label);
    row
}

/// Shared receive panel. All four chains present the same thing — a QR, the address, and
/// a way to copy it — so they share one builder rather than four near-identical copies
/// that drift apart.
fn receive_panel(chain: &str, entries: Vec<(String, Option<gtk::gdk::Texture>)>) -> gtk::Box {
    let receive_box = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .visible(false)
        .css_classes(["receiver-box"])
        .build();

    receive_box.append(&ui::heading(&format!(
        "Your {} address",
        ui::chain_display_name(chain)
    )));
    receive_box.append(&ui::dim("Safe to share. This works even with the radios off."));

    for (address, qr) in entries {
        let entry_box = ui::vbox(10);

        if let Some(texture) = qr {
            // The QR sits on a deliberately white frame: the code is dark modules on a
            // light field, so on a dark theme background it would not scan.
            let image = gtk::Image::from_paintable(Some(&texture));
            image.set_pixel_size(180);
            let frame = gtk::Box::new(Orientation::Vertical, 0);
            frame.add_css_class("qr-frame");
            frame.set_halign(Align::Center);
            frame.append(&image);
            entry_box.append(&frame);
        }

        entry_box.append(&ui::mono_address(&address));

        let copy = ui::icon_button("Copy address", "edit-copy-symbolic");
        let to_copy = address.clone();
        copy.connect_clicked(move |_| {
            if !to_copy.is_empty() {
                clipboard::copy_text(&to_copy);
                // Previously copying gave no feedback at all, so there was no way to tell
                // whether the tap registered.
                ui::toast("Address copied. The clipboard clears shortly.");
            }
        });
        entry_box.append(&copy);

        receive_box.append(&entry_box);
    }

    receive_box
}

pub fn generate_btc_receive_box(btc_wallets: &[BitcoinWallet]) -> gtk::Box {
    receive_panel(
        "btc",
        btc_wallets
            .iter()
            .map(|w| (w.address.clone().unwrap_or_default(), w.generate_qr_address().ok()))
            .collect(),
    )
}

pub fn generate_eth_receive_box(eth_wallets: &Vec<EthereumWallet>) -> gtk::Box {
    receive_panel(
        "eth",
        eth_wallets
            .iter()
            .map(|w| (w.address.clone().unwrap_or_default(), w.generate_qr_address().ok()))
            .collect(),
    )
}

pub fn generate_sol_receive_box(sol_wallets: &Vec<SolanaWallet>) -> gtk::Box {
    receive_panel(
        "sol",
        sol_wallets
            .iter()
            .map(|w| (w.address.clone().unwrap_or_default(), w.generate_qr_address().ok()))
            .collect(),
    )
}

pub fn generate_ltc_receive_box(ltc_wallets: &Vec<LitecoinWallet>) -> gtk::Box {
    receive_panel(
        "ltc",
        ltc_wallets
            .iter()
            .map(|w| (w.address.clone().unwrap_or_default(), w.generate_qr_address().ok()))
            .collect(),
    )
}

pub fn generate_xmr_receive_box(xmr_wallets: &Vec<MoneroWallet>) -> gtk::Box {
    receive_panel(
        "xmr",
        xmr_wallets
            .iter()
            .map(|w| (w.address.clone().unwrap_or_default(), w.generate_qr_address().ok()))
            .collect(),
    )
}

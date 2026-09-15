use adw::prelude::*;
use glib::{clone, ControlFlow};
use gtk::{Align, Orientation};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::views::nav;
use crate::views::ui;
use crate::ApplicationSettings;

/// One history entry, already split into its display parts. The old view formatted
/// everything into a single run-on string ("+0.001 BTC  3 conf  a1b2c3d4e5f6"), which is
/// unreadable at a glance and unstyleable per-part.
#[derive(Clone)]
struct ActivityRow {
    incoming: bool,
    amount: String,
    chain: String,
    confirmations: u32,
    txid: String,
}

pub fn activity_view(app_settings: Arc<Mutex<ApplicationSettings>>) -> gtk::Box {
    let page = ui::page_body(12);

    let banner = ui::notice("Recent history from this device. Receiving still works if a node is unreachable.");
    let empty = ui::empty_state(
        "No activity yet",
        "Transactions you send and receive on this device will appear here.",
        "view-list-symbolic",
    );
    let list = ui::vbox(2);

    page.append(&banner);
    page.append(&empty);
    page.append(&list);

    let scroll = ui::scroller(&page);
    let wrapper = ui::vbox(0);
    wrapper.append(&scroll);

    let (sender, receiver) = crate::configuration::ui_channel::unbounded();
    let app = app_settings.clone();
    thread::spawn(move || {
        loop {
            let snapshot = app.lock().unwrap().clone();
            let mut rows: Vec<ActivityRow> = Vec::new();
            let mut offline = false;
            for wallet in &snapshot.btc_wallets {
                if nav::label_is_offline(&wallet.balance.lock().unwrap()) {
                    offline = true;
                }
                for item in wallet.history.lock().unwrap().iter() {
                    let incoming = item.amount_sats >= 0;
                    let amount = if incoming {
                        crate::currencies::btc_chain::format_btc(item.amount_sats as u64)
                    } else {
                        crate::currencies::btc_chain::format_btc(item.amount_sats.unsigned_abs())
                    };
                    rows.push(ActivityRow {
                        incoming,
                        amount: format!("{amount} BTC"),
                        chain: "btc".into(),
                        confirmations: item.confirmations,
                        txid: item.txid.clone(),
                    });
                }
            }
            for wallet in &snapshot.eth_wallets {
                if nav::label_is_offline(&wallet.balance.lock().unwrap()) {
                    offline = true;
                }
                for item in wallet.history.lock().unwrap().iter() {
                    rows.push(ActivityRow {
                        incoming: item.incoming,
                        amount: format!("{} {}", item.amount, item.symbol),
                        chain: "eth".into(),
                        confirmations: item.confirmations,
                        txid: item.txid.clone(),
                    });
                }
            }
            for wallet in &snapshot.sol_wallets {
                if nav::label_is_offline(&wallet.balance.lock().unwrap()) {
                    offline = true;
                }
                for item in wallet.history.lock().unwrap().iter() {
                    rows.push(ActivityRow {
                        incoming: item.incoming,
                        amount: format!("{} {}", item.amount, item.symbol),
                        chain: "sol".into(),
                        confirmations: item.confirmations,
                        txid: item.txid.clone(),
                    });
                }
            }
            for wallet in &snapshot.ltc_wallets {
                if nav::label_is_offline(&wallet.balance.lock().unwrap()) {
                    offline = true;
                }
                for item in wallet.history.lock().unwrap().iter() {
                    let incoming = item.amount_sats >= 0;
                    let amount = if incoming {
                        crate::currencies::ltc_chain::format_ltc(item.amount_sats as u64)
                    } else {
                        crate::currencies::ltc_chain::format_ltc(item.amount_sats.unsigned_abs())
                    };
                    rows.push(ActivityRow {
                        incoming,
                        amount: format!("{amount} LTC"),
                        chain: "ltc".into(),
                        confirmations: item.confirmations,
                        txid: item.txid.clone(),
                    });
                }
            }
            for wallet in &snapshot.xmr_wallets {
                if nav::label_is_offline(&wallet.balance.lock().unwrap()) {
                    offline = true;
                }
                for item in wallet.history.lock().unwrap().iter() {
                    let incoming = item.amount_piconero >= 0;
                    let amount = crate::currencies::xmr_chain::format_xmr(item.amount_piconero.unsigned_abs());
                    rows.push(ActivityRow {
                        incoming,
                        amount: format!("{amount} XMR"),
                        chain: "xmr".into(),
                        confirmations: item.confirmations,
                        txid: item.txid.clone(),
                    });
                }
            }
            // Fewest confirmations first, so the newest transactions lead. Confirmation
            // count is the only ordering signal the per-chain history items all carry.
            rows.sort_by_key(|row| row.confirmations);
            if sender.send_blocking((rows, offline)).is_err() {
                break;
            }
            thread::sleep(Duration::from_secs(5));
        }
    });

    crate::configuration::ui_channel::attach(
        receiver,
        clone!(
            #[weak] list,
            #[weak] empty,
            #[weak] banner,
            #[upgrade_or]
            ControlFlow::Break,
            move |(rows, offline): (Vec<ActivityRow>, bool)| {
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
                empty.set_visible(rows.is_empty());
                banner.set_label(if offline {
                    "A node is unreachable. Receiving still works offline."
                } else {
                    "Recent history from this device. Receiving still works if a node is unreachable."
                });
                ui::set_notice_warning(&banner, offline);

                if !rows.is_empty() {
                    let group = ui::group("Recent");
                    for row in rows {
                        group.add(&activity_row_widget(&row));
                    }
                    list.append(&group);
                }
                ControlFlow::Continue
            }
        ),
    );

    wrapper
}

fn activity_row_widget(item: &ActivityRow) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    row.add_css_class("tx-row");

    // Direction badge: a green down-arrow for received, neutral up-arrow for sent. Colour
    // and arrow both encode it, so it survives a colour-blind reader and a grey theme.
    let icon = gtk::Image::from_icon_name(if item.incoming {
        "go-down-symbolic"
    } else {
        "go-up-symbolic"
    });
    icon.set_valign(Align::Center);
    icon.add_css_class(if item.incoming { "tx-icon-in" } else { "tx-icon-out" });

    let text_box = gtk::Box::new(Orientation::Vertical, 1);
    text_box.set_hexpand(true);
    text_box.set_valign(Align::Center);
    text_box.append(
        &gtk::Label::builder()
            .label(if item.incoming { "Received" } else { "Sent" })
            .halign(Align::Start)
            .css_classes(["currency-name"])
            .build(),
    );

    let meta = if item.confirmations == 0 {
        format!("Pending · {}", short_txid(&item.txid))
    } else {
        format!("{} conf · {}", item.confirmations, short_txid(&item.txid))
    };
    text_box.append(
        &gtk::Label::builder()
            .label(&meta)
            .halign(Align::Start)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["tx-meta"])
            .build(),
    );

    let sign = if item.incoming { "+" } else { "−" };
    let amount = gtk::Label::builder()
        .label(&format!("{sign}{}", item.amount))
        .halign(Align::End)
        .valign(Align::Center)
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(16)
        .css_classes([if item.incoming {
            "transaction-positive"
        } else {
            "transaction-negative"
        }])
        .build();

    let chain_tag = gtk::Label::builder()
        .label(ui::chain_display_name(&item.chain))
        .valign(Align::Center)
        .css_classes(["chain-tag"])
        .build();

    let right = gtk::Box::new(Orientation::Vertical, 2);
    right.set_valign(Align::Center);
    right.append(&amount);
    let tag_wrap = gtk::Box::new(Orientation::Horizontal, 0);
    tag_wrap.set_halign(Align::End);
    tag_wrap.append(&chain_tag);
    right.append(&tag_wrap);

    row.append(&icon);
    row.append(&text_box);
    row.append(&right);
    row
}

fn short_txid(txid: &str) -> String {
    txid.chars().take(12).collect()
}

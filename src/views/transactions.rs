use adw::prelude::*;
use glib::{clone, ControlFlow};
use gtk::{Button, Orientation};
use pango::WrapMode;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::ApplicationSettings;
use crate::currencies::btc_chain;
use crate::currencies::eth_chain;
use crate::currencies::ltc_chain;
use crate::currencies::sol_chain;
use crate::currencies::tokens::Token;
use crate::currencies::xmr_chain;
use crate::views::ui;

pub fn transaction_view(app_settings: ApplicationSettings, token: Token) -> (gtk::Box, ApplicationSettings) {
    match token.chain.as_str() {
        "btc" => (btc_send_view(app_settings.clone()), app_settings),
        "sol" => (sol_send_view(app_settings.clone(), token), app_settings),
        "ltc" => (ltc_send_view(app_settings.clone()), app_settings),
        "xmr" => (xmr_send_view(app_settings.clone()), app_settings),
        _ => (eth_send_view(app_settings.clone(), token), app_settings),
    }
}

/// The parts every send screen shares. Built once here so the five chains cannot drift
/// apart visually, and so the confirmation gating is written in exactly one place.
struct SendChrome {
    page: gtk::Box,
    /// Boxed-list group holding the account / recipient / amount / fee rows.
    form: adw::PreferencesGroup,
    error: gtk::Label,
    review: Button,
    confirm_box: gtk::Box,
    summary: gtk::Label,
    confirm: Button,
    cancel: Button,
    /// The "I understand this spends real value" tick. Owned by the caller so it can be
    /// re-armed for every send rather than staying ticked for the life of the screen.
    ack: gtk::CheckButton,
    /// Whether this screen is on a real-value network, i.e. whether `ack` gates Confirm.
    spends_real_value: bool,
    /// Testnet vs real-value strip above the summary, filled in when a plan is ready.
    network_note: gtk::Label,
    status: gtk::Label,
}

/// `spends_real_value` drives the acknowledgement checkbox: when true the Confirm button
/// starts insensitive and only the checkbox can enable it.
fn send_chrome(chain: &str, spends_real_value: bool, ack_text: &str) -> SendChrome {
    let page = ui::page_body(14);

    page.append(&ui::heading(&format!("Send {}", ui::chain_display_name(chain))));

    let form = ui::group("Details");
    page.append(&form);

    let review = ui::primary_button("Review send");
    page.append(&review);

    let error = ui::error_label("");
    page.append(&error);

    // ---- review card ----
    let confirm_box = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .visible(false)
        .css_classes(["review-card"])
        .build();

    let network_note = gtk::Label::builder()
        .label("")
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .xalign(0.0)
        .build();

    let summary = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .selectable(true)
        .css_classes(["review-summary"])
        .build();

    let confirm = ui::primary_button("Confirm and broadcast");
    let cancel = ui::button("Cancel");

    let ack = gtk::CheckButton::with_label(ack_text);
    ack.set_visible(spends_real_value);
    if spends_real_value {
        confirm.set_sensitive(false);
    }
    let confirm_gate = confirm.clone();
    // `!spends_real_value ||` matters: on a testnet screen the tick is hidden and never
    // gates anything, so re-arming it between sends must not leave Confirm dead.
    ack.connect_toggled(move |cb| {
        confirm_gate.set_sensitive(!spends_real_value || cb.is_active());
    });

    confirm_box.append(&network_note);
    confirm_box.append(&ui::heading("Review"));
    confirm_box.append(&summary);
    confirm_box.append(&ack);
    confirm_box.append(&confirm);
    confirm_box.append(&cancel);
    page.append(&confirm_box);

    let status = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .xalign(0.0)
        .visible(false)
        .css_classes(["info-banner"])
        .build();
    page.append(&status);

    SendChrome {
        page,
        form,
        error,
        review,
        confirm_box,
        summary,
        confirm,
        cancel,
        ack,
        spends_real_value,
        network_note,
        status,
    }
}

/// Owns the review card's lifecycle.
///
/// Two rules it exists to enforce, both of which were previously unenforced:
///
/// 1. A summary the user has read must never outlive the inputs it was built from. Every
///    field on the form is watched, so editing the recipient after tapping Review tears the
///    card down instead of leaving Confirm wired to the old plan.
/// 2. Consent is per-send, not per-screen. `arm` unticks the acknowledgement each time a
///    plan is prepared, so a second send has to tick it again.
/// Widgets are held weakly on purpose. The Confirm button owns a click handler that captures
/// this gate, so a strong reference back to that same button would be a cycle: the handler
/// keeps the button alive, the button owns the handler, and neither is ever freed. Since
/// every send screen is rebuilt from scratch on each navigation, that would leak a whole page
/// of widgets per visit. The page itself keeps these alive for as long as they matter.
#[derive(Clone)]
struct ReviewGate {
    confirm_box: glib::WeakRef<gtk::Box>,
    ack: glib::WeakRef<gtk::CheckButton>,
    confirm: glib::WeakRef<Button>,
    spends_real_value: bool,
    /// Type-erased `*prepared.lock() = None`, so one gate serves all four chains despite
    /// each having its own `PreparedSend`.
    discard_plan: Rc<dyn Fn()>,
}

impl ReviewGate {
    fn new<T: 'static>(
        confirm_box: &gtk::Box,
        ack: &gtk::CheckButton,
        confirm: &Button,
        spends_real_value: bool,
        prepared: &Arc<Mutex<Option<T>>>,
    ) -> Self {
        let slot = prepared.clone();
        Self {
            confirm_box: confirm_box.downgrade(),
            ack: ack.downgrade(),
            confirm: confirm.downgrade(),
            spends_real_value,
            discard_plan: Rc::new(move || *slot.lock().unwrap() = None),
        }
    }

    /// Reset the gate to its just-built state. `set_active(false)` only emits `toggled` when
    /// the value actually changes, so sensitivity is set explicitly rather than relying on
    /// the handler firing.
    fn rearm_ack(&self) {
        if let Some(ack) = self.ack.upgrade() {
            ack.set_active(false);
        }
        if let Some(confirm) = self.confirm.upgrade() {
            confirm.set_sensitive(!self.spends_real_value);
        }
    }

    /// A fresh plan is ready: show the card with consent withdrawn.
    fn arm(&self) {
        self.rearm_ack();
        if let Some(confirm_box) = self.confirm_box.upgrade() {
            confirm_box.set_visible(true);
        }
    }

    /// Something the plan was derived from changed, so the plan no longer describes what the
    /// user can see. Drop it and hide the card.
    fn invalidate(&self) {
        (self.discard_plan)();
        self.rearm_ack();
        if let Some(confirm_box) = self.confirm_box.upgrade() {
            confirm_box.set_visible(false);
        }
    }

    fn watch_entry(&self, row: &adw::EntryRow) {
        let gate = self.clone();
        row.connect_changed(move |_| gate.invalidate());
    }

    fn watch_combo(&self, row: &adw::ComboRow) {
        let gate = self.clone();
        row.connect_selected_notify(move |_| gate.invalidate());
    }
}

/// Style the strip above the review summary: green for testnet, amber for real value.
fn set_network_note(label: &gtk::Label, testnet: bool, text: &str) {
    label.set_label(text);
    label.remove_css_class("testnet-note");
    label.remove_css_class("spend-warning");
    label.add_css_class(if testnet { "testnet-note" } else { "spend-warning" });
}

/// Wrap a send page so a long form still reaches its Confirm button on a 360×720 screen.
fn scrolled(page: gtk::Box) -> gtk::Box {
    let outer = ui::vbox(0);
    outer.append(&ui::scroller(&page));
    outer
}

fn btc_send_view(app_settings: ApplicationSettings) -> gtk::Box {
    let btc_mainnet = !app_settings.btc_network.eq_ignore_ascii_case("testnet");
    let chrome = send_chrome("btc", btc_mainnet, "I understand this spends real bitcoin.");
    let SendChrome {
        page: box_,
        form,
        error,
        review,
        confirm_box,
        summary,
        confirm,
        cancel,
        ack,
        spends_real_value,
        network_note,
        status,
    } = chrome;

    let names: Vec<String> = app_settings
        .btc_wallets
        .iter()
        .map(|wallet| {
            wallet
                .wallet_name
                .clone()
                .unwrap_or_else(|| "Bitcoin".to_string())
        })
        .collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let from_row = ui::combo_row("From account", &name_refs);
    let receive_address = ui::entry_row("Recipient");
    let amount = ui::entry_row("Amount (BTC)");
    let fee_row = ui::combo_row("Network fee", &["Low", "Medium", "High"]);
    fee_row.set_selected(1);

    form.add(&from_row);
    form.add(&receive_address);
    form.add(&amount);
    form.add(&fee_row);

    // The chrome exposes ComboRows, but the handlers below were written against
    // `DropDown::selected()`. `ComboRow::selected()` has the same meaning, so the aliases
    // keep the rest of this function unchanged.
    let from_wallet = from_row.clone();
    let fee = fee_row.clone();

    let app_settings = Arc::new(Mutex::new(app_settings));
    let prepared = Arc::new(Mutex::new(None::<btc_chain::PreparedSend>));

    let gate = ReviewGate::new(&confirm_box, &ack, &confirm, spends_real_value, &prepared);
    gate.watch_entry(&receive_address);
    gate.watch_entry(&amount);
    gate.watch_combo(&fee_row);
    gate.watch_combo(&from_row);

    review.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[weak] receive_address,
        #[weak] amount,
        #[weak] fee,
        #[weak] error,
        #[strong] gate,
        #[weak] summary,
        #[weak] from_wallet,
        move |_| {
            error.set_visible(false);
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.btc_wallets.get(index) else {
                error.set_label("Select a Bitcoin account.");
                error.set_visible(true);
                return;
            };
            let Some(mnemonic) = wallet.mnemonic.clone() else {
                error.set_label("This account has no recovery phrase in memory. Unlock again.");
                error.set_visible(true);
                return;
            };
            let passphrase = wallet.password.clone().unwrap_or_default();
            let to = receive_address.text().to_string();
            let network = crate::currencies::btc_chain::parse_network(&settings.btc_network);
            if let Err(err) = btc_chain::validate_address(&to, network) {
                error.set_label(&format!("{err:?}").replace("ERROR: ", "").replace('\"', ""));
                error.set_visible(true);
                return;
            }
            let amount_sats = match btc_chain::btc_to_sats(&amount.text()) {
                Ok(value) if value > 0 => value,
                Ok(_) => {
                    error.set_label("Amount must be greater than 0.");
                    error.set_visible(true);
                    return;
                }
                Err(_) => {
                    error.set_label("Amount must be a BTC number, for example 0.001.");
                    error.set_visible(true);
                    return;
                }
            };
            let labels = ["Low", "Medium", "High"];
            let fee_label = labels
                .get(fee.selected() as usize)
                .copied()
                .unwrap_or("Medium")
                .to_string();
            let node = settings.btc_node.clone();
            let network_name = settings.btc_network.clone();
            let testnet = network_name.eq_ignore_ascii_case("testnet");
            drop(settings);

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let tiers = btc_chain::fetch_fee_tiers(&node, &network_name);
                let fee_rate = btc_chain::fee_rate_from_tier(&tiers, &fee_label);
                let result = btc_chain::prepare_send(
                    &mnemonic,
                    &passphrase,
                    &network_name,
                    &node,
                    &to,
                    amount_sats,
                    fee_rate,
                );
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] error,
                    #[weak] summary,
                    #[weak] network_note,
                    #[strong] prepared,
                    #[strong] gate,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(plan) => {
                                set_network_note(
                                    &network_note,
                                    testnet,
                                    if testnet {
                                        "Bitcoin testnet. These coins have no mainnet value."
                                    } else {
                                        "Mainnet. This spends real bitcoin."
                                    },
                                );
                                summary.set_label(&plan.summary());
                                *prepared.lock().unwrap() = Some(plan);
                                gate.arm();
                            }
                            Err(_) => {
                                error.set_label("Could not build that transaction. Check the amount, address and balance, and whether the Bitcoin node is reachable. Receiving still works offline.");
                                error.set_visible(true);
                                gate.invalidate();
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    cancel.connect_clicked(clone!(
        #[strong] gate,
        move |_| gate.invalidate()
    ));

    confirm.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[weak] error,
        #[weak] status,
        #[strong] gate,
        #[weak] from_wallet,
        move |_| {
            let plan = match prepared.lock().unwrap().clone() {
                Some(plan) => plan,
                None => return,
            };
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.btc_wallets.get(index) else {
                return;
            };
            let Some(mnemonic) = wallet.mnemonic.clone() else {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let passphrase = wallet.password.clone().unwrap_or_default();
            let node = settings.btc_node.clone();
            let network_name = settings.btc_network.clone();
            drop(settings);

            error.set_visible(false);
            status.set_label("Broadcasting…");
            status.set_visible(true);
            // Clears the plan as well as hiding the card, so a second tap on Confirm while
            // the first broadcast is still in flight cannot send the same transaction twice.
            gate.invalidate();

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = btc_chain::sign_and_broadcast(
                    &mnemonic,
                    &passphrase,
                    &network_name,
                    &node,
                    &plan,
                );
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] status,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(txid) => {
                                status.set_label(&format!("Sent. Transaction ID: {txid}"));
                                ui::set_notice_warning(&status, false);
                                ui::toast("Transaction broadcast.");
                            }
                            Err(_) => {
                                status.set_label(
                                    "Broadcast failed. The node may be unreachable; your receive address is still valid offline.",
                                );
                                ui::set_notice_warning(&status, true);
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    scrolled(box_)
}

fn ltc_send_view(app_settings: ApplicationSettings) -> gtk::Box {
    let ltc_mainnet = !app_settings.ltc_network.eq_ignore_ascii_case("testnet");
    let SendChrome {
        page: box_,
        form,
        error,
        review,
        confirm_box,
        summary,
        confirm,
        cancel,
        ack,
        spends_real_value,
        network_note,
        status,
    } = send_chrome("ltc", ltc_mainnet, "I understand this spends real litecoin.");

    let names: Vec<String> = app_settings
        .ltc_wallets
        .iter()
        .map(|wallet| {
            wallet
                .wallet_name
                .clone()
                .unwrap_or_else(|| "Litecoin".to_string())
        })
        .collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let from_row = ui::combo_row("From account", &name_refs);
    let receive_address = ui::entry_row("Recipient");
    let amount = ui::entry_row("Amount (LTC)");
    let fee_row = ui::combo_row("Network fee", &["Low", "Medium", "High"]);
    fee_row.set_selected(1);

    form.add(&from_row);
    form.add(&receive_address);
    form.add(&amount);
    form.add(&fee_row);

    let from_wallet = from_row.clone();
    let fee = fee_row.clone();

    let app_settings = Arc::new(Mutex::new(app_settings));
    let prepared = Arc::new(Mutex::new(None::<ltc_chain::PreparedSend>));

    let gate = ReviewGate::new(&confirm_box, &ack, &confirm, spends_real_value, &prepared);
    gate.watch_entry(&receive_address);
    gate.watch_entry(&amount);
    gate.watch_combo(&fee_row);
    gate.watch_combo(&from_row);

    review.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[strong] gate,
        #[weak] receive_address,
        #[weak] amount,
        #[weak] fee,
        #[weak] error,
        #[weak] summary,
        #[weak] from_wallet,
        move |_| {
            error.set_visible(false);
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.ltc_wallets.get(index) else {
                error.set_label("Select a Litecoin account.");
                error.set_visible(true);
                return;
            };
            let Some(from) = wallet.address.clone() else {
                error.set_label("Wallet address is missing.");
                error.set_visible(true);
                return;
            };
            if wallet.private_key.is_none() {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let to = receive_address.text().to_string();
            let network = ltc_chain::parse_network(&settings.ltc_network);
            if let Err(err) = ltc_chain::validate_address(&to, network) {
                error.set_label(&format!("{err:?}").replace("ERROR: ", "").replace('\"', ""));
                error.set_visible(true);
                return;
            }
            let node = settings.ltc_node.clone();
            let network_name = settings.ltc_network.clone();
            let testnet = network_name.eq_ignore_ascii_case("testnet");
            drop(settings);

            let labels = ["Low", "Medium", "High"];
            let fee_label = labels
                .get(fee.selected() as usize)
                .copied()
                .unwrap_or("Medium")
                .to_string();
            let amount_text = amount.text().to_string();
            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = ltc_chain::prepare_send(&from, &to, &amount_text, &node, &network_name, &fee_label);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] error,
                    #[weak] summary,
                    #[weak] network_note,
                    #[strong] prepared,
                    #[strong] gate,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(plan) => {
                                set_network_note(
                                    &network_note,
                                    testnet,
                                    if testnet {
                                        "Litecoin testnet. These coins have no mainnet value."
                                    } else {
                                        "Mainnet. This spends real litecoin."
                                    },
                                );
                                summary.set_label(&plan.summary());
                                *prepared.lock().unwrap() = Some(plan);
                                gate.arm();
                            }
                            Err(_) => {
                                error.set_label("Could not build that transaction. Check the amount, address and balance, and whether the Litecoin node is reachable. Receiving still works offline.");
                                error.set_visible(true);
                                gate.invalidate();
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    cancel.connect_clicked(clone!(
        #[strong] gate,
        move |_| gate.invalidate()
    ));

    confirm.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[weak] error,
        #[weak] status,
        #[strong] gate,
        #[weak] from_wallet,
        move |_| {
            let plan = match prepared.lock().unwrap().clone() {
                Some(plan) => plan,
                None => return,
            };
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.ltc_wallets.get(index) else {
                return;
            };
            let Some(private_key) = wallet.private_key.clone() else {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let node = settings.ltc_node.clone();
            let network_name = settings.ltc_network.clone();
            drop(settings);

            error.set_visible(false);
            status.set_label("Broadcasting…");
            status.set_visible(true);
            gate.invalidate();

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = ltc_chain::sign_and_broadcast(&private_key, &plan, &node, &network_name);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] status,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(txid) => {
                                status.set_label(&format!("Sent. Transaction ID: {txid}"));
                                ui::set_notice_warning(&status, false);
                                ui::toast("Transaction broadcast.");
                            }
                            Err(_) => {
                                status.set_label(
                                    "Broadcast failed. The node may be unreachable; your receive address is still valid offline.",
                                );
                                ui::set_notice_warning(&status, true);
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    scrolled(box_)
}

/// The keys a Monero send needs, taken from the account the user picked.
///
/// Unlike the other chains a Monero transaction cannot be built from an address: choosing
/// inputs and computing their key images needs the spend key, and the change output needs
/// the view key. The snapshot wipes both when it drops.
fn xmr_sync_account(wallet: &crate::currencies::xmr::MoneroWallet) -> Option<xmr_chain::SyncAccount> {
    Some(xmr_chain::SyncAccount {
        spend_hex: wallet.private_key.clone()?,
        view_hex: wallet.private_view_key.clone()?,
        address: wallet.address.clone()?,
        restore_height: wallet.restore_height,
        birthday: wallet.birthday,
    })
}

fn xmr_send_view(app_settings: ApplicationSettings) -> gtk::Box {
    let xmr_mainnet = !xmr_chain::is_testnet(xmr_chain::parse_network(&app_settings.xmr_network));
    let SendChrome {
        page: box_,
        form,
        error,
        review,
        confirm_box,
        summary,
        confirm,
        cancel,
        ack,
        spends_real_value,
        network_note,
        status,
    } = send_chrome("xmr", xmr_mainnet, "I understand this spends real monero.");

    let names: Vec<String> = app_settings
        .xmr_wallets
        .iter()
        .map(|wallet| wallet.wallet_name.clone().unwrap_or_else(|| "Monero".to_string()))
        .collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let from_row = ui::combo_row("From account", &name_refs);
    let receive_address = ui::entry_row("Recipient");
    let amount = ui::entry_row("Amount (XMR)");
    // Monero's fee is a priority tier rather than a rate the user picks; the node quotes a
    // per-byte figure for each tier and the wallet caps it.
    let fee_row = ui::combo_row("Priority", &["Low", "Medium", "High"]);
    fee_row.set_selected(1);

    form.add(&from_row);
    form.add(&receive_address);
    form.add(&amount);
    form.add(&fee_row);

    let from_wallet = from_row.clone();
    let fee = fee_row.clone();

    let app_settings = Arc::new(Mutex::new(app_settings));
    let prepared = Arc::new(Mutex::new(None::<xmr_chain::PreparedSend>));

    let gate = ReviewGate::new(&confirm_box, &ack, &confirm, spends_real_value, &prepared);
    gate.watch_entry(&receive_address);
    gate.watch_entry(&amount);
    gate.watch_combo(&fee_row);
    gate.watch_combo(&from_row);

    review.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[strong] gate,
        #[weak] receive_address,
        #[weak] amount,
        #[weak] fee,
        #[weak] error,
        #[weak] summary,
        #[weak] from_wallet,
        move |_| {
            error.set_visible(false);
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.xmr_wallets.get(index) else {
                error.set_label("Select a Monero account.");
                error.set_visible(true);
                return;
            };
            let Some(account) = xmr_sync_account(wallet) else {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let to = receive_address.text().to_string();
            let network = xmr_chain::parse_network(&settings.xmr_network);
            if let Err(err) = xmr_chain::validate_address(&to, network) {
                error.set_label(&format!("{err:?}").replace("ERROR: ", "").replace('\"', ""));
                error.set_visible(true);
                return;
            }
            let node = settings.xmr_node.clone();
            let network_name = settings.xmr_network.clone();
            let testnet = xmr_chain::is_testnet(network);
            drop(settings);

            let labels = ["Low", "Medium", "High"];
            let fee_label = labels
                .get(fee.selected() as usize)
                .copied()
                .unwrap_or("Medium")
                .to_string();
            let amount_text = amount.text().to_string();
            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = xmr_chain::prepare_send(&account, &to, &amount_text, &node, &network_name, &fee_label);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] error,
                    #[weak] summary,
                    #[weak] network_note,
                    #[strong] prepared,
                    #[strong] gate,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(plan) => {
                                set_network_note(
                                    &network_note,
                                    testnet,
                                    if testnet {
                                        "Monero stagenet. These coins have no mainnet value."
                                    } else {
                                        "Mainnet. This spends real monero."
                                    },
                                );
                                summary.set_label(&plan.summary());
                                *prepared.lock().unwrap() = Some(plan);
                                gate.arm();
                            }
                            Err(why) => {
                                // Monero has more ways for a build to fail honestly than the
                                // other chains (not synced yet, everything still in the
                                // ten-block lock, decoys unavailable), so the reason is shown.
                                let reason = format!("{why:?}").replace("ERROR: ", "").replace('\"', "");
                                error.set_label(&format!("Could not build that transaction: {reason}. Receiving still works offline."));
                                error.set_visible(true);
                                gate.invalidate();
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    cancel.connect_clicked(clone!(
        #[strong] gate,
        move |_| gate.invalidate()
    ));

    confirm.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[weak] error,
        #[weak] status,
        #[strong] gate,
        #[weak] from_wallet,
        move |_| {
            let plan = match prepared.lock().unwrap().clone() {
                Some(plan) => plan,
                None => return,
            };
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.xmr_wallets.get(index) else {
                return;
            };
            let Some(account) = xmr_sync_account(wallet) else {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let node = settings.xmr_node.clone();
            let network_name = settings.xmr_network.clone();
            drop(settings);

            error.set_visible(false);
            status.set_label("Signing and broadcasting… Monero ring signatures take a few seconds.");
            status.set_visible(true);
            gate.invalidate();

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = xmr_chain::sign_and_broadcast(&account, &plan, &node, &network_name);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] status,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(txid) => {
                                status.set_label(&format!("Sent. Transaction ID: {txid}"));
                                ui::set_notice_warning(&status, false);
                                ui::toast("Transaction broadcast.");
                            }
                            Err(_) => {
                                status.set_label(
                                    "Broadcast failed. The node may be unreachable; your receive address is still valid offline.",
                                );
                                ui::set_notice_warning(&status, true);
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    scrolled(box_)
}

fn eth_send_view(app_settings: ApplicationSettings, token: Token) -> gtk::Box {
    // Gate on "is this a known testnet", not "is this literally mainnet" — an unrecognized or
    // new network name (e.g. an L2) must default to showing the real-value warning, not hiding it.
    let eth_real_value = !eth_chain::is_testnet(eth_chain::parse_network(&app_settings.eth_network));
    let SendChrome {
        page: box_,
        form,
        error,
        review,
        confirm_box,
        summary,
        confirm,
        cancel,
        ack,
        spends_real_value,
        network_note,
        status,
    } = send_chrome("eth", eth_real_value, "I understand this spends real value.");

    let names: Vec<String> = app_settings
        .eth_wallets
        .iter()
        .map(|wallet| {
            wallet
                .wallet_name
                .clone()
                .unwrap_or_else(|| "Ethereum".to_string())
        })
        .collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let symbol = token.symbol.clone();

    let from_row = ui::combo_row("From account", &name_refs);
    let receive_address = ui::entry_row("Recipient");
    let amount = ui::entry_row(&format!("Amount ({symbol})"));
    let fee_row = ui::combo_row("Network fee", &["Low", "Medium", "High"]);
    fee_row.set_selected(1);

    form.add(&from_row);
    form.add(&receive_address);
    form.add(&amount);
    form.add(&fee_row);

    let from_wallet = from_row.clone();
    let fee = fee_row.clone();

    let app_settings = Arc::new(Mutex::new(app_settings));
    let prepared = Arc::new(Mutex::new(None::<eth_chain::PreparedSend>));
    let token = Arc::new(token);

    let gate = ReviewGate::new(&confirm_box, &ack, &confirm, spends_real_value, &prepared);
    gate.watch_entry(&receive_address);
    gate.watch_entry(&amount);
    gate.watch_combo(&fee_row);
    gate.watch_combo(&from_row);

    review.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[strong] token,
        #[strong] gate,
        #[weak] receive_address,
        #[weak] amount,
        #[weak] fee,
        #[weak] error,
        #[weak] summary,
        #[weak] from_wallet,
        move |_| {
            error.set_visible(false);
            gate.invalidate();
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.eth_wallets.get(index) else {
                error.set_label("Select an Ethereum account.");
                error.set_visible(true);
                return;
            };
            let Some(from) = wallet.address.clone() else {
                error.set_label("Wallet address is missing.");
                error.set_visible(true);
                return;
            };
            if wallet.private_key.is_none() {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let to = receive_address.text().to_string();
            // Only a syntactic check here. An ENS name cannot be validated without a network
            // call, and the UI thread must not block on one, so resolution happens in the
            // worker below and the real verdict comes back with the quote.
            if eth_chain::validate_address(&to).is_err()
                && !crate::currencies::ens::looks_like_name(&to)
            {
                error.set_label("Recipient must be a 0x Ethereum address or an ENS name.");
                error.set_visible(true);
                return;
            }
            let node = settings.eth_node.clone();
            let network_name = settings.eth_network.clone();
            let infura_key = settings.infura_key.clone();
            let testnet = network_name.eq_ignore_ascii_case("sepolia");
            drop(settings);

            let labels = ["Low", "Medium", "High"];
            let fee_label = labels
                .get(fee.selected() as usize)
                .copied()
                .unwrap_or("Medium")
                .to_string();
            let amount_text = amount.text().to_string();
            let token = (*token).clone();
            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                // Resolve first, then quote against the resolved address. The plan therefore
                // carries the 0x address, and the review card shows that rather than the name,
                // so the user confirms what will actually be signed.
                let result = eth_chain::resolve_recipient(&to, &node, &network_name, &infura_key)
                    .and_then(|(address, _name)| {
                        eth_chain::prepare_send(
                            &from,
                            &format!("{address:?}"),
                            &amount_text,
                            &token,
                            &node,
                            &network_name,
                            &infura_key,
                            &fee_label,
                        )
                    });
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] error,
                    #[weak] summary,
                    #[weak] network_note,
                    #[strong] prepared,
                    #[strong] gate,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(plan) => {
                                set_network_note(
                                    &network_note,
                                    testnet,
                                    if testnet {
                                        "Sepolia testnet. These coins have no mainnet value."
                                    } else {
                                        "Live network. This spends real value."
                                    },
                                );
                                summary.set_label(&plan.summary());
                                *prepared.lock().unwrap() = Some(plan);
                                gate.arm();
                            }
                            Err(_) => {
                                error.set_label("Could not build that transaction. Check the amount, address and balance, and whether the Ethereum node is reachable. Receiving still works offline.");
                                error.set_visible(true);
                                gate.invalidate();
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    cancel.connect_clicked(clone!(
        #[strong] gate,
        move |_| gate.invalidate()
    ));

    confirm.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[weak] error,
        #[weak] status,
        #[strong] gate,
        #[weak] from_wallet,
        move |_| {
            let plan = match prepared.lock().unwrap().clone() {
                Some(plan) => plan,
                None => return,
            };
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.eth_wallets.get(index) else {
                return;
            };
            let Some(private_key) = wallet.private_key.clone() else {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let node = settings.eth_node.clone();
            let network_name = settings.eth_network.clone();
            let infura_key = settings.infura_key.clone();
            drop(settings);

            error.set_visible(false);
            status.set_label("Broadcasting…");
            status.set_visible(true);
            gate.invalidate();

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = eth_chain::sign_and_broadcast(
                    &private_key,
                    &plan,
                    &node,
                    &network_name,
                    &infura_key,
                );
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] status,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(txid) => {
                                status.set_label(&format!("Sent. Transaction ID: {txid}"));
                                ui::set_notice_warning(&status, false);
                                ui::toast("Transaction broadcast.");
                            }
                            Err(_) => {
                                status.set_label(
                                    "Broadcast failed. The node may be unreachable; your receive address is still valid offline.",
                                );
                                ui::set_notice_warning(&status, true);
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    scrolled(box_)
}

fn sol_send_view(app_settings: ApplicationSettings, token: Token) -> gtk::Box {
    let sol_mainnet = !app_settings.sol_network.eq_ignore_ascii_case("devnet");
    let SendChrome {
        page: box_,
        form,
        error,
        review,
        confirm_box,
        summary,
        confirm,
        cancel,
        ack,
        spends_real_value,
        network_note,
        status,
    } = send_chrome("sol", sol_mainnet, "I understand this spends real SOL or tokens.");

    let names: Vec<String> = app_settings
        .sol_wallets
        .iter()
        .map(|wallet| {
            wallet
                .wallet_name
                .clone()
                .unwrap_or_else(|| "Solana".to_string())
        })
        .collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let symbol = token.symbol.clone();

    let from_row = ui::combo_row("From account", &name_refs);
    let receive_address = ui::entry_row("Recipient");
    let amount = ui::entry_row(&format!("Amount ({symbol})"));

    form.add(&from_row);
    form.add(&receive_address);
    form.add(&amount);
    // Solana has no user-selectable fee tier: the network charges a flat per-signature
    // fee, so a Low/Medium/High control here would be a lie. Say so instead of leaving a
    // conspicuous gap where the other chains have a fee row.
    let fee_note = adw::ActionRow::builder()
        .title("Network fee")
        .subtitle("Flat 5000 lamports per signature")
        .build();
    fee_note.add_prefix(&gtk::Image::from_icon_name("emblem-system-symbolic"));
    form.add(&fee_note);

    let from_wallet = from_row.clone();

    let app_settings = Arc::new(Mutex::new(app_settings));
    let prepared = Arc::new(Mutex::new(None::<sol_chain::PreparedSend>));
    let token = Arc::new(token);

    let gate = ReviewGate::new(&confirm_box, &ack, &confirm, spends_real_value, &prepared);
    gate.watch_entry(&receive_address);
    gate.watch_entry(&amount);
    gate.watch_combo(&from_row);

    review.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[strong] token,
        #[strong] gate,
        #[weak] receive_address,
        #[weak] amount,
        #[weak] error,
        #[weak] summary,
        #[weak] from_wallet,
        move |_| {
            error.set_visible(false);
            gate.invalidate();
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.sol_wallets.get(index) else {
                error.set_label("Select a Solana account.");
                error.set_visible(true);
                return;
            };
            let Some(from) = wallet.address.clone() else {
                error.set_label("Wallet address is missing.");
                error.set_visible(true);
                return;
            };
            if wallet.private_key.is_none() {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let to = receive_address.text().to_string();
            if sol_chain::validate_address(&to).is_err() {
                error.set_label("Recipient must be a base58 Solana address.");
                error.set_visible(true);
                return;
            }
            let node = settings.sol_node.clone();
            let network_name = settings.sol_network.clone();
            let testnet = network_name.eq_ignore_ascii_case("devnet");
            drop(settings);

            let amount_text = amount.text().to_string();
            let token = (*token).clone();
            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = sol_chain::prepare_send(&from, &to, &amount_text, &token, &node, &network_name);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] error,
                    #[weak] summary,
                    #[weak] network_note,
                    #[strong] prepared,
                    #[strong] gate,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(plan) => {
                                set_network_note(
                                    &network_note,
                                    testnet,
                                    if testnet {
                                        "Solana devnet. These coins have no mainnet value."
                                    } else {
                                        "Mainnet. This spends real SOL or tokens."
                                    },
                                );
                                summary.set_label(&plan.summary());
                                *prepared.lock().unwrap() = Some(plan);
                                gate.arm();
                            }
                            Err(_) => {
                                error.set_label("Could not build that transaction. Check the amount, address and balance, and whether the Solana node is reachable. Receiving still works offline.");
                                error.set_visible(true);
                                gate.invalidate();
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    cancel.connect_clicked(clone!(
        #[strong] gate,
        move |_| gate.invalidate()
    ));

    confirm.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] prepared,
        #[weak] error,
        #[weak] status,
        #[strong] gate,
        #[weak] from_wallet,
        move |_| {
            let plan = match prepared.lock().unwrap().clone() {
                Some(plan) => plan,
                None => return,
            };
            let settings = app_settings.lock().unwrap();
            let index = from_wallet.selected() as usize;
            let Some(wallet) = settings.sol_wallets.get(index) else {
                return;
            };
            let Some(private_key) = wallet.private_key.clone() else {
                error.set_label("Wallet is locked.");
                error.set_visible(true);
                return;
            };
            let node = settings.sol_node.clone();
            let network_name = settings.sol_network.clone();
            drop(settings);

            error.set_visible(false);
            status.set_label("Broadcasting…");
            status.set_visible(true);
            gate.invalidate();

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = sol_chain::sign_and_broadcast(&private_key, &plan, &node, &network_name);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] status,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(txid) => {
                                status.set_label(&format!("Sent. Transaction ID: {txid}"));
                                ui::set_notice_warning(&status, false);
                                ui::toast("Transaction broadcast.");
                            }
                            Err(_) => {
                                status.set_label(
                                    "Broadcast failed. The node may be unreachable; your receive address is still valid offline.",
                                );
                                ui::set_notice_warning(&status, true);
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    scrolled(box_)
}

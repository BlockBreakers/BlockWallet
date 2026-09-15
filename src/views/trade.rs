//! The swap screen.
//!
//! Structurally a sibling of the send screen, and deliberately so: the same review-then-
//! confirm shape, the same acknowledgement gate on real value, and the same rule that a
//! reviewed plan dies the moment any input it was built from changes. Swaps have more moving
//! parts than sends, which is a reason to reuse a flow people already understand rather than
//! invent a second one.
//!
//! What is specific to swapping: several providers are asked at once and their answers ranked,
//! the ones that refuse say why rather than vanishing, and any route where a protocol vault
//! holds the funds in flight is labelled as such before it can be confirmed.

use adw::prelude::*;
use glib::{clone, ControlFlow};
use gtk::{Button, Orientation};
use pango::WrapMode;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::configuration::application_settings::ApplicationSettings;
use crate::currencies::swap::{
    self, execute::SigningContext, Custody, SwapAsset, SwapQuote, SwapRequest,
};
use crate::currencies::tokens::Token;
use crate::views::ui;

/// Assets the swap screen offers, drawn from what the wallet actually holds tokens for.
fn swappable_assets(settings: &ApplicationSettings) -> Vec<(String, SwapAsset)> {
    let mut out: Vec<(String, SwapAsset)> = Vec::new();
    let mut tokens: Vec<&Token> = settings.tokens.eth_tokens.values().collect();
    // Stable ordering so the dropdown does not reshuffle between visits.
    tokens.sort_by(|a, b| (a.chain.as_str(), a.symbol.as_str()).cmp(&(b.chain.as_str(), b.symbol.as_str())));
    for token in tokens {
        // No venue this wallet talks to has a Monero pool: THORChain and Maya never listed
        // it, and the aggregators are single-chain. Listing it would only ever produce
        // "no offers", so it is left out rather than shown as a swap that cannot happen.
        if token.chain == "xmr" {
            continue;
        }
        let asset = SwapAsset::from_token(token);
        let label = format!("{} ({})", asset.symbol, ui::chain_display_name(&asset.chain));
        if !out.iter().any(|(existing, _)| existing == &label) {
            out.push((label, asset));
        }
    }
    out
}

/// The account this wallet would spend from for a given chain.
fn source_address(settings: &ApplicationSettings, chain: &str) -> Option<String> {
    match chain {
        "btc" => settings.btc_wallets.first().and_then(|w| w.address.clone()),
        "eth" => settings.eth_wallets.first().and_then(|w| w.address.clone()),
        "sol" => settings.sol_wallets.first().and_then(|w| w.address.clone()),
        "ltc" => settings.ltc_wallets.first().and_then(|w| w.address.clone()),
        _ => None,
    }
}

/// Affiliate payout addresses, read from settings.
///
/// An unset address means that venue is asked for no fee at all, so a build with none
/// configured quotes exactly as it did before the fee existed.
fn fee_payout(settings: &ApplicationSettings) -> swap::FeePayout {
    swap::FeePayout {
        evm: settings.fee_evm_address.clone(),
        lifi_integrator: settings.fee_lifi_integrator.clone(),
        solana: settings.fee_solana_account.clone(),
        thorchain: settings.fee_thorchain_address.clone(),
        maya: settings.fee_maya_address.clone(),
    }
}

fn signing_context(settings: &ApplicationSettings) -> SigningContext {
    SigningContext {
        btc_mnemonic: settings.btc_wallets.first().and_then(|w| w.mnemonic.clone()),
        btc_passphrase: settings
            .btc_wallets
            .first()
            .and_then(|w| w.password.clone())
            .unwrap_or_default(),
        ltc_private_key: settings.ltc_wallets.first().and_then(|w| w.private_key.clone()),
        eth_private_key: settings.eth_wallets.first().and_then(|w| w.private_key.clone()),
        sol_private_key: settings.sol_wallets.first().and_then(|w| w.private_key.clone()),
        sol_address: settings
            .sol_wallets
            .first()
            .and_then(|w| w.address.clone())
            .unwrap_or_default(),
        btc_node: settings.btc_node.clone(),
        btc_network: settings.btc_network.clone(),
        ltc_node: settings.ltc_node.clone(),
        ltc_network: settings.ltc_network.clone(),
        eth_node: settings.eth_node.clone(),
        eth_network: settings.eth_network.clone(),
        infura_key: settings.infura_key.clone(),
        sol_node: settings.sol_node.clone(),
        sol_network: settings.sol_network.clone(),
    }
}

/// Discards a chosen quote and hides the review card.
///
/// Same reasoning as the send screen's gate: a quote the user has read must never survive a
/// change to the assets or the amount, and consent to a real-value swap is per swap rather
/// than per screen. Widgets are held weakly because the Confirm button owns a handler that
/// captures this, and a strong reference back would leak the page on every visit.
#[derive(Clone)]
struct SwapGate {
    review_card: glib::WeakRef<gtk::Box>,
    ack: glib::WeakRef<gtk::CheckButton>,
    confirm: glib::WeakRef<Button>,
    chosen: Rc<Mutex<Option<(SwapQuote, SwapRequest)>>>,
}

impl SwapGate {
    fn rearm(&self) {
        if let Some(ack) = self.ack.upgrade() {
            ack.set_active(false);
        }
        if let Some(confirm) = self.confirm.upgrade() {
            confirm.set_sensitive(false);
        }
    }

    fn arm(&self, quote: SwapQuote, request: SwapRequest) {
        *self.chosen.lock().unwrap() = Some((quote, request));
        self.rearm();
        if let Some(card) = self.review_card.upgrade() {
            card.set_visible(true);
        }
    }

    fn invalidate(&self) {
        *self.chosen.lock().unwrap() = None;
        self.rearm();
        if let Some(card) = self.review_card.upgrade() {
            card.set_visible(false);
        }
    }

    fn watch_entry(&self, row: &adw::EntryRow) {
        let gate = self.clone();
        row.connect_changed(move |_| gate.invalidate());
    }

    fn watch_picker(&self, row: &ui::PickerRow) {
        let gate = self.clone();
        row.connect_changed(move || gate.invalidate());
    }
}

/// One provider's offer, rendered as a selectable row.
fn quote_row(quote: &SwapQuote) -> adw::ActionRow {
    let custody_note = match quote.custody {
        Custody::AtomicOnChain => "settles in one transaction",
        Custody::ProtocolVault => "held by a protocol vault in flight",
    };
    let eta = quote
        .eta_seconds
        .map(|s| {
            if s < 120 {
                format!("{s}s")
            } else {
                format!("{} min", s / 60)
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    let row = adw::ActionRow::builder()
        .title(format!(
            "{} {}",
            quote.expected_out_display(),
            quote.to.symbol
        ))
        // The total cost is stated on the offer itself, not only at review, since this is
        // the screen where offers get compared with each other. Same single figure and same
        // name as the review card, so the two never appear to disagree.
        .subtitle(match swap::swap_fee_line(quote) {
            Some(fee) => format!(
                "{} · {} · ~{} · swap fee {}",
                quote.provider_name, custody_note, eta, fee
            ),
            None => format!("{} · {} · ~{}", quote.provider_name, custody_note, eta),
        })
        .activatable(true)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(
        if quote.custody == Custody::AtomicOnChain {
            "emblem-ok-symbolic"
        } else {
            "network-transmit-receive-symbolic"
        },
    ));
    row
}

pub fn trade_view(
    app_settings: Arc<Mutex<ApplicationSettings>>,
) -> (gtk::Box, Arc<Mutex<ApplicationSettings>>) {
    let page = ui::page_body(14);
    page.append(&ui::heading("Swap"));

    let assets = swappable_assets(&app_settings.lock().unwrap());
    if assets.len() < 2 {
        page.append(&ui::empty_state(
            "Nothing to swap yet",
            "Add or unlock an account first. Swaps need at least two assets.",
            "object-flip-horizontal-symbolic",
        ));
        let outer = ui::vbox(0);
        outer.append(&ui::scroller(&page));
        return (outer, app_settings);
    }

    // Searchable pickers rather than dropdowns: the bundled list runs to a few hundred tokens
    // on Ethereum mainnet, and an AdwComboRow that long cannot be scanned on a phone.
    let labels: Vec<String> = assets.iter().map(|(label, _)| label.clone()).collect();

    let form = ui::group("Swap");
    let from_row = ui::PickerRow::new("From", &labels);
    let to_row = ui::PickerRow::new("To", &labels);
    // Anything other than the same asset on both sides, so the screen opens on a valid pair.
    if labels.len() > 1 {
        to_row.set_selected(1);
    }
    let amount = ui::entry_row("Amount");
    form.add(from_row.row());
    form.add(to_row.row());
    form.add(&amount);
    page.append(&form);

    let get_quotes = ui::primary_button("Find the best rate");
    page.append(&get_quotes);

    let error = ui::error_label("");
    page.append(&error);

    let status = ui::notice("");
    status.set_visible(false);
    page.append(&status);

    // ---- offers ----
    let offers = ui::group_with_description(
        "Offers",
        "Best rate first. Tap one to review it.",
    );
    let offers_box = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .visible(false)
        .build();
    offers_box.append(&offers);
    page.append(&offers_box);

    // The rows currently in the offers group, so they can be removed by identity when the
    // list is refreshed. An AdwPreferencesGroup gives no way to enumerate what was added to
    // it, and walking its widget children hits the internal box instead of the rows.
    let shown: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));

    let rejected = ui::dim("");
    rejected.set_wrap(true);
    rejected.set_wrap_mode(WrapMode::WordChar);
    rejected.set_visible(false);
    page.append(&rejected);

    // ---- review ----
    let review_card = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .visible(false)
        .css_classes(["review-card"])
        .build();

    let custody_note = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .xalign(0.0)
        .build();
    let summary = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .css_classes(["review-summary"])
        .build();
    let ack = gtk::CheckButton::with_label("I understand this spends real value and cannot be undone.");
    let confirm = ui::primary_button("Confirm swap");
    confirm.set_sensitive(false);
    let cancel = ui::button("Cancel");

    review_card.append(&custody_note);
    review_card.append(&ui::heading("Review"));
    review_card.append(&summary);
    review_card.append(&ack);
    review_card.append(&confirm);
    review_card.append(&cancel);
    page.append(&review_card);

    let confirm_gate = confirm.clone();
    ack.connect_toggled(move |cb| confirm_gate.set_sensitive(cb.is_active()));

    let gate = SwapGate {
        review_card: review_card.downgrade(),
        ack: ack.downgrade(),
        confirm: confirm.downgrade(),
        chosen: Rc::new(Mutex::new(None)),
    };
    gate.watch_entry(&amount);
    gate.watch_picker(&from_row);
    gate.watch_picker(&to_row);

    let assets = Rc::new(assets);

    // ---- find quotes ----
    get_quotes.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] gate,
        #[strong] assets,
        #[strong] shown,
        #[weak] amount,
        // Strong: `PickerRow` is a plain struct rather than a GObject, so it cannot be held
        // weakly. It holds no reference back to this button, and the handlers it does hold
        // keep only weak references to widgets, so this closes no reference cycle.
        #[strong] from_row,
        #[strong] to_row,
        #[weak] error,
        #[weak] status,
        #[weak] offers,
        #[weak] offers_box,
        #[weak] rejected,
        move |_| {
            error.set_visible(false);
            rejected.set_visible(false);
            gate.invalidate();

            let Some((_, from)) = assets.get(from_row.selected() as usize).cloned() else { return };
            let Some((_, to)) = assets.get(to_row.selected() as usize).cloned() else { return };
            if from == to {
                error.set_label("Pick two different assets.");
                error.set_visible(true);
                return;
            }

            let settings = app_settings.lock().unwrap();
            let Some(from_address) = source_address(&settings, &from.chain) else {
                error.set_label("No account for that chain. Unlock the wallet first.");
                error.set_visible(true);
                return;
            };
            let Some(destination) = source_address(&settings, &to.chain) else {
                error.set_label("No account to receive on that chain.");
                error.set_visible(true);
                return;
            };

            // Amounts are parsed with the same locale-safe normaliser the send screens use,
            // so a comma decimal separator cannot silently multiply the input.
            let amount_text = amount.text().to_string();
            let amount_in_base = match parse_amount(&amount_text, from.decimals) {
                Ok(value) if value > 0 => value,
                Ok(_) => {
                    error.set_label("Amount must be greater than 0.");
                    error.set_visible(true);
                    return;
                }
                Err(message) => {
                    error.set_label(&message);
                    error.set_visible(true);
                    return;
                }
            };

            let evm_chain_id = crate::currencies::swap::lifi::chain_id_for(
                crate::currencies::eth_chain::parse_network(&settings.eth_network),
            );
            let request = SwapRequest {
                from,
                to,
                amount_in_base,
                from_address,
                destination,
                slippage_bps: swap::safety::DEFAULT_SLIPPAGE_BPS,
                evm_chain_id,
                thornode_url: settings.thornode_url.clone(),
                fee: fee_payout(&settings),
                sol_node: settings.sol_node.clone(),
                sol_network: settings.sol_network.clone(),
            };
            drop(settings);

            status.set_label("Asking every provider…");
            ui::set_notice_warning(&status, false);
            status.set_visible(true);
            offers_box.set_visible(false);

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            let for_thread = request.clone();
            thread::spawn(move || {
                let set = swap::collect_quotes(&for_thread);
                let _ = sender.send_blocking(set);
            });

            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[weak] status,
                    #[weak] offers,
                    #[weak] offers_box,
                    #[weak] rejected,
                    #[strong] gate,
                    #[strong] shown,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |set| {
                        // Clear the previous offers by removing exactly the rows that were
                        // added, tracked in `shown`.
                        //
                        // Emphatically not by walking `first_child`: an AdwPreferencesGroup's
                        // first child is its own internal box, which `remove` will not take,
                        // so `while let Some(child) = offers.first_child()` spins forever and
                        // hangs the GTK main thread. That ran before the empty check, so on a
                        // testnet where every provider declines it froze the app outright.
                        for row in shown.borrow_mut().drain(..) {
                            offers.remove(&row);
                        }

                        if set.quotes.is_empty() {
                            status.set_label("No provider could route that swap.");
                            ui::set_notice_warning(&status, true);
                            status.set_visible(true);
                            offers_box.set_visible(false);
                        } else {
                            status.set_visible(false);
                            for quote in &set.quotes {
                                let row = quote_row(quote);
                                shown.borrow_mut().push(row.clone());
                                let gate = gate.clone();
                                let quote = quote.clone();
                                let request = request.clone();
                                row.connect_activated(move |_| {
                                    gate.arm(quote.clone(), request.clone());
                                });
                                offers.add(&row);
                            }
                            offers_box.set_visible(true);
                        }

                        // Providers that declined say why. A swap silently missing a venue is
                        // more confusing than one that explains a halt or an unsupported pair.
                        if set.rejected.is_empty() {
                            rejected.set_visible(false);
                        } else {
                            let lines: Vec<String> = set
                                .rejected
                                .iter()
                                .map(|(id, why)| format!("{id}: {why}"))
                                .collect();
                            rejected.set_label(&format!("Not offered\n{}", lines.join("\n")));
                            rejected.set_visible(true);
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    // ---- review card is populated when an offer is chosen ----
    {
        let chosen = gate.chosen.clone();
        let summary = summary.clone();
        let custody_note = custody_note.clone();
        let review_card = review_card.clone();
        review_card.connect_visible_notify(move |card| {
            if !card.is_visible() {
                return;
            }
            let Some((quote, _)) = chosen.lock().unwrap().clone() else { return };
            custody_note.set_label(quote.custody.describe());
            custody_note.remove_css_class("testnet-note");
            custody_note.remove_css_class("spend-warning");
            custody_note.add_css_class(match quote.custody {
                Custody::AtomicOnChain => "testnet-note",
                Custody::ProtocolVault => "spend-warning",
            });
            // One "Swap fee" line carrying the whole cost, not one line per component.
            //
            // The venues that report a total already fold the affiliate cut into it, so
            // showing the venue's fee and the wallet's separately would read as more being
            // taken than actually is. The route note is a different thing again (which DEX,
            // what price impact) and stays on its own line rather than being mistaken for a
            // charge, which is what happened when both shared a field.
            let swap_fee = swap::swap_fee_line(&quote)
                .map(|line| format!("\nSwap fee: {line}"))
                .unwrap_or_default();
            let route = quote
                .route_note
                .as_ref()
                .map(|note| format!("\nRoute: {note}"))
                .unwrap_or_default();
            summary.set_label(&format!(
                "Provider: {}\nYou send: {} {}\nYou receive at least: {} {}\nExpected: {} {}\nSettles to: {}{}{}",
                quote.provider_name,
                quote.amount_in_display(),
                quote.from.symbol,
                quote.min_out_display(),
                quote.to.symbol,
                quote.expected_out_display(),
                quote.to.symbol,
                ui::short_address(&quote.destination),
                swap_fee,
                route,
            ));
        });
    }

    cancel.connect_clicked(clone!(
        #[strong] gate,
        move |_| gate.invalidate()
    ));

    // ---- execute ----
    confirm.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] gate,
        #[weak] error,
        #[weak] status,
        move |_| {
            let Some((quote, request)) = gate.chosen.lock().unwrap().clone() else { return };
            let context = signing_context(&app_settings.lock().unwrap());

            error.set_visible(false);
            status.set_label("Signing and broadcasting…");
            ui::set_notice_warning(&status, false);
            status.set_visible(true);
            // Clears the chosen quote too, so a second tap cannot broadcast it twice.
            gate.invalidate();

            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = swap::execute::execute(&quote, &request, &context);
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
                            Ok(receipt) => {
                                let mut message = String::new();
                                if let Some(approval) = &receipt.approval_txid {
                                    message.push_str(&format!(
                                        "Approval sent: {}\n",
                                        ui::short_address(approval)
                                    ));
                                }
                                message.push_str(&format!("Swap sent: {}", receipt.txid));
                                if receipt.pending_cross_chain {
                                    message.push_str(
                                        "\nThe other side settles once the network confirms this. \
                                         It can take several minutes.",
                                    );
                                }
                                status.set_label(&message);
                                ui::set_notice_warning(&status, false);
                                ui::toast("Swap broadcast.");
                            }
                            Err(why) => {
                                status.set_label(&format!("Swap failed. {why}"));
                                ui::set_notice_warning(&status, true);
                            }
                        }
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    let outer = ui::vbox(0);
    outer.append(&ui::scroller(&page));
    (outer, app_settings)
}

/// Parse a typed amount into the asset's base units.
///
/// Shares the locale-safe normaliser with the send screens, so `1,5` cannot be read as `15`.
fn parse_amount(text: &str, decimals: u8) -> Result<u128, String> {
    let normalized = crate::currencies::amount::normalize_decimal_input(text)
        .map_err(|why| format!("{why}"))?;
    let (whole, frac) = match normalized.split_once('.') {
        Some((whole, frac)) => (whole, frac),
        None => (normalized.as_str(), ""),
    };
    if frac.len() > decimals as usize {
        return Err(format!("Amount has more than {decimals} decimal places."));
    }
    let mut padded = frac.to_string();
    while padded.len() < decimals as usize {
        padded.push('0');
    }
    let combined = format!("{}{}", if whole.is_empty() { "0" } else { whole }, padded);
    combined
        .trim_start_matches('0')
        .parse::<u128>()
        .or_else(|_| if combined.chars().all(|c| c == '0') { Ok(0) } else { Err(()) })
        .map_err(|_| "Amount is not a number this wallet can handle.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_convert_to_base_units_at_each_precision() {
        assert_eq!(parse_amount("1", 8).unwrap(), 100_000_000);
        assert_eq!(parse_amount("0.001", 8).unwrap(), 100_000);
        assert_eq!(parse_amount("1.5", 18).unwrap(), 1_500_000_000_000_000_000);
        assert_eq!(parse_amount("2.5", 6).unwrap(), 2_500_000);
        assert_eq!(parse_amount("0", 8).unwrap(), 0);
    }

    #[test]
    fn a_comma_decimal_separator_cannot_multiply_the_amount() {
        // The same hazard the send screens guard against, reaching the swap screen too.
        assert_eq!(parse_amount("1,5", 8).unwrap(), 150_000_000);
        assert_eq!(parse_amount("1.5", 8).unwrap(), 150_000_000);
    }

    #[test]
    fn too_many_decimal_places_is_refused_rather_than_truncated() {
        assert!(parse_amount("0.000000001", 8).is_err());
        assert!(parse_amount("1.1234567", 6).is_err());
    }

    #[test]
    fn junk_is_refused() {
        assert!(parse_amount("", 8).is_err());
        assert!(parse_amount("abc", 8).is_err());
        assert!(parse_amount("-1", 8).is_err());
        assert!(parse_amount("1,234.56", 8).is_err());
    }
}

use adw::prelude::*;
use adw::ApplicationWindow;
use glib::{clone, ControlFlow};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::configuration::application_settings::*;
use crate::configuration::endpoint;
use crate::configuration::wallet_store::CustomTokenRecord;
use crate::currencies::eth_chain;
use crate::currencies::sol_chain;
use crate::currencies::xmr_chain;
use crate::views::ui;
use crate::views::{login, stack};

// Index 1 ("sepolia") is load-bearing: the "Use test networks" toggle hardcodes
// `eth_network.set_selected(1)` to mean Sepolia.
const ETH_NETWORKS: [&str; 8] = [
    "mainnet", "sepolia", "arbitrum", "base", "optimism", "polygon", "bsc", "avalanche",
];

/// Friendly names for the network dropdown. Same order and length as `ETH_NETWORKS`, so
/// index 1 still means Sepolia and the test-networks toggle keeps working.
const ETH_NETWORK_LABELS: [&str; 8] = [
    "Ethereum Mainnet",
    "Sepolia (testnet)",
    "Arbitrum One",
    "Base",
    "Optimism",
    "Polygon PoS",
    "BNB Smart Chain",
    "Avalanche C-Chain",
];

const TIMEOUT_LABELS: [&str; 4] = ["Off", "1 minute", "2 minutes", "5 minutes"];
const TIMEOUT_VALUES: [u32; 4] = [0, 60, 120, 300];

/// An "add token by address" control: an entry row with an inline Add button and a status
/// line underneath.
struct AddTokenControls {
    entry: adw::EntryRow,
    add: gtk::Button,
    status: gtk::Label,
}

fn add_token_controls(group: &adw::PreferencesGroup, title: &str, placeholder: &str) -> AddTokenControls {
    let entry = adw::EntryRow::builder().title(title).build();
    let add = gtk::Button::from_icon_name("list-add-symbolic");
    add.set_tooltip_text(Some("Add token"));
    add.set_valign(gtk::Align::Center);
    add.add_css_class("flat");
    entry.add_suffix(&add);
    group.add(&entry);

    let status = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .visible(false)
        .margin_top(4)
        .css_classes(["info-banner"])
        .build();
    group.add(&status);

    let _ = placeholder;
    AddTokenControls { entry, add, status }
}

pub fn settings_view(window: ApplicationWindow, app_settings: Arc<Mutex<ApplicationSettings>>) {
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(
        &gtk::Label::builder()
            .label("Settings")
            .css_classes(["title-4"])
            .build(),
    ));
    let back = ui::flat_icon_button("go-previous-symbolic", "Back");
    header_bar.pack_start(&back);

    // AdwPreferencesPage gives grouped, titled, boxed-list sections with correct phone
    // margins for free. The previous screen was a bare column of unlabelled dropdowns and
    // entries, where the only clue to a field's purpose was placeholder text that
    // disappeared as soon as you typed in it.
    let page = adw::PreferencesPage::new();

    // ------------------------------------------------------------------ security
    let security = ui::group("Security");
    let timeout = ui::combo_row("Auto-lock", &TIMEOUT_LABELS);
    timeout.set_subtitle("Lock the wallet after this much idle time");
    let current_timeout = app_settings.lock().unwrap().lock_timeout_secs;
    timeout.set_selected(
        TIMEOUT_VALUES
            .iter()
            .position(|v| *v == current_timeout)
            .unwrap_or(2) as u32,
    );
    security.add(&timeout);
    page.add(&security);

    // ------------------------------------------------------------------- display
    let display = ui::group("Display");
    let theme = ui::combo_row("Appearance", &["Follow system", "Light", "Dark"]);
    theme.set_selected(crate::configuration::theme::load() as u32);
    // Applied and saved the moment it is picked, rather than waiting for the Save button. A
    // theme choice you cannot see the result of until you press something else is a guess.
    theme.connect_selected_notify(move |row| {
        let choice = crate::configuration::theme::Appearance::from_index(row.selected());
        choice.apply();
        crate::configuration::theme::save(choice);
    });
    display.add(&theme);
    // `add_switch_row` adds its row on the spot, so it has to come before the rows that
    // should sit under it.
    let prices = ui::add_switch_row(
        &display,
        "Fiat prices",
        "Fetches quotes from CoinGecko. Off by default.",
        app_settings.lock().unwrap().show_prices,
    );
    let fiat = ui::combo_row("Currency", &["US dollar", "Euro"]);
    if app_settings.lock().unwrap().fiat.eq_ignore_ascii_case("eur") {
        fiat.set_selected(1);
    }
    let hide_zero = ui::add_switch_row(
        &display,
        "Hide empty assets",
        "Leaves out assets with a zero balance. A syncing or unreachable one is never hidden, and the Assets screen keeps a button to show them so you can still receive.",
        app_settings.lock().unwrap().hide_zero_balances,
    );
    let units = ui::combo_row("Bitcoin units", &["BTC", "Satoshis"]);
    if app_settings.lock().unwrap().btc_units.eq_ignore_ascii_case("sats") {
        units.set_selected(1);
    }
    display.add(&fiat);
    display.add(&units);
    page.add(&display);

    let save_display = ui::primary_button("Save display and security");
    let save_display_group = adw::PreferencesGroup::new();
    save_display_group.add(&save_display);
    page.add(&save_display_group);

    // ------------------------------------------------------------------ networks
    let network = network_settings(&page, app_settings.clone());

    // -------------------------------------------------------------------- danger
    let session = ui::group("Session");
    let logout_button = ui::button("Lock wallet");
    logout_button.add_css_class("destructive-action");
    logout_button.add_css_class("pill-button");
    session.add(&logout_button);
    if app_settings.lock().unwrap().is_unlocked() {
        page.add(&session);
    }

    let scroll = ui::scroller(&page);
    let setting_box = ui::vbox(0);
    setting_box.append(&header_bar);
    setting_box.append(&scroll);

    let overlay = ui::with_toasts(&setting_box);
    window.set_content(Some(&overlay));
    window.present();

    save_display.connect_clicked(clone!(
        #[strong] app_settings,
        #[weak] timeout,
        #[weak] prices,
        #[weak] fiat,
        #[weak] units,
        #[weak] hide_zero,
        move |_| {
            let secs = TIMEOUT_VALUES
                .get(timeout.selected() as usize)
                .copied()
                .unwrap_or(120);
            let mut settings = app_settings.lock().unwrap();
            settings.lock_timeout_secs = secs;
            settings.show_prices = prices.is_active();
            settings.fiat = if fiat.selected() == 1 { "eur".into() } else { "usd".into() };
            settings.btc_units = if units.selected() == 1 { "sats".into() } else { "btc".into() };
            settings.hide_zero_balances = hide_zero.is_active();
            let _ = settings.write_config();
            drop(settings);
            ui::toast("Display and security settings saved.");
        }
    ));

    logout_button.connect_clicked(clone!(
        #[weak] window,
        #[strong] app_settings,
        move |_| {
            login::lock_and_show(window.clone(), app_settings.clone());
        }
    ));

    back.connect_clicked(clone!(
        #[weak] window,
        #[strong] app_settings,
        move |_| {
            if app_settings.lock().unwrap().is_unlocked() {
                stack::stack_view(&window, app_settings.lock().unwrap().clone());
            } else {
                login::lock_and_show(window.clone(), app_settings.clone());
            }
        }
    ));

    // Keep the network widgets alive for as long as the page is on screen.
    let _ = network;
}

/// Widgets the network section needs to keep referenced after construction.
struct NetworkWidgets {
    _keep: (),
}

fn network_settings(page: &adw::PreferencesPage, app_settings: Arc<Mutex<ApplicationSettings>>) -> NetworkWidgets {
    // ---- master test-network switch ----
    let networks = ui::group_with_description(
        "Networks",
        "Test networks use worthless coins. Layer 2 networks are real value and are chosen per chain below.",
    );
    let testnet = ui::add_switch_row(
        &networks,
        "Use test networks",
        "Bitcoin testnet, Ethereum Sepolia, Solana devnet, Litecoin testnet, Monero stagenet",
        app_settings.lock().unwrap().is_test_mode(),
    );
    page.add(&networks);

    // ---- Bitcoin ----
    let btc_group = ui::group("Bitcoin");
    let btc_network = ui::combo_row("Network", &["Mainnet", "Testnet"]);
    if app_settings.lock().unwrap().btc_network.to_ascii_lowercase() == "testnet" {
        btc_network.set_selected(1);
    }
    let btc_node = adw::EntryRow::builder().title("Node URL").build();
    btc_node.set_text(&app_settings.lock().unwrap().btc_node);
    let btc_hint = adw::ActionRow::builder()
        .title("Accepted formats")
        .subtitle("Esplora https://… or Electrum ssl://host:port. Plaintext http:// and tcp:// only to localhost. Leave empty for the default.")
        .build();
    btc_hint.add_prefix(&gtk::Image::from_icon_name("network-wired-symbolic"));
    let btc_incorrect_format = ui::error_label(
        "Bitcoin node should be an https Esplora URL or an ssl://host:port Electrum server. Plaintext http:// or tcp:// is only accepted for a node on this device.",
    );
    btc_group.add(&btc_network);
    btc_group.add(&btc_node);
    btc_group.add(&btc_hint);
    btc_group.add(&btc_incorrect_format);
    page.add(&btc_group);

    // ---- Ethereum and its L2s ----
    let eth_group = ui::group_with_description(
        "Ethereum and layer 2",
        "One address works across every network here. Layer 2 networks spend real value.",
    );
    let eth_network = ui::combo_row("Network", &ETH_NETWORK_LABELS);
    let current_eth = app_settings.lock().unwrap().eth_network.to_ascii_lowercase();
    if let Some(index) = ETH_NETWORKS.iter().position(|n| *n == current_eth) {
        eth_network.set_selected(index as u32);
    }
    let ethereum_node = adw::EntryRow::builder().title("RPC URL").build();
    ethereum_node.set_text(&app_settings.lock().unwrap().eth_node);
    let eth_hint = adw::ActionRow::builder()
        .title("Leave empty")
        .subtitle("Uses a public default for the selected network. Public nodes can see your address.")
        .build();
    eth_hint.add_prefix(&gtk::Image::from_icon_name("network-wired-symbolic"));
    let infura_key = adw::PasswordEntryRow::builder().title("Infura API key").build();
    let etherscan_api_key = adw::PasswordEntryRow::builder().title("Etherscan API key").build();
    let eth_incorrect_format =
        ui::error_label("Ethereum RPC must be https://, or empty for the network default. Plaintext http:// is only accepted for a node on this device.");
    eth_group.add(&eth_network);
    eth_group.add(&ethereum_node);
    eth_group.add(&eth_hint);
    eth_group.add(&infura_key);
    eth_group.add(&etherscan_api_key);
    eth_group.add(&eth_incorrect_format);
    page.add(&eth_group);

    let erc20_group = ui::group_with_description(
        "ERC-20 tokens",
        "Add a token by its contract address. Name and decimals are read from the chain.",
    );
    let erc20 = add_token_controls(&erc20_group, "Contract address (0x…)", "0x…");
    page.add(&erc20_group);

    // ---- Solana ----
    let sol_group = ui::group("Solana");
    let sol_network = ui::combo_row("Network", &["Mainnet", "Devnet"]);
    if app_settings.lock().unwrap().sol_network.to_ascii_lowercase() == "devnet" {
        sol_network.set_selected(1);
    }
    let solana_node = adw::EntryRow::builder().title("RPC URL").build();
    solana_node.set_text(&app_settings.lock().unwrap().sol_node);
    let sol_incorrect_format =
        ui::error_label("Solana RPC must be https://, or empty for the network default. Plaintext http:// is only accepted for a node on this device.");
    sol_group.add(&sol_network);
    sol_group.add(&solana_node);
    sol_group.add(&sol_incorrect_format);
    page.add(&sol_group);

    let spl_group = ui::group_with_description(
        "SPL tokens",
        "Add a Solana token by its mint address.",
    );
    let spl = add_token_controls(&spl_group, "Mint address", "base58");
    page.add(&spl_group);

    // ---- Swaps ----
    let swap_group = ui::group_with_description(
        "Swaps",
        "Cross-chain swaps route through THORChain. Same-chain swaps use a DEX aggregator and need no endpoint here.",
    );
    let thornode = adw::EntryRow::builder().title("THORNode URL").build();
    thornode.set_text(&app_settings.lock().unwrap().thornode_url);
    let thornode_incorrect_format = ui::error_label(
        "THORNode must be an https URL, or empty for the public default. Plaintext http:// is only accepted for a node on this device.",
    );
    swap_group.add(&thornode);
    swap_group.add(&thornode_incorrect_format);
    page.add(&swap_group);

    // ---- Litecoin ----
    let ltc_group = ui::group("Litecoin");
    let ltc_network = ui::combo_row("Network", &["Mainnet", "Testnet"]);
    if app_settings.lock().unwrap().ltc_network.to_ascii_lowercase() == "testnet" {
        ltc_network.set_selected(1);
    }
    let litecoin_node = adw::EntryRow::builder().title("Esplora URL").build();
    litecoin_node.set_text(&app_settings.lock().unwrap().ltc_node);
    let ltc_incorrect_format =
        ui::error_label("Litecoin node must be an https Esplora URL, or empty for the network default. Plaintext http:// is only accepted for a node on this device.");
    ltc_group.add(&ltc_network);
    ltc_group.add(&litecoin_node);
    ltc_group.add(&ltc_incorrect_format);
    page.add(&ltc_group);

    // ---- Monero ----
    let xmr_group = ui::group_with_description(
        "Monero",
        "Monero has no address lookup: the wallet downloads blocks and scans them itself, so a node learns nothing about your account from a sync.",
    );
    let xmr_network = ui::combo_row("Network", &["Mainnet", "Stagenet"]);
    if xmr_chain::is_testnet(xmr_chain::parse_network(&app_settings.lock().unwrap().xmr_network)) {
        xmr_network.set_selected(1);
    }
    let monero_node = adw::EntryRow::builder().title("Daemon RPC URL").build();
    monero_node.set_text(&app_settings.lock().unwrap().xmr_node);
    let xmr_incorrect_format = ui::error_label(
        "Monero node must be an https daemon URL, or empty for the public defaults. Plaintext http:// is only accepted for a node on this device.",
    );
    // One height for every Monero account, applied on save and only when it changes:
    // moving it drops the scan cache so the next sync starts over from there.
    let xmr_restore_height = adw::EntryRow::builder().title("Restore height").build();
    {
        let settings = app_settings.lock().unwrap();
        if let Some(height) = settings.xmr_wallets.first().and_then(|w| w.restore_height) {
            xmr_restore_height.set_text(&height.to_string());
        }
    }
    let xmr_restore_hint = adw::ActionRow::builder()
        .title("Scanning")
        .subtitle("A wallet created here scans from its own creation. A restored phrase or imported key scans the last month unless a block height is given above. Lowering it rescans from there.")
        .build();
    xmr_restore_hint.add_css_class("property");
    let xmr_bad_height = ui::error_label("Restore height must be a block number, or empty.");
    // Applied only when edited: the row shows the first account's height, and a Save that
    // never touched it must not push that value onto every other account and rescan them.
    let xmr_height_as_shown = xmr_restore_height.text().to_string();
    xmr_group.add(&xmr_network);
    xmr_group.add(&monero_node);
    xmr_group.add(&xmr_incorrect_format);
    xmr_group.add(&xmr_restore_height);
    xmr_group.add(&xmr_restore_hint);
    xmr_group.add(&xmr_bad_height);
    page.add(&xmr_group);

    let save_group = adw::PreferencesGroup::new();
    let save_button = ui::primary_button("Save network settings");
    save_group.add(&save_button);
    page.add(&save_group);

    erc20.add.connect_clicked(clone!(
        #[strong] app_settings,
        #[weak(rename_to = token_contract)] erc20.entry,
        #[weak(rename_to = token_status)] erc20.status,
        move |_| {
            let contract = token_contract.text().to_string();
            if eth_chain::validate_address(&contract).is_err() {
                token_status.set_label("Token contract must be a 0x address.");
                token_status.set_visible(true);
                return;
            }
            token_status.set_label("Looking up token…");
            token_status.set_visible(true);
            let settings = app_settings.lock().unwrap();
            let node = settings.eth_node.clone();
            let network = settings.eth_network.clone();
            let infura = settings.infura_key.clone();
            drop(settings);
            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = eth_chain::fetch_token_metadata(&contract, &node, &network, &infura);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[strong] app_settings,
                    #[weak] token_status,
                    #[weak] token_contract,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(token) => {
                                app_settings.lock().unwrap().add_custom_token(CustomTokenRecord {
                                    symbol: token.symbol.clone(),
                                    name: token.name.clone(),
                                    address: token.address.clone(),
                                    decimals: token.decimals as i32,
                                    chain: "eth".to_string(),
                                });
                                let _ = app_settings.lock().unwrap().write_config();
                                token_status.set_label(&format!("Added {} ({})", token.symbol, token.address));
                                token_contract.set_text("");
                                ui::toast(&format!("Added {}", token.symbol));
                            }
                            Err(_) => {
                                token_status.set_label("Could not read that contract. Check the address and Ethereum RPC.");
                            }
                        }
                        token_status.set_visible(true);
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    spl.add.connect_clicked(clone!(
        #[strong] app_settings,
        #[weak(rename_to = spl_mint)] spl.entry,
        #[weak(rename_to = spl_status)] spl.status,
        move |_| {
            let mint = spl_mint.text().to_string();
            if sol_chain::validate_address(&mint).is_err() {
                spl_status.set_label("Mint must be a base58 Solana address.");
                spl_status.set_visible(true);
                return;
            }
            spl_status.set_label("Looking up token…");
            spl_status.set_visible(true);
            let settings = app_settings.lock().unwrap();
            let node = settings.sol_node.clone();
            let network = settings.sol_network.clone();
            drop(settings);
            let (sender, receiver) = crate::configuration::ui_channel::unbounded();
            thread::spawn(move || {
                let result = sol_chain::fetch_token_metadata(&mint, &node, &network);
                let _ = sender.send_blocking(result);
            });
            crate::configuration::ui_channel::attach(
                receiver,
                clone!(
                    #[strong] app_settings,
                    #[weak] spl_status,
                    #[weak] spl_mint,
                    #[upgrade_or]
                    ControlFlow::Break,
                    move |result| {
                        match result {
                            Ok(token) => {
                                app_settings.lock().unwrap().add_custom_token(CustomTokenRecord {
                                    symbol: token.symbol.clone(),
                                    name: token.name.clone(),
                                    address: token.address.clone(),
                                    decimals: token.decimals as i32,
                                    chain: "sol".to_string(),
                                });
                                let _ = app_settings.lock().unwrap().write_config();
                                spl_status.set_label(&format!("Added {} ({})", token.symbol, token.address));
                                spl_mint.set_text("");
                                ui::toast(&format!("Added {}", token.symbol));
                            }
                            Err(_) => {
                                spl_status.set_label("Could not read that mint. Check the address and Solana RPC.");
                            }
                        }
                        spl_status.set_visible(true);
                        ControlFlow::Break
                    }
                ),
            );
        }
    ));

    save_button.connect_clicked(move |_| {
        eth_incorrect_format.set_visible(false);
        btc_incorrect_format.set_visible(false);
        sol_incorrect_format.set_visible(false);
        ltc_incorrect_format.set_visible(false);
        xmr_incorrect_format.set_visible(false);
        xmr_bad_height.set_visible(false);
        thornode_incorrect_format.set_visible(false);
        let mut all_valid = true;
        if !etherscan_api_key.text().is_empty() {
            app_settings.lock().unwrap().etherscan_key = etherscan_api_key.text().to_string();
        }
        let eth_text = ethereum_node.text().to_string();
        if endpoint::validate(&eth_text, false).is_ok() {
            app_settings.lock().unwrap().eth_node = eth_text;
        } else {
            eth_incorrect_format.set_visible(true);
            all_valid = false;
        }
        if testnet.is_active() {
            app_settings.lock().unwrap().apply_test_networks(true);
            btc_network.set_selected(1);
            eth_network.set_selected(1);
            sol_network.set_selected(1);
            ltc_network.set_selected(1);
            xmr_network.set_selected(1);
        } else {
            let selected_eth = ETH_NETWORKS
                .get(eth_network.selected() as usize)
                .unwrap_or(&"mainnet");
            app_settings.lock().unwrap().apply_eth_network(selected_eth);
            let networks = ["bitcoin", "testnet"];
            let selected_network = networks
                .get(btc_network.selected() as usize)
                .unwrap_or(&"bitcoin");
            let _ = app_settings.lock().unwrap().apply_btc_network(selected_network);
            let sol_networks = ["mainnet", "devnet"];
            let selected_sol = sol_networks
                .get(sol_network.selected() as usize)
                .unwrap_or(&"mainnet");
            app_settings.lock().unwrap().apply_sol_network(selected_sol);
            let ltc_networks = ["litecoin", "testnet"];
            let selected_ltc = ltc_networks
                .get(ltc_network.selected() as usize)
                .unwrap_or(&"litecoin");
            app_settings.lock().unwrap().apply_ltc_network(selected_ltc);
            let xmr_networks = ["monero", "stagenet"];
            let selected_xmr = xmr_networks
                .get(xmr_network.selected() as usize)
                .unwrap_or(&"monero");
            app_settings.lock().unwrap().apply_xmr_network(selected_xmr);
        }
        let btc_text = btc_node.text().to_string();
        if endpoint::validate(&btc_text, true).is_ok() {
            app_settings.lock().unwrap().btc_node = btc_text;
        } else {
            btc_incorrect_format.set_visible(true);
            all_valid = false;
        }
        let sol_text = solana_node.text().to_string();
        if endpoint::validate(&sol_text, false).is_ok() {
            app_settings.lock().unwrap().sol_node = sol_text;
        } else {
            sol_incorrect_format.set_visible(true);
            all_valid = false;
        }
        let thornode_text = thornode.text().to_string();
        if endpoint::validate(&thornode_text, false).is_ok() {
            app_settings.lock().unwrap().thornode_url = thornode_text;
        } else {
            thornode_incorrect_format.set_visible(true);
            all_valid = false;
        }
        let ltc_text = litecoin_node.text().to_string();
        if endpoint::validate(&ltc_text, false).is_ok() {
            app_settings.lock().unwrap().ltc_node = ltc_text;
        } else {
            ltc_incorrect_format.set_visible(true);
            all_valid = false;
        }
        let xmr_text = monero_node.text().to_string();
        if endpoint::validate(&xmr_text, false).is_ok() {
            app_settings.lock().unwrap().xmr_node = xmr_text;
        } else {
            xmr_incorrect_format.set_visible(true);
            all_valid = false;
        }
        let height_text = xmr_restore_height.text().to_string();
        let height_edited = height_text.trim() != xmr_height_as_shown.trim();
        let height = if height_text.trim().is_empty() {
            Ok(None)
        } else {
            height_text.trim().replace(',', "").parse::<u64>().map(Some)
        };
        match height {
            _ if !height_edited => {}
            Ok(height) => {
                let mut settings = app_settings.lock().unwrap();
                let network = settings.xmr_network.clone();
                for wallet in &mut settings.xmr_wallets {
                    if wallet.restore_height == height {
                        continue;
                    }
                    wallet.restore_height = height;
                    // Forget what was scanned so the next sync starts from the new height.
                    // Raising it is allowed too: that is how a user who set it too low stops a
                    // needless scan of years they know were empty.
                    if let (Some(spend), Some(view), Some(address)) =
                        (&wallet.private_key, &wallet.private_view_key, &wallet.address)
                    {
                        let _ = xmr_chain::reset_scan_cache(spend, view, address, &network);
                    }
                }
            }
            Err(_) => {
                xmr_bad_height.set_visible(true);
                all_valid = false;
            }
        }
        if !infura_key.text().is_empty() {
            app_settings.lock().unwrap().infura_key = infura_key.text().to_string();
        }
        let _ = app_settings.lock().unwrap().write_config();
        // Saving used to be completely silent, so there was no way to tell whether the
        // tap registered — or that one field had been rejected while the rest went through.
        ui::toast(if all_valid {
            "Network settings saved."
        } else {
            "Saved, but some fields were rejected. See the messages above."
        });
    });

    NetworkWidgets { _keep: () }
}

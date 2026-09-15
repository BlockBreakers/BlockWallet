use adw::prelude::*;
use glib::clone;
use gtk::{Image, Orientation};
use pango::WrapMode;
use std::sync::{Arc, Mutex};

use crate::configuration::application_settings::*;
use crate::configuration::clipboard;
use crate::configuration::seed;
use crate::configuration::wallet_display;
use crate::currencies::btc;
use crate::currencies::btc::BitcoinWallet;
use crate::currencies::eth;
use crate::currencies::eth::EthereumWallet;
use crate::currencies::ltc;
use crate::currencies::ltc::LitecoinWallet;
use crate::currencies::sol;
use crate::currencies::sol::SolanaWallet;
use crate::currencies::xmr::MoneroWallet;
use crate::views::ui;

pub fn wallet_view(app_settings: Arc<Mutex<ApplicationSettings>>) -> (gtk::Box, Arc<Mutex<ApplicationSettings>>) {
    let btc_wallets = app_settings.lock().unwrap().btc_wallets.clone();
    let eth_wallets = app_settings.lock().unwrap().eth_wallets.clone();
    let sol_wallets = app_settings.lock().unwrap().sol_wallets.clone();
    let ltc_wallets = app_settings.lock().unwrap().ltc_wallets.clone();
    let xmr_wallets = app_settings.lock().unwrap().xmr_wallets.clone();

    let scrollable_box = ui::page_body(12);

    scrollable_box.append(&ui::notice(
        "One recovery phrase backs every account here. Addresses are safe to share; the phrase never is.",
    ));

    // All five chains are shown at once in titled groups. The old screen hid every
    // account behind a toggle button per chain, so a fresh wallet opened on four
    // identical grey buttons and no information at all.
    let btc_group = chain_group("btc", &btc_wallets.len().to_string());
    let eth_group = chain_group("eth", &eth_wallets.len().to_string());
    let sol_group = chain_group("sol", &sol_wallets.len().to_string());
    let ltc_group = chain_group("ltc", &ltc_wallets.len().to_string());
    let xmr_group = chain_group("xmr", &xmr_wallets.len().to_string());

    for wallet in &btc_wallets {
        add_btc_wallet(&btc_group, wallet, app_settings.clone());
    }
    for wallet in &eth_wallets {
        add_eth_wallet(&eth_group, wallet, app_settings.clone());
    }
    for wallet in &sol_wallets {
        add_sol_wallet(&sol_group, wallet, app_settings.clone());
    }
    for wallet in &ltc_wallets {
        add_ltc_wallet(&ltc_group, wallet, app_settings.clone());
    }
    for wallet in &xmr_wallets {
        add_xmr_wallet(&xmr_group, wallet, app_settings.clone());
    }

    scrollable_box.append(&btc_group);
    scrollable_box.append(&eth_group);
    scrollable_box.append(&sol_group);
    scrollable_box.append(&ltc_group);
    scrollable_box.append(&xmr_group);

    let add_group = adw::PreferencesGroup::new();
    let add_wallet_button = ui::icon_button("Add account", "list-add-symbolic");
    add_group.add(&add_wallet_button);
    scrollable_box.append(&add_group);

    let scrollable_container = ui::scroller(&scrollable_box);
    let wallet_box = ui::vbox(0);

    let new_wallet_box = new_wallet_box(
        app_settings.clone(),
        Arc::new(Mutex::new(btc_group.clone())),
        Arc::new(Mutex::new(eth_group.clone())),
        Arc::new(Mutex::new(sol_group.clone())),
        Arc::new(Mutex::new(ltc_group.clone())),
        Arc::new(Mutex::new(xmr_group.clone())),
        Arc::new(Mutex::new(scrollable_container.clone())),
    );
    wallet_box.append(&scrollable_container);
    wallet_box.append(&new_wallet_box);

    add_wallet_button.connect_clicked(move |_| {
        new_wallet_box.set_visible(true);
        scrollable_container.set_visible(false);
    });

    (wallet_box, app_settings)
}

fn chain_group(chain: &str, count: &str) -> adw::PreferencesGroup {
    let plural = if count == "1" { "account" } else { "accounts" };
    ui::group_with_description(
        ui::chain_display_name(chain),
        &format!("{count} {plural}"),
    )
}

fn append_account_card(
    parent: &adw::PreferencesGroup,
    name: Option<&str>,
    address: Option<&str>,
    chain: &str,
    qr_box: gtk::Box,
    app_settings: Arc<Mutex<ApplicationSettings>>,
) {
    let lines = wallet_display::default_visible_lines(name, address);
    let title = lines[0].clone();
    let address_text = address.unwrap_or("").to_string();
    let subtitle = if address_text.is_empty() {
        String::new()
    } else {
        wallet_display::truncate_address(&address_text)
    };

    let address_label = ui::mono_address(&lines[1]);
    address_label.set_margin_start(12);
    address_label.set_margin_end(12);
    address_label.set_margin_top(8);

    let copy_address = ui::icon_button("Copy address", "edit-copy-symbolic");
    copy_address.set_margin_start(12);
    copy_address.set_margin_end(12);
    let address_for_copy = address_text.clone();
    copy_address.connect_clicked(move |_| {
        if !address_for_copy.is_empty() {
            clipboard::copy_text(&address_for_copy);
            ui::toast("Address copied. The clipboard clears shortly.");
        }
    });

    let qr_button = ui::icon_button("Show QR code", "scanner-symbolic");
    qr_button.set_margin_start(12);
    qr_button.set_margin_end(12);
    qr_box.set_halign(gtk::Align::Center);
    let qr_box_toggle = qr_box.clone();
    qr_button.connect_clicked(move |button| {
        let show = !qr_box_toggle.is_visible();
        qr_box_toggle.set_visible(show);
        if let Some(content) = button.child().and_then(|c| c.downcast::<adw::ButtonContent>().ok()) {
            content.set_label(if show { "Hide QR code" } else { "Show QR code" });
        }
    });

    let expander = adw::ExpanderRow::builder()
        .title(&title)
        .subtitle(&subtitle)
        .css_classes(["wallet-expander"])
        .build();
    // The real coin logo, same as Home and Assets, falling back to a coloured monogram
    // where no PNG is bundled. The old code passed "BTC"/"ETH" as an icon-theme name,
    // which resolves to nothing and rendered as the broken-image glyph.
    let symbol = chain_symbol(chain);
    let logo = crate::configuration::paths::token_icon_path(symbol);
    expander.add_prefix(&ui::coin_mark(&logo, symbol, chain, 32));

    expander.add_row(&address_label);
    expander.add_row(&copy_address);
    expander.add_row(&qr_box);
    expander.add_row(&qr_button);
    expander.add_row(&reveal_box(address_text, app_settings));

    parent.add(&expander);
}

fn chain_symbol(chain: &str) -> &'static str {
    match chain {
        "btc" => "BTC",
        "sol" => "SOL",
        "ltc" => "LTC",
        "xmr" => "XMR",
        _ => "ETH",
    }
}

fn reveal_box(address: String, app_settings: Arc<Mutex<ApplicationSettings>>) -> gtk::Box {
    let box_ = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(8)
        .build();

    let reveal_button = ui::icon_button("Reveal backup details", "view-reveal-symbolic");
    let password = gtk::PasswordEntry::builder()
        .placeholder_text("Password")
        .show_peek_icon(true)
        .hexpand(true)
        .visible(false)
        .build();
    let unlock_details = ui::button("Show details");
    unlock_details.set_visible(false);
    let error = ui::error_label("Password incorrect.");
    // Anyone who sees this text owns the funds. It gets the loudest treatment in the app.
    let warning = gtk::Label::builder()
        .label("Anyone with this phrase can spend these funds. Never type it into a website or share it.")
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .xalign(0.0)
        .visible(false)
        .css_classes(["danger-note"])
        .build();
    let phrase_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(WrapMode::WordChar)
        .selectable(true)
        .visible(false)
        .css_classes(["seed-chip"])
        .build();
    let key_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(WrapMode::Char)
        .selectable(true)
        .visible(false)
        .css_classes(["seed-chip"])
        .build();
    let copy_phrase = ui::icon_button("Copy recovery phrase", "edit-copy-symbolic");
    copy_phrase.set_visible(false);
    let hide_button = ui::icon_button("Hide details", "view-conceal-symbolic");
    hide_button.set_visible(false);

    box_.append(&reveal_button);
    box_.append(&password);
    box_.append(&unlock_details);
    box_.append(&error);
    box_.append(&warning);
    box_.append(&phrase_label);
    box_.append(&key_label);
    box_.append(&copy_phrase);
    box_.append(&hide_button);

    reveal_button.connect_clicked(clone!(
        #[weak] password,
        #[weak] unlock_details,
        #[weak] error,
        move |button| {
            error.set_visible(false);
            password.set_visible(true);
            unlock_details.set_visible(true);
            button.set_visible(false);
        }
    ));

    unlock_details.connect_clicked(clone!(
        #[weak] password,
        #[weak] error,
        #[weak] warning,
        #[weak] phrase_label,
        #[weak] key_label,
        #[weak] copy_phrase,
        #[weak] hide_button,
        #[weak] unlock_details,
        #[strong] app_settings,
        #[strong] address,
        move |_| {
            let entered = password.text().to_string();
            password.set_text("");
            if !app_settings.lock().unwrap().verify_password(&entered) {
                error.set_visible(true);
                return;
            }
            error.set_visible(false);
            let (mnemonic, private_key) = app_settings.lock().unwrap().secrets_for_address(&address);
            warning.set_visible(true);
            if let Some(phrase) = mnemonic.filter(|value| !value.is_empty()) {
                phrase_label.set_label(&format!("Recovery phrase: {phrase}"));
                phrase_label.set_visible(true);
                copy_phrase.set_visible(true);
            }
            if let Some(key) = private_key.filter(|value| !value.is_empty()) {
                key_label.set_label(&format!("Private key: {key}"));
                key_label.set_visible(true);
            }
            hide_button.set_visible(true);
            unlock_details.set_visible(false);
            password.set_visible(false);
        }
    ));

    copy_phrase.connect_clicked(clone!(
        #[weak] phrase_label,
        move |_| {
            let text = phrase_label.label().to_string();
            let phrase = text
                .strip_prefix("Recovery phrase: ")
                .unwrap_or(text.as_str())
                .to_string();
            if !phrase.is_empty() {
                clipboard::copy_text_timed(&phrase);
                ui::toast("Recovery phrase copied. The clipboard clears shortly.");
            }
        }
    ));

    hide_button.connect_clicked(clone!(
        #[weak] phrase_label,
        #[weak] key_label,
        #[weak] copy_phrase,
        #[weak] hide_button,
        #[weak] reveal_button,
        #[weak] password,
        #[weak] error,
        #[weak] warning,
        move |_| {
            phrase_label.set_label("");
            key_label.set_label("");
            phrase_label.set_visible(false);
            key_label.set_visible(false);
            copy_phrase.set_visible(false);
            hide_button.set_visible(false);
            warning.set_visible(false);
            password.set_text("");
            password.set_visible(false);
            error.set_visible(false);
            reveal_button.set_visible(true);
        }
    ));

    box_
}

fn add_btc_wallet(
    btc_box: &adw::PreferencesGroup,
    btcw: &BitcoinWallet,
    app_settings: Arc<Mutex<ApplicationSettings>>,
) {
    append_account_card(
        btc_box,
        btcw.wallet_name.as_deref(),
        btcw.address.as_deref(),
        "btc",
        btc_qr_box(btcw),
        app_settings,
    );
}

fn add_eth_wallet(
    eth_box: &adw::PreferencesGroup,
    ethw: &EthereumWallet,
    app_settings: Arc<Mutex<ApplicationSettings>>,
) {
    append_account_card(
        eth_box,
        ethw.wallet_name.as_deref(),
        ethw.address.as_deref(),
        "eth",
        eth_qr_box(ethw),
        app_settings,
    );
}

fn add_sol_wallet(
    sol_box: &adw::PreferencesGroup,
    solw: &SolanaWallet,
    app_settings: Arc<Mutex<ApplicationSettings>>,
) {
    append_account_card(
        sol_box,
        solw.wallet_name.as_deref(),
        solw.address.as_deref(),
        "sol",
        sol_qr_box(solw),
        app_settings,
    );
}

fn add_ltc_wallet(
    ltc_box: &adw::PreferencesGroup,
    ltcw: &LitecoinWallet,
    app_settings: Arc<Mutex<ApplicationSettings>>,
) {
    append_account_card(
        ltc_box,
        ltcw.wallet_name.as_deref(),
        ltcw.address.as_deref(),
        "ltc",
        ltc_qr_box(ltcw),
        app_settings,
    );
}

fn add_xmr_wallet(
    xmr_box: &adw::PreferencesGroup,
    xmrw: &MoneroWallet,
    app_settings: Arc<Mutex<ApplicationSettings>>,
) {
    append_account_card(
        xmr_box,
        xmrw.wallet_name.as_deref(),
        xmrw.address.as_deref(),
        "xmr",
        xmr_qr_box(xmrw),
        app_settings,
    );
}

pub fn btc_qr_box(btcw: &BitcoinWallet) -> gtk::Box {
    qr_box_from_texture(btcw.generate_qr_address().ok())
}

pub fn eth_qr_box(ethw: &EthereumWallet) -> gtk::Box {
    qr_box_from_texture(ethw.generate_qr_address().ok())
}

pub fn sol_qr_box(solw: &SolanaWallet) -> gtk::Box {
    qr_box_from_texture(solw.generate_qr_address().ok())
}

pub fn ltc_qr_box(ltcw: &LitecoinWallet) -> gtk::Box {
    qr_box_from_texture(ltcw.generate_qr_address().ok())
}

pub fn xmr_qr_box(xmrw: &MoneroWallet) -> gtk::Box {
    qr_box_from_texture(xmrw.generate_qr_address().ok())
}

fn qr_box_from_texture(texture: Option<gtk::gdk::Texture>) -> gtk::Box {
    let qr_box = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .visible(false)
        .halign(gtk::Align::Center)
        .margin_start(12)
        .margin_end(12)
        .build();
    if let Some(qr_code) = texture {
        let qr_image = Image::from_paintable(Some(&qr_code));
        qr_image.set_pixel_size(170);
        // White frame: the QR is dark modules on a light field and will not scan against
        // a dark theme background.
        let frame = gtk::Box::new(Orientation::Vertical, 0);
        frame.add_css_class("qr-frame");
        frame.set_halign(gtk::Align::Center);
        frame.append(&qr_image);
        qr_box.append(&frame);
    }
    qr_box
}

fn new_wallet_box(
    app_settings: Arc<Mutex<ApplicationSettings>>,
    btc_box: Arc<Mutex<adw::PreferencesGroup>>,
    eth_box: Arc<Mutex<adw::PreferencesGroup>>,
    sol_box: Arc<Mutex<adw::PreferencesGroup>>,
    ltc_box: Arc<Mutex<adw::PreferencesGroup>>,
    xmr_box: Arc<Mutex<adw::PreferencesGroup>>,
    scrollable_container: Arc<Mutex<gtk::ScrolledWindow>>,
) -> gtk::Box {
    let content = ui::page_body(14);

    let back_button = ui::icon_button("Back to accounts", "go-previous-symbolic");
    content.append(&back_button);

    content.append(&ui::notice(
        "Add another account from this recovery phrase, or import an existing key. The phrase is never shown here.",
    ));

    let tokens = ["Bitcoin", "Ethereum", "Solana", "Litecoin", "Monero"];

    // ---- derive from the existing seed ----
    let create_group = ui::group_with_description(
        "New account",
        "Derived from the recovery phrase already in this wallet.",
    );
    let token_selector = ui::combo_row("Chain", &tokens);
    let wallet_name = ui::entry_row("Account name");
    create_group.add(&token_selector);
    create_group.add(&wallet_name);
    let create_wallet_button = ui::primary_button("Add from recovery phrase");
    create_group.add(&create_wallet_button);
    let create_error = ui::error_label("Could not add that account.");
    create_group.add(&create_error);
    content.append(&create_group);

    // ---- import an outside key ----
    let import_group = ui::group_with_description(
        "Import account",
        "Bring in a key that was created elsewhere. It is stored in the same encrypted file.",
    );
    let import_token_selector = ui::combo_row("Chain", &tokens);
    let import_wallet_name = ui::entry_row("Account name");
    let import_secret = ui::password_row("WIF, private key or phrase");
    // Monero only. A Monero account has to scan the chain for its own payments, and a key
    // made elsewhere gives no hint where to start; without this it scans the last month
    // and Settings has to be visited for anything older.
    let import_restore_height = ui::entry_row("Restore height (block the account was created at)");
    import_restore_height.set_visible(false);
    import_group.add(&import_token_selector);
    import_group.add(&import_wallet_name);
    import_group.add(&import_secret);
    import_group.add(&import_restore_height);
    import_token_selector.connect_selected_notify(clone!(
        #[weak] import_restore_height,
        move |selector| {
            import_restore_height.set_visible(selector.selected() == 4);
        }
    ));
    let import_wallet_button = ui::button("Import account");
    import_wallet_button.add_css_class("pill-button");
    import_group.add(&import_wallet_button);
    let import_error = ui::error_label("Could not import that key.");
    import_group.add(&import_error);
    content.append(&import_group);

    let new_wallet_box = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .visible(false)
        .build();
    new_wallet_box.append(&ui::scroller(&content));

    let form = new_wallet_box.clone();
    back_button.connect_clicked(clone!(
        #[strong] scrollable_container,
        move |_| {
            form.set_visible(false);
            scrollable_container.lock().unwrap().set_visible(true);
        }
    ));

    create_wallet_button.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] eth_box,
        #[strong] sol_box,
        #[strong] scrollable_container,
        #[weak] token_selector,
        #[weak] wallet_name,
        #[weak] create_error,
        #[weak] new_wallet_box,
        move |_| {
            create_error.set_visible(false);
            let name = wallet_name.text().to_string();
            let mut settings = app_settings.lock().unwrap();
            let Some(phrase) = settings.mnemonic.clone() else {
                create_error.set_label("Unlock the wallet before adding an account.");
                create_error.set_visible(true);
                return;
            };
            let passphrase = settings.seed_passphrase.clone().unwrap_or_default();
            if token_selector.selected() == 0 {
                create_error.set_label("This seed already has a Bitcoin account. Import a WIF to add a different key.");
                create_error.set_visible(true);
                return;
            }
            if token_selector.selected() == 3 {
                create_error.set_label("This seed already has a Litecoin account. Import a WIF to add a different key.");
                create_error.set_visible(true);
                return;
            }
            if token_selector.selected() == 4 {
                create_error.set_label("This seed already has a Monero account. Import a spend key to add a different one.");
                create_error.set_visible(true);
                return;
            }
            if token_selector.selected() == 2 {
                // Extra Solana accounts increment the hardened account index (m/44'/501'/n'/0'),
                // matching Phantom/Solflare's convention, since ed25519 SLIP-10 requires every
                // component to be hardened (no non-hardened /0/n suffix like the ETH path uses).
                let path = format!("m/44'/501'/{}'/0'", settings.sol_wallets.len());
                let mut solw = match SolanaWallet::from_mnemonic(&phrase, &path, &passphrase) {
                    Ok(wallet) => wallet,
                    Err(_) => {
                        create_error.set_visible(true);
                        return;
                    }
                };
                if !name.is_empty() {
                    solw.set_wallet_name(name);
                }
                add_sol_wallet(&sol_box.lock().unwrap(), &solw, app_settings.clone());
                settings.sol_wallets.push(solw);
                let _ = settings.write_config();
                new_wallet_box.set_visible(false);
                scrollable_container.lock().unwrap().set_visible(true);
                return;
            }
            let path = format!("m/44'/60'/0'/0/{}", settings.eth_wallets.len());
            let mut ethw = match seed::ethereum_from_seed(&phrase, &path, &passphrase, &name) {
                Ok(wallet) => wallet,
                Err(_) => {
                    create_error.set_visible(true);
                    return;
                }
            };
            if !name.is_empty() {
                ethw.set_wallet_name(name);
            }
            add_eth_wallet(&eth_box.lock().unwrap(), &ethw, app_settings.clone());
            settings.eth_wallets.push(ethw);
            let _ = settings.write_config();
            new_wallet_box.set_visible(false);
            scrollable_container.lock().unwrap().set_visible(true);
        }
    ));

    import_wallet_button.connect_clicked(clone!(
        #[strong] app_settings,
        #[strong] btc_box,
        #[strong] eth_box,
        #[strong] sol_box,
        #[strong] ltc_box,
        #[strong] xmr_box,
        #[strong] scrollable_container,
        #[weak] import_token_selector,
        #[weak] import_wallet_name,
        #[weak] import_secret,
        #[weak] import_restore_height,
        #[weak] import_error,
        #[weak] new_wallet_box,
        move |_| {
            import_error.set_visible(false);
            let secret = import_secret.text().to_string();
            import_secret.set_text("");
            let name = import_wallet_name.text().to_string();
            let mut settings = app_settings.lock().unwrap();
            if import_token_selector.selected() == 4 {
                let height_text = import_restore_height.text().to_string();
                let restore_height = if height_text.trim().is_empty() {
                    None
                } else {
                    match height_text.trim().replace(',', "").parse::<u64>() {
                        Ok(height) => Some(height),
                        Err(_) => {
                            import_error.set_label("Restore height must be a block number, or empty to scan the last month.");
                            import_error.set_visible(true);
                            return;
                        }
                    }
                };
                let network = crate::currencies::xmr_chain::parse_network(&settings.xmr_network);
                let word_count = secret.split_whitespace().count();
                let mut xmrw = if word_count == 25 {
                    // Monero's own seed format encodes the spend key in a 1626-word list this
                    // wallet does not carry. The spend key itself is what that seed holds, and
                    // every Monero wallet will show it.
                    import_error.set_label("Monero 25-word seeds are not supported. Import the private spend key instead: any Monero wallet shows it under its keys.");
                    import_error.set_visible(true);
                    return;
                } else if word_count > 1 {
                    match MoneroWallet::from_mnemonic_on(&secret, "", network) {
                        Ok(wallet) => wallet,
                        Err(_) => {
                            import_error.set_visible(true);
                            return;
                        }
                    }
                } else {
                    match MoneroWallet::from_private_key_on(&secret, network, restore_height) {
                        Ok(wallet) => wallet,
                        Err(_) => {
                            import_error.set_label("Could not import that key. A Monero spend key is 64 hex characters.");
                            import_error.set_visible(true);
                            return;
                        }
                    }
                };
                xmrw.restore_height = restore_height;
                if !name.is_empty() {
                    xmrw.set_wallet_name(name);
                }
                add_xmr_wallet(&xmr_box.lock().unwrap(), &xmrw, app_settings.clone());
                settings.xmr_wallets.push(xmrw);
                import_restore_height.set_text("");
            } else if import_token_selector.selected() == 0 {
                let mut btcw = match btc::generate_from_private_key(&secret) {
                    Some(wallet) => wallet,
                    None => {
                        import_error.set_visible(true);
                        return;
                    }
                };
                if !name.is_empty() {
                    btcw.set_wallet_name(name);
                }
                add_btc_wallet(&btc_box.lock().unwrap(), &btcw, app_settings.clone());
                settings.btc_wallets.push(btcw);
            } else if import_token_selector.selected() == 2 {
                let mut solw = if secret.contains(' ') {
                    match sol::generate_from_mnemonic(&secret, seed::SOL_PATH) {
                        Some(wallet) => wallet,
                        None => {
                            import_error.set_visible(true);
                            return;
                        }
                    }
                } else {
                    match sol::generate_from_private_key(&secret) {
                        Some(wallet) => wallet,
                        None => {
                            import_error.set_visible(true);
                            return;
                        }
                    }
                };
                if !name.is_empty() {
                    solw.set_wallet_name(name);
                }
                add_sol_wallet(&sol_box.lock().unwrap(), &solw, app_settings.clone());
                settings.sol_wallets.push(solw);
            } else if import_token_selector.selected() == 3 {
                let mut ltcw = match ltc::generate_from_private_key(&secret) {
                    Some(wallet) => wallet,
                    None => {
                        import_error.set_visible(true);
                        return;
                    }
                };
                if !name.is_empty() {
                    ltcw.set_wallet_name(name);
                }
                add_ltc_wallet(&ltc_box.lock().unwrap(), &ltcw, app_settings.clone());
                settings.ltc_wallets.push(ltcw);
            } else if secret.contains(' ') {
                let mut ethw = match eth::generate_from_mnemonic(&secret, seed::ETH_PATH) {
                    Some(wallet) => wallet,
                    None => {
                        import_error.set_visible(true);
                        return;
                    }
                };
                if !name.is_empty() {
                    ethw.set_wallet_name(name);
                }
                add_eth_wallet(&eth_box.lock().unwrap(), &ethw, app_settings.clone());
                settings.eth_wallets.push(ethw);
            } else {
                let mut ethw = match eth::generate_from_private_key(&secret) {
                    Some(wallet) => wallet,
                    None => {
                        import_error.set_visible(true);
                        return;
                    }
                };
                if !name.is_empty() {
                    ethw.set_wallet_name(name);
                }
                add_eth_wallet(&eth_box.lock().unwrap(), &ethw, app_settings.clone());
                settings.eth_wallets.push(ethw);
            }
            let _ = settings.write_config();
            new_wallet_box.set_visible(false);
            scrollable_container.lock().unwrap().set_visible(true);
        }
    ));

    new_wallet_box
}

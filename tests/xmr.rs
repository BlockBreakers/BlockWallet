use block_wallet::currencies::xmr::{generate_from_mnemonic, generate_from_private_key, generate_xmr_hd_wallet};
use block_wallet::currencies::xmr_chain::{
    format_xmr, parse_network, validate_address, xmr_to_piconero, SyncAccount, XmrNetwork,
};

const ABANDON: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[test]
fn generate_from_known_mnemonic() {
    let wallet = generate_from_mnemonic(ABANDON, "").unwrap();
    let address = wallet.address.clone().unwrap();
    assert!(address.starts_with('4'), "{address}");
    assert_eq!(address.len(), 95);
    assert!(wallet.private_key.is_some());
    assert!(wallet.private_view_key.is_some());
    assert!(wallet.mnemonic.is_some());
}

#[test]
fn generate_from_spend_key_roundtrips_address() {
    let generated = generate_xmr_hd_wallet().unwrap();
    let key = generated.private_key.clone().unwrap();
    let wallet = generate_from_private_key(&key, None).unwrap();
    assert_eq!(wallet.address, generated.address);
    assert_eq!(wallet.private_view_key, generated.private_view_key);
    assert!(wallet.public_key.is_some());
}

#[test]
fn xmr_chain_validates_and_parses_without_rpc() {
    assert_eq!(parse_network("stagenet"), XmrNetwork::Stagenet);
    assert_eq!(parse_network(""), XmrNetwork::Mainnet);
    assert_eq!(xmr_to_piconero("1.25").unwrap(), 1_250_000_000_000);
    assert_eq!(format_xmr(1_250_000_000_000), "1.25");
    let wallet = generate_xmr_hd_wallet().unwrap();
    let address = wallet.address.clone().unwrap();
    assert!(validate_address(&address, XmrNetwork::Mainnet).is_ok());
    assert!(validate_address(&address, XmrNetwork::Stagenet).is_err());
    assert!(validate_address("not-a-monero-address", XmrNetwork::Mainnet).is_err());
}

/// Switching to test networks must re-encode the Monero address for stagenet, not just
/// relabel it. Same regression guard as the Litecoin one.
#[test]
fn switching_networks_reencodes_the_monero_address() {
    use block_wallet::configuration::initialization;
    use block_wallet::ApplicationSettings;

    let mut settings = ApplicationSettings::new(initialization::load_tokens());
    settings.mnemonic = Some(ABANDON.to_string());
    settings.seed_passphrase = Some(String::new());
    settings.xmr_wallets = vec![block_wallet::configuration::seed::monero_from_seed(ABANDON, "", "Monero").unwrap()];

    let mainnet = settings.xmr_wallets[0].address.clone().unwrap();
    assert!(mainnet.starts_with('4'), "{mainnet}");

    settings.apply_xmr_network("stagenet");
    let stagenet = settings.xmr_wallets[0].address.clone().unwrap();
    assert!(stagenet.starts_with('5'), "stagenet address should carry the stagenet prefix, got {stagenet}");
    assert_ne!(mainnet, stagenet);
    assert_eq!(settings.xmr_network, "stagenet");

    settings.apply_xmr_network("monero");
    assert_eq!(settings.xmr_wallets[0].address.clone().unwrap(), mainnet);
}

fn sync_account_for(wallet: &block_wallet::currencies::xmr::MoneroWallet, restore_height: Option<u64>) -> SyncAccount {
    SyncAccount {
        spend_hex: wallet.private_key.clone().unwrap(),
        view_hex: wallet.private_view_key.clone().unwrap(),
        address: wallet.address.clone().unwrap(),
        restore_height,
        birthday: None,
    }
}

/// Talks to a public node. Run with `cargo test --test xmr -- --ignored`.
///
/// The account is the well-known "abandon" phrase, so nothing is found; what this proves
/// is that the transport, the block download, the scanner and the cache all work against
/// a real daemon, on both networks.
#[test]
#[ignore]
fn scans_recent_blocks_on_a_public_node() {
    use block_wallet::currencies::xmr_chain::{reset_scan_cache, sync_account};

    let root = std::env::temp_dir().join(format!("blockwallet-xmr-live-{}", std::process::id()));
    std::env::set_var("BLOCKWALLET_HOME", &root);

    for network in ["monero", "stagenet"] {
        let wallet = block_wallet::currencies::xmr::MoneroWallet::from_mnemonic_on(ABANDON, "", parse_network(network)).unwrap();
        // First pass: a restore height past the tip is clamped to the tip, so this learns the
        // chain height without scanning. Then rescan the last few dozen blocks from it.
        let account = sync_account_for(&wallet, Some(u64::MAX));
        reset_scan_cache(&account.spend_hex, &account.view_hex, &account.address, network).unwrap();
        let mut labels = Vec::new();
        let first = sync_account(&account, "", network, &mut |label| {
            labels.push(label.to_string());
            true
        })
        .unwrap();
        assert!(!first.offline, "{network}: node should be reachable");
        assert!(first.chain_height > 1_000_000, "{network}: height {}", first.chain_height);

        let start = first.chain_height - 40;
        let account = sync_account_for(&wallet, Some(start));
        reset_scan_cache(&account.spend_hex, &account.view_hex, &account.address, network).unwrap();
        let state = sync_account(&account, "", network, &mut |label| {
            labels.push(label.to_string());
            true
        })
        .unwrap();
        assert!(!state.offline);
        assert_eq!(state.scanned_height, state.chain_height, "{network}: scanned to the tip");
        assert_eq!(state.unlocked_piconero, 0);
        assert!(labels.iter().any(|l| l.starts_with("Syncing…")), "{labels:?}");

        // A second sync is incremental: nothing to redo, still at the tip.
        let again = sync_account(&account, "", network, &mut |_| true).unwrap();
        assert!(again.scanned_height >= state.scanned_height);
        assert_eq!(again.balance_display(), "0.0 XMR");
    }

    std::env::remove_var("BLOCKWALLET_HOME");
    let _ = std::fs::remove_dir_all(root);
}

/// The stagenet faucet at stagenet-faucet.xmr-tw.org paid the "abandon" phrase's stagenet
/// address on 15 September 2026, at block 2208276 or the one after. Scanning from just before
/// that must find the payment, and a second sync must not find it twice.
///
/// Run with `cargo test --test xmr -- --ignored`. Depends on a public stagenet node.
#[test]
#[ignore]
fn finds_the_faucet_payment_on_stagenet() {
    use block_wallet::currencies::xmr_chain::{reset_scan_cache, sync_account};

    const FAUCET_TX: &str = "789c37ccda26d57ebbec2b902ca61555d2aedfea30da8eb5bf51509ce24c1ce4";
    let root = std::env::temp_dir().join(format!("blockwallet-xmr-faucet-{}", std::process::id()));
    std::env::set_var("BLOCKWALLET_HOME", &root);

    let wallet = block_wallet::currencies::xmr::MoneroWallet::from_mnemonic_on(ABANDON, "", XmrNetwork::Stagenet).unwrap();
    let account = sync_account_for(&wallet, Some(2_208_270));
    reset_scan_cache(&account.spend_hex, &account.view_hex, &account.address, "stagenet").unwrap();

    let state = sync_account(&account, "", "stagenet", &mut |_| true).unwrap();
    assert!(!state.offline);
    let payment = state
        .history
        .iter()
        .find(|item| item.txid == FAUCET_TX)
        .unwrap_or_else(|| panic!("faucet payment not found in {:?}", state.history));
    assert!(payment.amount_piconero > 0, "{payment:?}");
    assert!(state.unlocked_piconero + state.locked_piconero >= payment.amount_piconero as u64);

    let again = sync_account(&account, "", "stagenet", &mut |_| true).unwrap();
    assert_eq!(again.history.iter().filter(|item| item.txid == FAUCET_TX).count(), 1, "counted once");
    assert_eq!(
        again.unlocked_piconero + again.locked_piconero,
        state.unlocked_piconero + state.locked_piconero,
        "an incremental sync must not double count"
    );
    println!("faucet payment: {} XMR, balance {}", format_xmr(payment.amount_piconero as u64), again.balance_display());

    std::env::remove_var("BLOCKWALLET_HOME");
    let _ = std::fs::remove_dir_all(root);
}

/// Spends stagenet coins for real: from the "abandon" account to the same phrase with the
/// BIP39 passphrase `blockwallet`. Needs the faucet output above to be past its ten-block
/// lock, and leaves a transaction on stagenet each time it runs.
///
/// Run with `cargo test --test xmr sends -- --ignored --nocapture`. Depends on a public
/// stagenet node, and on the sender still holding unlocked stagenet coins.
#[test]
#[ignore]
fn sends_on_stagenet_and_the_recipient_sees_it() {
    use block_wallet::currencies::xmr_chain::{prepare_send, reset_scan_cache, sign_and_broadcast, sync_account};

    let root = std::env::temp_dir().join(format!("blockwallet-xmr-send-{}", std::process::id()));
    std::env::set_var("BLOCKWALLET_HOME", &root);

    let sender_wallet = block_wallet::currencies::xmr::MoneroWallet::from_mnemonic_on(ABANDON, "", XmrNetwork::Stagenet).unwrap();
    let recipient_wallet =
        block_wallet::currencies::xmr::MoneroWallet::from_mnemonic_on(ABANDON, "blockwallet", XmrNetwork::Stagenet).unwrap();
    let sender = sync_account_for(&sender_wallet, Some(2_208_270));
    let recipient_address = recipient_wallet.address.clone().unwrap();
    reset_scan_cache(&sender.spend_hex, &sender.view_hex, &sender.address, "stagenet").unwrap();

    let before = sync_account(&sender, "", "stagenet", &mut |_| true).unwrap();
    assert!(!before.offline);
    assert!(before.unlocked_piconero > 0, "nothing unlocked to spend: {}", before.balance_display());

    let plan = prepare_send(&sender, &recipient_address, "0.01", "", "stagenet", "Low").unwrap();
    println!("{}", plan.summary());
    assert_eq!(plan.amount_piconero, 10_000_000_000);
    assert!(plan.fee_piconero > 0 && plan.fee_piconero < plan.amount_piconero, "fee {}", plan.fee_piconero);
    assert!(!plan.inputs.is_empty());

    let txid = sign_and_broadcast(&sender, &plan, "", "stagenet").unwrap();
    assert_eq!(txid.len(), 64);
    println!("broadcast {txid}");

    // The inputs are marked spent at once, so a second send cannot reuse them, and the
    // send shows in history as pending.
    let after = sync_account(&sender, "", "stagenet", &mut |_| true).unwrap();
    assert!(after.unlocked_piconero + after.locked_piconero < before.unlocked_piconero + before.locked_piconero);
    let pending = after.history.iter().find(|item| item.txid == txid).expect("pending send in history");
    assert_eq!(pending.amount_piconero, -(plan.total_piconero as i64));
    assert_eq!(pending.confirmations, 0);

    std::env::remove_var("BLOCKWALLET_HOME");
    let _ = std::fs::remove_dir_all(root);
}

/// The other half of the send test, to run once its transaction has been mined: the
/// recipient account, scanned from the block of the send, must see the 0.01 XMR, and the
/// sender must see its change come back and the spend confirmed.
///
/// `XMR_SEND_TXID` names the transaction and `XMR_SEND_HEIGHT` a block at or before it.
#[test]
#[ignore]
fn the_recipient_and_sender_agree_after_the_send_is_mined() {
    use block_wallet::currencies::xmr_chain::{reset_scan_cache, sync_account};

    let txid = std::env::var("XMR_SEND_TXID").expect("XMR_SEND_TXID");
    let height: u64 = std::env::var("XMR_SEND_HEIGHT").expect("XMR_SEND_HEIGHT").parse().unwrap();
    let root = std::env::temp_dir().join(format!("blockwallet-xmr-recv-{}", std::process::id()));
    std::env::set_var("BLOCKWALLET_HOME", &root);

    let recipient_wallet =
        block_wallet::currencies::xmr::MoneroWallet::from_mnemonic_on(ABANDON, "blockwallet", XmrNetwork::Stagenet).unwrap();
    let recipient = sync_account_for(&recipient_wallet, Some(height));
    reset_scan_cache(&recipient.spend_hex, &recipient.view_hex, &recipient.address, "stagenet").unwrap();
    let received = sync_account(&recipient, "", "stagenet", &mut |_| true).unwrap();
    let item = received.history.iter().find(|item| item.txid == txid).expect("recipient sees the send");
    assert_eq!(item.amount_piconero, 10_000_000_000, "{item:?}");
    println!("recipient: {}", received.balance_display());

    let sender_wallet = block_wallet::currencies::xmr::MoneroWallet::from_mnemonic_on(ABANDON, "", XmrNetwork::Stagenet).unwrap();
    let sender = sync_account_for(&sender_wallet, Some(2_208_270));
    reset_scan_cache(&sender.spend_hex, &sender.view_hex, &sender.address, "stagenet").unwrap();
    let sent = sync_account(&sender, "", "stagenet", &mut |_| true).unwrap();
    let item = sent.history.iter().find(|item| item.txid == txid).expect("sender sees the spend");
    assert!(item.amount_piconero < 0, "{item:?}");
    assert!(item.confirmations > 0, "{item:?}");
    // 0.01 XMR plus a fee left; the rest came back as change and is counted, not lost.
    assert!(item.amount_piconero.unsigned_abs() > 10_000_000_000);
    assert!(item.amount_piconero.unsigned_abs() < 10_000_000_000 + 1_000_000_000, "fee is not absurd: {item:?}");
    println!("sender: {} (spend {})", sent.balance_display(), format_xmr(item.amount_piconero.unsigned_abs()));

    std::env::remove_var("BLOCKWALLET_HOME");
    let _ = std::fs::remove_dir_all(root);
}

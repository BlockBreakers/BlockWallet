use adw::prelude::*;
use adw::ApplicationWindow;
use glib::clone;
use gtk::{Button, Orientation};
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::configuration::application_settings::*;
use crate::configuration::onboarding::{self, WordCount};
use crate::views::ui;
use crate::views::{login, stack};

#[derive(Default)]
struct Draft {
    mnemonic: String,
    passphrase: String,
    from_restore: bool,
}

pub fn generate_wallet_view(window: ApplicationWindow, app_settings: ApplicationSettings) {
    let app_settings = Arc::new(Mutex::new(app_settings));
    let draft = Rc::new(RefCell::new(Draft::default()));

    let title = gtk::Label::builder()
        .label("Set up wallet")
        .css_classes(["title-4"])
        .build();
    let header = adw::HeaderBar::new();
    header.add_css_class("flat");
    header.set_title_widget(Some(&title));

    let stack = gtk::Stack::builder()
        .vexpand(true)
        .hexpand(true)
        .transition_type(gtk::StackTransitionType::SlideLeftRight)
        .build();

    let (choice_page, create_12, create_24, restore_btn, import_btn, choice_error) = choice_page();
    let (show_page, seed_grid, seed_continue, seed_back) = show_seed_page();
    let (confirm_page, confirm_view, confirm_continue, confirm_back, confirm_error) = confirm_page();
    let (restore_page, restore_view, restore_pass, restore_continue, restore_back, restore_error) =
        restore_page();
    let (password_page, password_input, repeat_input, password_save, password_back, password_error) =
        password_page();
    let (import_page, path_input, import_save, import_back, import_error) = import_page();

    stack.add_named(&choice_page, Some("choice"));
    stack.add_named(&show_page, Some("show_seed"));
    stack.add_named(&confirm_page, Some("confirm"));
    stack.add_named(&restore_page, Some("restore"));
    stack.add_named(&password_page, Some("password"));
    stack.add_named(&import_page, Some("import"));
    stack.set_visible_child_name("choice");

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(400);
    clamp.set_child(Some(&stack));

    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&clamp)
        .build();

    let outer = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .build();
    outer.append(&header);
    outer.append(&scroll);

    window.set_content(Some(&ui::with_toasts(&outer)));
    window.present();

    create_12.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] seed_grid,
        #[weak] choice_error,
        #[strong] draft,
        move |_| {
            start_create(
                WordCount::Words12,
                &draft,
                &seed_grid,
                &stack,
                &title,
                &choice_error,
            );
        }
    ));
    create_24.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] seed_grid,
        #[weak] choice_error,
        #[strong] draft,
        move |_| {
            start_create(
                WordCount::Words24,
                &draft,
                &seed_grid,
                &stack,
                &title,
                &choice_error,
            );
        }
    ));
    restore_btn.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] restore_error,
        move |_| {
            restore_error.set_visible(false);
            title.set_label("Restore wallet");
            stack.set_visible_child_name("restore");
        }
    ));
    import_btn.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] import_error,
        move |_| {
            import_error.set_visible(false);
            title.set_label("Import wallet file");
            stack.set_visible_child_name("import");
        }
    ));

    seed_back.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[strong] draft,
        move |_| {
            *draft.borrow_mut() = Draft::default();
            title.set_label("Set up wallet");
            stack.set_visible_child_name("choice");
        }
    ));
    seed_continue.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] confirm_error,
        #[weak] confirm_view,
        move |_| {
            confirm_error.set_visible(false);
            confirm_view.buffer().set_text("");
            title.set_label("Confirm phrase");
            stack.set_visible_child_name("confirm");
        }
    ));

    confirm_back.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        move |_| {
            title.set_label("Recovery phrase");
            stack.set_visible_child_name("show_seed");
        }
    ));
    confirm_continue.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] confirm_error,
        #[weak] confirm_view,
        #[strong] draft,
        move |_| {
            let typed = textview_content(&confirm_view);
            match onboarding::confirm_created_phrase(&draft.borrow().mnemonic, &typed) {
                Ok(_) => {
                    confirm_error.set_visible(false);
                    title.set_label("Set password");
                    stack.set_visible_child_name("password");
                }
                Err(err) => {
                    confirm_error.set_label(err.as_label());
                    confirm_error.set_visible(true);
                }
            }
        }
    ));

    restore_back.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[strong] draft,
        move |_| {
            *draft.borrow_mut() = Draft::default();
            title.set_label("Set up wallet");
            stack.set_visible_child_name("choice");
        }
    ));
    restore_continue.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[weak] restore_error,
        #[weak] restore_view,
        #[weak] restore_pass,
        #[strong] draft,
        move |_| {
            match onboarding::parse_restore_phrase(&textview_content(&restore_view)) {
                Ok(phrase) => {
                    restore_error.set_visible(false);
                    let mut draft = draft.borrow_mut();
                    draft.mnemonic = phrase;
                    draft.passphrase = restore_pass.text().to_string();
                    draft.from_restore = true;
                    title.set_label("Set password");
                    stack.set_visible_child_name("password");
                }
                Err(err) => {
                    restore_error.set_label(err.as_label());
                    restore_error.set_visible(true);
                }
            }
        }
    ));

    password_back.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        #[strong] draft,
        move |_| {
            if draft.borrow().from_restore {
                title.set_label("Restore wallet");
                stack.set_visible_child_name("restore");
            } else {
                title.set_label("Confirm phrase");
                stack.set_visible_child_name("confirm");
            }
        }
    ));

    password_save.connect_clicked(clone!(
        #[weak] window,
        #[weak] password_input,
        #[weak] repeat_input,
        #[weak] password_error,
        #[strong] draft,
        #[strong] app_settings,
        move |_| {
            let password = password_input.text().to_string();
            let repeat = repeat_input.text().to_string();
            if let Err(err) = onboarding::validate_password(&password, &repeat) {
                password_error.set_label(err.as_label());
                password_error.set_visible(true);
                return;
            }
            let mnemonic = draft.borrow().mnemonic.clone();
            let passphrase = draft.borrow().passphrase.clone();
            if mnemonic.is_empty() {
                password_error.set_label("Recovery phrase is missing. Go back and try again.");
                password_error.set_visible(true);
                return;
            }
            password_error.set_visible(false);
            let freshly_generated = !draft.borrow().from_restore;
            let saved = app_settings.lock().unwrap().finish_onboarding(
                &mnemonic,
                &passphrase,
                &password,
                freshly_generated,
            );
            match saved {
                Ok(()) => {
                    password_input.set_text("");
                    repeat_input.set_text("");
                    *draft.borrow_mut() = Draft::default();
                    let settings = app_settings.lock().unwrap().clone();
                    stack::stack_view(&window, settings);
                }
                Err(_) => {
                    password_error.set_label("Could not save the wallet. Try again.");
                    password_error.set_visible(true);
                }
            }
        }
    ));

    import_back.connect_clicked(clone!(
        #[weak] stack,
        #[weak] title,
        move |_| {
            title.set_label("Set up wallet");
            stack.set_visible_child_name("choice");
        }
    ));
    import_save.connect_clicked(clone!(
        #[weak] window,
        #[weak] path_input,
        #[weak] import_error,
        #[strong] app_settings,
        move |_| {
            let path_text = path_input.text().to_string();
            if Path::new(&path_text).exists() && path_text.contains(".dic") {
                let dest = app_settings.lock().unwrap().config_path.clone();
                if dest.exists() {
                    let mut old = dest.clone();
                    old.pop();
                    old.push(format!("Config-Old-{}.dic", chrono::Local::now()));
                    let _ = fs::rename(&dest, old);
                }
                match fs::copy(&path_text, &dest) {
                    Ok(_) => tracing::info!("imported wallet store"),
                    Err(_) => {
                        crate::configuration::logging::error("failed to import wallet store");
                        import_error.set_visible(true);
                        return;
                    }
                }
                path_input.set_text("");
                login::login_view(window.clone(), app_settings.lock().unwrap().clone());
            } else {
                import_error.set_visible(true);
                path_input.set_text("");
            }
        }
    ));
}

fn start_create(
    word_count: WordCount,
    draft: &Rc<RefCell<Draft>>,
    seed_grid: &gtk::Grid,
    stack: &gtk::Stack,
    title: &gtk::Label,
    choice_error: &gtk::Label,
) {
    match onboarding::generate_create_phrase(word_count) {
        Ok(phrase) => {
            choice_error.set_visible(false);
            let mut draft = draft.borrow_mut();
            draft.mnemonic = phrase.clone();
            draft.passphrase.clear();
            draft.from_restore = false;
            fill_seed_grid(seed_grid, &phrase);
            title.set_label("Recovery phrase");
            stack.set_visible_child_name("show_seed");
        }
        Err(_) => {
            crate::configuration::logging::error("failed to generate recovery phrase");
            choice_error.set_visible(true);
        }
    }
}

fn fill_seed_grid(grid: &gtk::Grid, phrase: &str) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
    for (index, word) in phrase.split_whitespace().enumerate() {
        // Number and word as separate labels, so the index reads as a quiet ordinal and
        // the word itself stays the prominent thing to copy down.
        let chip = gtk::Box::new(Orientation::Horizontal, 8);
        chip.add_css_class("seed-chip");
        chip.append(
            &gtk::Label::builder()
                .label(&format!("{:>2}", index + 1))
                .css_classes(["seed-index"])
                .build(),
        );
        chip.append(
            &gtk::Label::builder()
                .label(word)
                .xalign(0.0)
                .hexpand(true)
                .selectable(true)
                .css_classes(["seed-word"])
                .build(),
        );
        grid.attach(&chip, (index % 2) as i32, (index / 2) as i32, 1, 1);
    }
}

fn textview_content(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

fn page_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .margin_start(ui::GUTTER)
        .margin_end(ui::GUTTER)
        .margin_top(ui::GUTTER)
        .margin_bottom(20)
        .spacing(14)
        .build()
}

fn setup_button(label: &str) -> Button {
    let button = Button::builder().label(label).hexpand(true).build();
    button.add_css_class("standard_button");
    button.add_css_class("pill-button");
    button
}

fn wrapped_label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .max_width_chars(38)
        .build();
    for class in classes {
        label.add_css_class(class);
    }
    label
}

fn error_label(text: &str) -> gtk::Label {
    ui::error_label(text)
}

/// A step heading plus one line of explanation, so each onboarding page says what it is
/// before it asks for anything.
fn step_header(title: &str, subtitle: &str) -> gtk::Box {
    let header = ui::vbox(4);
    header.set_halign(gtk::Align::Center);
    header.append(
        &gtk::Label::builder()
            .label(title)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .css_classes(["onboard-title"])
            .build(),
    );
    header.append(&wrapped_label(subtitle, &["onboard-subtitle"]));
    header
}

fn seed_text_view() -> gtk::TextView {
    gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .left_margin(10)
        .right_margin(10)
        .top_margin(10)
        .bottom_margin(10)
        .build()
}

/// Wrap the phrase entry boxes in a card so they read as a field, not a bare text area.
fn seed_entry_frame(view: &gtk::TextView) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(140)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(view)
        .build();
    scroll.add_css_class("card");
    scroll
}

fn choice_page() -> (gtk::Box, Button, Button, Button, Button, gtk::Label) {
    let page = page_box();
    page.set_valign(gtk::Align::Center);
    page.set_vexpand(true);

    const TAGLINE: &str =
        "Self-custody Bitcoin, Ethereum, Solana and Litecoin. Your keys stay on this device.";
    match ui::logo_image(190) {
        Some(logo) => {
            page.append(&logo);
            // The artwork already reads "Block Wallet", so only the tagline is added here.
            page.append(&wrapped_label(TAGLINE, &["onboard-subtitle"]));
        }
        None => page.append(&step_header("Block Wallet", TAGLINE)),
    }

    // Creating is the common path, so it gets its own group and the only accent button.
    // Restore and import are recovery paths and sit in a quieter second group.
    let create_group = ui::group("Create a wallet");
    let create_12 = setup_button("Create 12-word wallet");
    create_12.add_css_class("suggested-action");
    let create_24 = setup_button("Create 24-word wallet");
    create_group.add(&create_12);
    create_group.add(&create_24);
    page.append(&create_group);

    let restore_group = ui::group("Already have one?");
    let restore_btn = setup_button("Restore from recovery phrase");
    let import_btn = setup_button("Import wallet file");
    restore_group.add(&restore_btn);
    restore_group.add(&import_btn);
    page.append(&restore_group);

    let choice_error = error_label("Could not generate a recovery phrase.");
    page.append(&choice_error);
    (
        page,
        create_12,
        create_24,
        restore_btn,
        import_btn,
        choice_error,
    )
}

fn show_seed_page() -> (gtk::Box, gtk::Grid, Button, Button) {
    let page = page_box();
    page.append(&step_header(
        "Your recovery phrase",
        "These words are the only way to restore this wallet.",
    ));
    page.append(
        &gtk::Label::builder()
            .label("Write them down in order and keep them offline. They will not be shown again after you continue. Anyone who has them can spend your funds.")
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .xalign(0.0)
            .css_classes(["danger-note"])
            .build(),
    );
    let seed_grid = gtk::Grid::builder()
        .column_spacing(8)
        .row_spacing(8)
        .column_homogeneous(true)
        .hexpand(true)
        .css_classes(["seed-grid"])
        .build();
    page.append(&seed_grid);
    let seed_continue = setup_button("I have written it down");
    seed_continue.add_css_class("suggested-action");
    let seed_back = setup_button("Back");
    page.append(&seed_continue);
    page.append(&seed_back);
    (page, seed_grid, seed_continue, seed_back)
}

fn confirm_page() -> (gtk::Box, gtk::TextView, Button, Button, gtk::Label) {
    let page = page_box();
    page.append(&step_header(
        "Confirm your phrase",
        "Type the words back in order to prove you have them written down.",
    ));
    let confirm_view = seed_text_view();
    page.append(&seed_entry_frame(&confirm_view));
    let confirm_error = error_label("Recovery phrase does not match.");
    let confirm_continue = setup_button("Confirm phrase");
    confirm_continue.add_css_class("suggested-action");
    let confirm_back = setup_button("Back");
    page.append(&confirm_error);
    page.append(&confirm_continue);
    page.append(&confirm_back);
    (page, confirm_view, confirm_continue, confirm_back, confirm_error)
}

fn restore_page() -> (gtk::Box, gtk::TextView, adw::PasswordEntryRow, Button, Button, gtk::Label) {
    let page = page_box();
    page.append(&step_header(
        "Restore a wallet",
        "Enter your 12 or 24-word recovery phrase.",
    ));
    let restore_view = seed_text_view();
    page.append(&seed_entry_frame(&restore_view));

    let pass_group = ui::group_with_description(
        "Optional passphrase",
        "The extra BIP39 word, if you set one. This is not your app password.",
    );
    let restore_pass = adw::PasswordEntryRow::builder().title("Passphrase").build();
    pass_group.add(&restore_pass);
    page.append(&pass_group);

    let restore_error = error_label("That recovery phrase is not valid.");
    let restore_continue = setup_button("Continue");
    restore_continue.add_css_class("suggested-action");
    let restore_back = setup_button("Back");
    page.append(&restore_error);
    page.append(&restore_continue);
    page.append(&restore_back);
    (
        page,
        restore_view,
        restore_pass,
        restore_continue,
        restore_back,
        restore_error,
    )
}

fn password_page() -> (gtk::Box, adw::PasswordEntryRow, adw::PasswordEntryRow, Button, Button, gtk::Label) {
    let page = page_box();
    page.append(&step_header(
        "Set a password",
        "This encrypts the wallet on this device. You will need it each time you unlock the app. A passphrase of a few words is easier to remember and far harder to guess than a short password.",
    ));
    let group = adw::PreferencesGroup::new();
    let password_input = adw::PasswordEntryRow::builder().title("Password").build();
    let repeat_input = adw::PasswordEntryRow::builder().title("Repeat password").build();
    group.add(&password_input);
    group.add(&repeat_input);
    page.append(&group);

    page.append(&ui::notice(
        "There is no password reset. If you lose it, only your recovery phrase can restore this wallet.",
    ));

    let password_error = error_label("Passwords do not match.");
    let password_save = setup_button("Save wallet");
    password_save.add_css_class("suggested-action");
    let password_back = setup_button("Back");
    page.append(&password_error);
    page.append(&password_save);
    page.append(&password_back);
    (
        page,
        password_input,
        repeat_input,
        password_save,
        password_back,
        password_error,
    )
}

fn import_page() -> (gtk::Box, adw::EntryRow, Button, Button, gtk::Label) {
    let page = page_box();
    page.append(&step_header(
        "Import a wallet file",
        "Load an existing Block Wallet store (.dic).",
    ));
    let group = adw::PreferencesGroup::new();
    let path_input = adw::EntryRow::builder().title("Path to .dic file").build();
    group.add(&path_input);
    page.append(&group);
    let import_error = error_label("File not found or incorrect type.");
    let import_save = setup_button("Import");
    import_save.add_css_class("suggested-action");
    let import_back = setup_button("Back");
    page.append(&import_error);
    page.append(&import_save);
    page.append(&import_back);
    (page, path_input, import_save, import_back, import_error)
}

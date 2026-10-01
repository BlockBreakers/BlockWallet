use adw::prelude::*;
use adw::ApplicationWindow;
use glib::{clone, ControlFlow};
use gtk::{Button, Orientation};
use std::sync::{Arc, Mutex};
use std::thread;
use zeroize::Zeroize;

use crate::configuration::application_settings::*;
use crate::views::stack;
use crate::views::ui;

pub fn lock_and_show(window: ApplicationWindow, app_settings: Arc<Mutex<ApplicationSettings>>) {
    app_settings.lock().unwrap().lock_store();
    let snapshot = app_settings.lock().unwrap().clone();
    // Auto-lock must not steal focus. If the user is in another app when the timer fires,
    // swapping in the lock screen is right but raising the window over what they are doing is
    // not: the wallet would jump to the front unprompted. The content is replaced either way,
    // so the lock screen is what they find when they come back.
    let was_active = window.is_active();
    build_login_view(window, snapshot, was_active);
}

/// Show the unlock screen and bring the window forward.
///
/// For the paths where the user is asking for it: app start, and finishing onboarding.
pub fn login_view(window: ApplicationWindow, app_settings: ApplicationSettings) {
    build_login_view(window, app_settings, true);
}

/// `present` is false when the lock screen is being installed behind the user's back, i.e. by
/// the idle timer while they are in another app.
fn build_login_view(window: ApplicationWindow, app_settings: ApplicationSettings, present: bool) {
    let header = adw::HeaderBar::new();
    header.add_css_class("flat");
    header.set_title_widget(Some(&gtk::Label::new(None)));

    // The unlock screen is the app's first impression and has exactly one job, so it gets
    // a centred lockup rather than a form crammed against the top of the window.
    let lockup = ui::vbox(10);
    lockup.set_halign(gtk::Align::Center);
    let logo = ui::logo_image(190);
    let has_logo = logo.is_some();
    match &logo {
        Some(image) => lockup.append(image),
        None => {
            let fallback = gtk::Image::from_icon_name("changes-prevent-symbolic");
            fallback.set_pixel_size(88);
            lockup.append(&fallback);
        }
    }
    // The logo artwork already carries the "Block Wallet" wordmark, so the text title
    // would just say it twice. It stays as the fallback for when the file is missing and
    // only a symbolic icon is shown.
    if !has_logo {
        lockup.append(
            &gtk::Label::builder()
                .label("Block Wallet")
                .css_classes(["onboard-title"])
                .build(),
        );
    }
    lockup.append(
        &gtk::Label::builder()
            .label("Enter your password to decrypt this wallet on this device.")
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .max_width_chars(34)
            .css_classes(["onboard-subtitle"])
            .build(),
    );

    let form = adw::PreferencesGroup::new();
    let input = adw::PasswordEntryRow::builder().title("Password").build();
    form.add(&input);

    let button = ui::primary_button("Unlock");

    let failed_login = ui::error_label("Unlock failed. Check the password and try again.");
    failed_login.set_halign(gtk::Align::Center);

    let page = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .margin_start(ui::GUTTER)
        .margin_end(ui::GUTTER)
        .margin_top(ui::GUTTER)
        .margin_bottom(24)
        .spacing(20)
        .build();
    page.append(&lockup);
    page.append(&form);
    page.append(&failed_login);
    page.append(&button);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(400);
    clamp.set_vexpand(true);
    clamp.set_child(Some(&page));

    let login_box = gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .build();
    login_box.append(&header);
    login_box.append(&ui::scroller(&clamp));

    stack::remove_shell_hooks(&window);
    window.set_content(Some(&ui::with_toasts(&login_box)));
    if present {
        window.present();
        // Focus the field so unlocking is type-and-Enter, not tap-then-type. Skipped when the
        // window is not being raised, since grabbing focus there would summon the on-screen
        // keyboard over whatever the user is actually doing.
        input.grab_focus();
    }
    let app_settings = Arc::new(Mutex::new(app_settings));

    button.connect_clicked(clone!(
        #[weak] window,
        #[weak] input,
        #[weak] failed_login,
        #[strong] app_settings,
        move |button| {
            try_unlock(&window, &input, &failed_login, button, &app_settings);
        }
    ));
    // `connect_entry_activated`, not `connect_activate`: the latter compiles, because
    // PasswordEntryRow inherits GtkListBoxRow's own "activate" signal, but it fires on row
    // activation rather than Enter in the text field — so pressing Enter would do nothing.
    input.connect_entry_activated(clone!(
        #[weak] window,
        #[weak] input,
        #[weak] failed_login,
        #[weak] button,
        #[strong] app_settings,
        move |_| {
            try_unlock(&window, &input, &failed_login, &button, &app_settings);
        }
    ));
}

fn try_unlock(
    window: &ApplicationWindow,
    input: &adw::PasswordEntryRow,
    failed_login: &gtk::Label,
    button: &Button,
    app_settings: &Arc<Mutex<ApplicationSettings>>,
) {
    let mut password = input.text().to_string();
    input.set_text("");
    if password.is_empty() {
        failed_login.set_visible(true);
        return;
    }

    // Argon2id at 64 MiB / t=3 takes appreciable time on a Librem 5, and this used to run on
    // the GTK main thread: the whole UI froze for the duration of every attempt, including
    // every wrong one. Off-thread instead, with the button disabled so a second tap cannot
    // start a concurrent derivation.
    failed_login.set_visible(false);
    button.set_sensitive(false);
    button.set_label("Unlocking…");

    let (sender, receiver) = crate::configuration::ui_channel::unbounded();
    let settings_for_thread = app_settings.clone();
    thread::spawn(move || {
        let result = settings_for_thread.lock().unwrap().unlock_store(&password);
        // The password has served its purpose; wipe this copy rather than letting the
        // allocation go back to the heap with the plaintext still in it.
        password.zeroize();
        let _ = sender.send_blocking(result.is_ok());
    });

    crate::configuration::ui_channel::attach(
        receiver,
        clone!(
            #[weak] window,
            #[weak] failed_login,
            #[weak] button,
            #[strong] app_settings,
            #[upgrade_or]
            ControlFlow::Break,
            move |unlocked| {
                button.set_sensitive(true);
                button.set_label("Unlock");
                if unlocked {
                    failed_login.set_visible(false);
                    let settings = app_settings.lock().unwrap().clone();
                    stack::stack_view(&window, settings);
                } else {
                    failed_login.set_visible(true);
                }
                ControlFlow::Break
            }
        ),
    );
}

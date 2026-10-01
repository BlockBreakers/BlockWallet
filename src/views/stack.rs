use adw::prelude::*;
use adw::ApplicationWindow;
use glib::{clone, ControlFlow};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::configuration::application_settings::*;
use crate::views::ui;
use crate::views::{activity, assets, header_bar, home, login, trade, wallets};

/// What a shell installs on its window. Each set captures that shell's own copy of the
/// settings, so it must go when the shell does: left connected, an old copy stays unlocked in
/// memory after Lock, and its close handler saves its out-of-date accounts over the store.
struct ShellHooks {
    window: glib::WeakRef<ApplicationWindow>,
    close: glib::SignalHandlerId,
    idle: glib::SourceId,
    activity: gtk::EventControllerLegacy,
}

thread_local! {
    static SHELL_HOOKS: RefCell<Vec<ShellHooks>> = const { RefCell::new(Vec::new()) };
}

/// Detach the previous shell's hooks from `window`. Called whenever the shell is rebuilt or
/// replaced by the lock screen.
pub fn remove_shell_hooks(window: &ApplicationWindow) {
    let stale: Vec<ShellHooks> = SHELL_HOOKS.with(|hooks| {
        let (stale, keep): (Vec<_>, Vec<_>) = hooks
            .borrow_mut()
            .drain(..)
            .partition(|h| h.window.upgrade().map_or(true, |w| &w == window));
        *hooks.borrow_mut() = keep;
        stale
    });
    for hooks in stale {
        // A destroyed window's timer stops itself on its next tick, and removing a source that
        // has gone panics, so only a live window's hooks are taken down here.
        if let Some(owner) = hooks.window.upgrade() {
            hooks.idle.remove();
            owner.disconnect(hooks.close);
            owner.remove_controller(&hooks.activity);
        }
    }
}

pub fn stack_view(window: &ApplicationWindow, app_settings_orig: ApplicationSettings) {
    remove_shell_hooks(window);
    let app_settings = Arc::new(Mutex::new(app_settings_orig));
    app_settings.lock().unwrap().update_balances();

    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header_bar = header_bar::header_bar_view(window.clone(), app_settings.clone());
    let stack = adw::ViewStack::new();

    let (home_box, home_nav, app_settings) = home::home_view(app_settings);
    stack
        .add_titled(&home_box, Some("Home"), "Home")
        .set_icon_name(Some("go-home-symbolic"));

    let (wallet_box, app_settings) = wallets::wallet_view(app_settings);
    stack
        .add_titled(&wallet_box, Some("Wallets"), "Wallets")
        .set_icon_name(Some("system-users-symbolic"));

    let (asset_box, asset_nav, app_settings) = assets::asset_view(app_settings);
    stack
        .add_titled(&asset_box, Some("Assets"), "Assets")
        .set_icon_name(Some("view-grid-symbolic"));

    let (trade_box, app_settings) = trade::trade_view(app_settings);
    stack
        .add_titled(&trade_box, Some("Swap"), "Swap")
        .set_icon_name(Some("object-flip-horizontal-symbolic"));

    let activity_box = activity::activity_view(app_settings.clone());
    stack
        .add_titled(&activity_box, Some("Activity"), "Activity")
        .set_icon_name(Some("view-list-symbolic"));

    let stack_bar = adw::ViewSwitcherBar::new();
    stack_bar.set_widget_name("stack_bar");
    stack_bar.set_stack(Some(&stack));
    stack_bar.set_reveal(true);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(520);
    clamp.set_vexpand(true);
    clamp.set_child(Some(&stack));

    container.append(&header_bar);
    container.append(&clamp);
    container.append(&stack_bar);

    // Toasts need to overlay the whole shell, so the overlay wraps the container rather
    // than any single page. Registering it here lets copy buttons anywhere below confirm
    // themselves without plumbing a handle through every constructor.
    let overlay = ui::with_toasts(&container);
    window.set_content(Some(&overlay));

    stack.connect_visible_child_notify(clone!(
        #[strong] home_nav,
        #[strong] asset_nav,
        move |stack| {
            let name = stack.visible_child_name().unwrap_or_default();
            if name != "Home" {
                home_nav.show_list();
            }
            if name != "Assets" {
                asset_nav.show_list();
            }
        }
    ));

    let last_input = Arc::new(Mutex::new(Instant::now()));

    // Every input event counts as activity, not just pointer motion and key presses.
    //
    // This is a phone. Tapping the screen produces touch events, and there is no physical
    // keyboard, so the previous motion+key controllers saw nothing at all during normal use
    // and the wallet locked itself while it was actively being used. `EventControllerLegacy`
    // sees the raw event stream, and the capture phase means a child widget consuming the
    // event (a button, an entry) does not hide it from the idle timer.
    let activity = gtk::EventControllerLegacy::new();
    activity.set_propagation_phase(gtk::PropagationPhase::Capture);
    activity.connect_event(clone!(
        #[strong] last_input,
        move |_, _| {
            *last_input.lock().unwrap() = Instant::now();
            glib::Propagation::Proceed
        }
    ));
    window.add_controller(activity.clone());

    let idle = glib::timeout_add_seconds_local(
        15,
        clone!(
            #[weak] window,
            #[strong] app_settings,
            #[strong] last_input,
            #[upgrade_or]
            ControlFlow::Break,
            move || {
                let timeout = app_settings.lock().unwrap().lock_timeout_secs;
                if timeout > 0
                    && last_input.lock().unwrap().elapsed() > Duration::from_secs(timeout as u64)
                    && app_settings.lock().unwrap().is_unlocked()
                {
                    login::lock_and_show(window.clone(), app_settings.clone());
                }
                ControlFlow::Continue
            }
        ),
    );

    let close = window.connect_close_request(clone!(
        #[strong] app_settings,
        move |_| {
            let _ = app_settings.lock().unwrap().write_config();
            glib::Propagation::Proceed
        }
    ));

    SHELL_HOOKS.with(|hooks| {
        hooks.borrow_mut().push(ShellHooks { window: window.downgrade(), close, idle, activity });
    });

    window.present();
}

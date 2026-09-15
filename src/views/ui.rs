//! Shared presentation helpers.
//!
//! Every screen used to hand-roll its own margins, labels and buttons, which is why the
//! app looked assembled rather than designed. These builders are the single source of
//! truth for spacing, chain identity and the standard widgets, so a change here moves the
//! whole app at once.

use adw::prelude::*;
use gtk::{Align, Orientation};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// Page gutter. One value, used everywhere, so nothing drifts by a stray pixel.
pub const GUTTER: i32 = 12;

// ------------------------------------------------------------------ chain identity

/// Brand colour class and display name for a wallet family. `chain` is `Token.chain`
/// ("btc" / "eth" / "sol" / "ltc" / "xmr"), the same dispatch key the send and balance code uses.
pub fn chain_css_class(chain: &str) -> &'static str {
    match chain {
        "btc" => "chain-btc",
        "sol" => "chain-sol",
        "ltc" => "chain-ltc",
        "xmr" => "chain-xmr",
        _ => "chain-eth",
    }
}

pub fn chain_display_name(chain: &str) -> &'static str {
    match chain {
        "btc" => "Bitcoin",
        "sol" => "Solana",
        "ltc" => "Litecoin",
        "xmr" => "Monero",
        _ => "Ethereum",
    }
}

/// Icon-theme name for a chain. Every name here was checked against the shipped Adwaita
/// icon theme — the old code passed "BTC"/"ETH" as icon names, which resolve to nothing
/// and rendered as the missing-image glyph.
pub fn chain_icon_name(chain: &str) -> &'static str {
    match chain {
        "btc" => "security-high-symbolic",
        "ltc" => "emoji-symbols-symbolic",
        "xmr" => "view-conceal-symbolic",
        "sol" => "weather-clear-symbolic",
        _ => "emblem-system-symbolic",
    }
}

/// Whether a token row should carry a chain tag.
///
/// The tag exists to disambiguate tokens that live on more than one chain — an ERC-20
/// USDC from an SPL USDC. For a chain's own native asset it just repeats the name
/// ("Bitcoin · Bitcoin"), so it is suppressed there.
pub fn needs_chain_tag(token_name: &str, chain: &str) -> bool {
    !token_name.eq_ignore_ascii_case(chain_display_name(chain))
}

/// The Block Wallet logo lockup as an image that follows the light/dark theme.
///
/// The artwork is drawn for light backgrounds — the cube's faces and the word "Block" are
/// a near-black navy that disappears on a dark surface — so `Logo-dark.png` carries a
/// recoloured variant. Returns `None` when no logo file is installed, letting callers fall
/// back to a symbolic icon plus a text title.
pub fn logo_image(size: i32) -> Option<gtk::Image> {
    let dir = crate::configuration::paths::images_path().ok()?;
    let light = dir.join("Logo.png");
    if !light.is_file() {
        return None;
    }
    let dark = dir.join("Logo-dark.png");

    let image = gtk::Image::new();
    image.set_pixel_size(size);

    let style = adw::StyleManager::default();
    image.set_from_file(Some(pick_logo(&light, &dark, style.is_dark())));
    // The system theme can flip while the app is open, so track it rather than sampling
    // once at construction.
    style.connect_dark_notify(glib::clone!(
        #[weak] image,
        move |style: &adw::StyleManager| {
            image.set_from_file(Some(pick_logo(&light, &dark, style.is_dark())));
        }
    ));
    Some(image)
}

fn pick_logo<'a>(light: &'a Path, dark: &'a Path, is_dark: bool) -> &'a Path {
    if is_dark && dark.is_file() {
        dark
    } else {
        light
    }
}

/// Round coin mark: the bundled PNG when one exists, otherwise a chain-coloured monogram.
///
/// Not every token has an icon (LTC has no bundled PNG at all, and add-by-contract tokens
/// never will), and `gtk::Image::from_file` on a missing path silently renders the broken
/// image icon. The monogram keeps those rows looking deliberate.
pub fn coin_mark(logo: &Path, symbol: &str, chain: &str, size: i32) -> gtk::Widget {
    if logo.is_file() {
        let icon = gtk::Image::from_file(logo);
        icon.set_pixel_size(size);
        icon.set_valign(Align::Center);
        return icon.upcast();
    }

    let initials: String = symbol.chars().take(2).collect::<String>().to_uppercase();
    let label = gtk::Label::builder()
        .label(&initials)
        .width_request(size)
        .height_request(size)
        .valign(Align::Center)
        .halign(Align::Center)
        .css_classes(["coin-monogram", chain_css_class(chain)])
        .build();
    label.upcast()
}

// -------------------------------------------------------------------- containers

pub fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(spacing)
        .build()
}

pub fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(spacing)
        .build()
}

/// Standard scrolling page body: vertical, guttered, never scrolls sideways.
pub fn page_body(spacing: i32) -> gtk::Box {
    gtk::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(spacing)
        .margin_start(GUTTER)
        .margin_end(GUTTER)
        .margin_top(GUTTER)
        .margin_bottom(GUTTER + 4)
        .build()
}

pub fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(child)
        .build()
}

/// A titled group of rows in libadwaita's boxed-list style.
pub fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

pub fn group_with_description(title: &str, description: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder()
        .title(title)
        .description(description)
        .build()
}

// ------------------------------------------------------------------------ labels

pub fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(Align::Start)
        .css_classes(["heading"])
        .build()
}

pub fn body(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(Align::Start)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .build()
}

pub fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(Align::Start)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build()
}

pub fn field_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(Align::Start)
        .css_classes(["field-label"])
        .build()
}

pub fn error_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .xalign(0.0)
        .visible(false)
        .css_classes(["label-error"])
        .build()
}

/// A soft inline notice strip. `warning` swaps it to the amber treatment.
pub fn notice(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(["info-banner"])
        .build()
}

pub fn set_notice_warning(label: &gtk::Label, warning: bool) {
    if warning {
        label.add_css_class("warning");
    } else {
        label.remove_css_class("warning");
    }
}

/// Monospace, character-wrapped, selectable — the right treatment for an address or key,
/// which must be readable digit by digit and never re-wrapped mid-word.
pub fn mono_address(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .selectable(true)
        .wrap(true)
        .wrap_mode(pango::WrapMode::Char)
        .xalign(0.0)
        .halign(Align::Start)
        .css_classes(["address-mono"])
        .build()
}

// ----------------------------------------------------------------------- buttons

pub fn button(label: &str) -> gtk::Button {
    let button = gtk::Button::builder().label(label).hexpand(true).build();
    button.add_css_class("standard_button");
    button
}

/// Primary call to action: pill-shaped and accent-filled.
pub fn primary_button(label: &str) -> gtk::Button {
    let button = button(label);
    button.add_css_class("pill-button");
    button.add_css_class("suggested-action");
    button
}

/// Icon + label, for the Send/Receive pair where the icon does most of the work.
pub fn icon_button(label: &str, icon: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder()
        .label(label)
        .icon_name(icon)
        .halign(Align::Center)
        .build();
    let button = gtk::Button::builder().child(&content).hexpand(true).build();
    button.add_css_class("standard_button");
    button.add_css_class("pill-button");
    button
}

pub fn flat_icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("flat");
    button.set_valign(Align::Center);
    button
}

// -------------------------------------------------------------------------- rows

/// Labelled text input as a boxed-list row. Replaces the app's previous pattern of bare
/// `GtkEntry`s stacked in a column, where the placeholder was the only clue what a field
/// was for — and vanished the moment you typed.
pub fn entry_row(title: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).build()
}

pub fn password_row(title: &str) -> adw::PasswordEntryRow {
    adw::PasswordEntryRow::builder().title(title).build()
}

pub fn combo_row(title: &str, options: &[&str]) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(options))
        .build()
}

// ------------------------------------------------------------------ searchable picker

/// Does this picker label match what the user has typed?
///
/// A free function rather than a closure body so it can be tested without a display, which
/// is the whole reason the matching rule lives here instead of inline in the filter.
///
/// Case-insensitive substring, over the whole visible label. Substring rather than prefix
/// because the labels carry the chain in them ("USDC (Ethereum)"), so typing a chain name is
/// a reasonable way to narrow a list of three hundred, and prefix matching would find
/// nothing. `needle` is expected already lowercased by the caller.
pub fn picker_matches(label: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    label.to_lowercase().contains(needle)
}

/// A row that opens a searchable list, for choices too numerous for a dropdown.
///
/// `AdwComboRow` is right for a handful of options and wrong for several hundred: the bundled
/// token list runs to about 275 entries on Ethereum mainnet, and a dropdown that long is a
/// single unscannable column on a 360 px phone screen. This presents the same choice as an
/// activatable row that opens a dialog with a search entry, which is how every phone contact
/// picker works and for the same reason.
///
/// Deliberately built from `AdwWindow`, `GtkSearchEntry` and `GtkListBox`, all present in
/// libadwaita 1.2 and GTK 4.8. `AdwDialog` would be the modern spelling but it needs 1.5,
/// which would raise the floor above Debian bookworm and break the phone this targets.
#[derive(Clone)]
pub struct PickerRow {
    row: adw::ActionRow,
    options: Rc<Vec<String>>,
    selected: Rc<Cell<usize>>,
    on_change: Rc<RefCell<Vec<Rc<dyn Fn()>>>>,
}

impl PickerRow {
    pub fn new(title: &str, options: &[String]) -> Self {
        let row = adw::ActionRow::builder()
            .title(title)
            .activatable(true)
            .build();
        // A chevron, matching what AdwComboRow shows, so the row reads as "opens something"
        // rather than as a static label.
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

        let picker = Self {
            row,
            options: Rc::new(options.to_vec()),
            selected: Rc::new(Cell::new(0)),
            on_change: Rc::new(RefCell::new(Vec::new())),
        };
        picker.apply_selection();

        let opener = picker.clone();
        picker.row.connect_activated(move |row| opener.open_dialog(row));
        picker
    }

    pub fn row(&self) -> &adw::ActionRow {
        &self.row
    }

    pub fn selected(&self) -> usize {
        self.selected.get()
    }

    pub fn set_selected(&self, index: usize) {
        if index >= self.options.len() {
            return;
        }
        if self.selected.get() == index {
            return;
        }
        self.selected.set(index);
        self.apply_selection();
        self.notify();
    }

    /// Run `f` whenever the selection changes, matching `connect_selected_notify` on a combo
    /// row so callers can treat the two the same way.
    pub fn connect_changed<F: Fn() + 'static>(&self, f: F) {
        self.on_change.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        // The handler list is cloned out and the borrow dropped before anything is called. A
        // handler is free to touch this picker (the swap screen's invalidation gate does), and
        // calling one while the list is still borrowed would panic at runtime rather than
        // fail to compile.
        let handlers: Vec<Rc<dyn Fn()>> = self.on_change.borrow().iter().cloned().collect();
        for handler in handlers {
            handler();
        }
    }

    fn apply_selection(&self) {
        let label = self
            .options
            .get(self.selected.get())
            .cloned()
            .unwrap_or_default();
        self.row.set_subtitle(&label);
    }

    fn open_dialog(&self, anchor: &adw::ActionRow) {
        let window = adw::Window::builder()
            .modal(true)
            .default_width(360)
            .default_height(640)
            .build();
        if let Some(root) = anchor.root().and_downcast::<gtk::Window>() {
            window.set_transient_for(Some(&root));
        }

        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(&self.row.title(), "")));
        let cancel = gtk::Button::with_label("Cancel");
        header.pack_start(&cancel);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search"));
        search.set_margin_start(GUTTER);
        search.set_margin_end(GUTTER);
        search.set_margin_top(GUTTER);
        search.set_margin_bottom(GUTTER / 2);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("boxed-list");
        list.set_margin_start(GUTTER);
        list.set_margin_end(GUTTER);
        list.set_margin_bottom(GUTTER);

        for (index, option) in self.options.iter().enumerate() {
            let item = adw::ActionRow::builder()
                .title(option)
                .activatable(true)
                .build();
            if index == self.selected.get() {
                item.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
            let picker = self.clone();
            let window_ref = window.clone();
            item.connect_activated(move |_| {
                picker.set_selected(index);
                window_ref.close();
            });
            list.append(&item);
        }

        // The row index is recovered from the widget position rather than stored alongside it,
        // so the filter needs no parallel bookkeeping to stay in step with the list.
        let options = Rc::clone(&self.options);
        let query = Rc::new(RefCell::new(String::new()));
        let query_for_filter = Rc::clone(&query);
        list.set_filter_func(move |row| {
            let needle = query_for_filter.borrow();
            options
                .get(row.index().max(0) as usize)
                .map(|label| picker_matches(label, &needle))
                .unwrap_or(true)
        });

        let list_for_search = list.clone();
        search.connect_search_changed(move |entry| {
            *query.borrow_mut() = entry.text().to_lowercase();
            list_for_search.invalidate_filter();
        });

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let body = gtk::Box::new(Orientation::Vertical, 0);
        body.append(&header);
        body.append(&search);
        body.append(&scroller);
        window.set_content(Some(&body));

        let window_for_cancel = window.clone();
        cancel.connect_clicked(move |_| window_for_cancel.close());

        window.present();
        search.grab_focus();
    }
}

/// Add a labelled toggle row to `group` and hand back the switch itself.
///
/// This is an `AdwActionRow` with a `GtkSwitch` suffix rather than an `AdwSwitchRow`,
/// which is what `AdwSwitchRow` is internally anyway. Hand-building it keeps the whole app
/// inside the libadwaita 1.2 API, so it still builds on Debian bookworm; `AdwSwitchRow`
/// alone would raise the floor to 1.4.
///
/// The switch is returned (rather than the row) because it is a GObject, so callers can
/// hold it with `#[weak]` in a `clone!` and read `is_active()` directly.
pub fn add_switch_row(
    group: &adw::PreferencesGroup,
    title: &str,
    subtitle: &str,
    active: bool,
) -> gtk::Switch {
    let switch = gtk::Switch::builder()
        .active(active)
        .valign(Align::Center)
        .build();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(true)
        .build();
    row.add_suffix(&switch);
    // Makes the whole row a hit target for the toggle, matching AdwSwitchRow. On a phone
    // the switch alone is a small thing to land a thumb on.
    row.set_activatable_widget(Some(&switch));
    group.add(&row);
    switch
}

/// A row whose whole surface is a button.
pub fn action_button_row(title: &str, subtitle: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(true)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}

// ------------------------------------------------------------------------ toasts

thread_local! {
    /// The overlay wrapping the current window content. Set once per screen build so any
    /// nested widget can raise a toast without threading an overlay handle through every
    /// constructor. Thread-local because GTK widgets are main-thread-only anyway.
    static TOAST_OVERLAY: RefCell<Option<adw::ToastOverlay>> = const { RefCell::new(None) };
}

pub fn register_toast_overlay(overlay: &adw::ToastOverlay) {
    TOAST_OVERLAY.with(|cell| *cell.borrow_mut() = Some(overlay.clone()));
}

/// Show a transient confirmation. Silently does nothing if no overlay is registered, so
/// callers never have to branch on whether the current screen has one.
pub fn toast(message: &str) {
    TOAST_OVERLAY.with(|cell| {
        if let Some(overlay) = cell.borrow().as_ref() {
            overlay.add_toast(adw::Toast::builder().title(message).timeout(3).build());
        }
    });
}

/// Wrap a page in a toast overlay and register it as the active one.
pub fn with_toasts(child: &impl IsA<gtk::Widget>) -> adw::ToastOverlay {
    let overlay = adw::ToastOverlay::new();
    overlay.set_child(Some(child));
    register_toast_overlay(&overlay);
    overlay
}

// ------------------------------------------------------------------- empty state

pub fn empty_state(title: &str, description: &str, icon: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .title(title)
        .description(description)
        .icon_name(icon)
        .vexpand(true)
        .build()
}

// ----------------------------------------------------------------- network chips

/// The live/test chip shown in the header bar. Knowing which network you are on is the
/// difference between a harmless test and spending real money, so it belongs on screen
/// permanently rather than buried in Settings.
pub fn network_chip(text: &str, is_test: bool) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .valign(Align::Center)
        .css_classes(["net-chip", if is_test { "test" } else { "live" }])
        .build()
}

/// Shorten an address for display: `bc1qxy…9k3f`. Full text stays available by copy.
pub fn short_address(address: &str) -> String {
    if address.chars().count() <= 20 {
        return address.to_string();
    }
    let chars: Vec<char> = address.chars().collect();
    let head: String = chars[..10].iter().collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_address_keeps_both_ends() {
        let full = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        let short = short_address(full);
        assert!(short.starts_with("bc1qar0srr"));
        assert!(short.ends_with("wf5mdq"));
        assert!(short.contains('…'));
    }

    #[test]
    fn short_address_leaves_short_strings_alone() {
        assert_eq!(short_address("0xabc"), "0xabc");
    }

    #[test]
    fn chain_classes_are_distinct_per_family() {
        // A shared class would let two chains' rows look identical, which is exactly the
        // confusion the coloured monogram exists to prevent.
        let classes = ["btc", "eth", "sol", "ltc", "xmr"].map(chain_css_class);
        let mut unique = classes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 5);
    }

    #[test]
    fn picker_search_matches_symbol_and_chain_anywhere_in_the_label() {
        // Labels the swap screen actually builds.
        let usdc_eth = "USDC (Ethereum)";
        let usdc_sol = "USDC (Solana)";
        let wbtc = "WBTC (Ethereum)";

        // Empty query shows everything, so opening the picker is not an empty screen.
        assert!(picker_matches(usdc_eth, ""));

        // Case-insensitive on the symbol.
        assert!(picker_matches(usdc_eth, "usdc"));
        assert!(picker_matches(usdc_eth, "usd"));
        assert!(!picker_matches(wbtc, "usdc"));

        // Substring, not prefix: the chain sits at the end of the label, and narrowing three
        // hundred tokens by chain is a reasonable thing to want.
        assert!(picker_matches(usdc_sol, "solana"));
        assert!(!picker_matches(usdc_eth, "solana"));

        // A query matching nothing matches nothing, rather than falling open.
        assert!(!picker_matches(usdc_eth, "zzzz"));
    }
}

use adw::prelude::*;
use gtk::{Button, Orientation};

/// In-tab list/detail/send navigation without walking the widget tree.
#[derive(Clone)]
pub struct Nav {
    pub stack: gtk::Stack,
    pub back: Button,
    /// Strip holding the back button. Owned here, and shown or hidden directly by
    /// `push`/`show_list`, so a pushed page always gets a visible way back and a list page
    /// never reserves an empty strip.
    bar: gtk::Box,
}

impl Nav {
    pub fn new(list: &impl IsA<gtk::Widget>) -> Self {
        let stack = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::SlideLeftRight)
            .build();
        stack.add_named(list, Some("list"));
        stack.set_visible_child_name("list");

        // Icon plus "Back": on a phone an unlabelled arrow floating over content is easy
        // to miss, and this is the only way out of a detail or send page.
        let content = adw::ButtonContent::builder()
            .icon_name("go-previous-symbolic")
            .label("Back")
            .build();
        let back = Button::builder().child(&content).build();
        back.set_tooltip_text(Some("Back"));
        back.add_css_class("flat");
        back.set_valign(gtk::Align::Center);

        let bar = gtk::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .margin_start(6)
            .margin_end(6)
            .margin_top(4)
            .visible(false)
            .build();
        bar.append(&back);

        let stack_back = stack.clone();
        let bar_back = bar.clone();
        back.connect_clicked(move |_| {
            let current = stack_back
                .visible_child_name()
                .unwrap_or_else(|| "list".into());
            if current == "send" && stack_back.child_by_name("detail").is_some() {
                stack_back.set_visible_child_name("detail");
            } else {
                stack_back.set_visible_child_name("list");
                bar_back.set_visible(false);
            }
        });

        Self { stack, back, bar }
    }

    pub fn wrap(self) -> gtk::Box {
        let outer = gtk::Box::builder()
            .orientation(Orientation::Vertical)
            .build();
        outer.append(&self.bar);
        outer.append(&self.stack);
        outer
    }

    pub fn push(&self, name: &str, child: &impl IsA<gtk::Widget>) {
        if let Some(existing) = self.stack.child_by_name(name) {
            self.stack.remove(&existing);
        }
        self.stack.add_named(child, Some(name));
        self.stack.set_visible_child_name(name);
        self.bar.set_visible(true);
    }

    pub fn show_list(&self) {
        self.stack.set_visible_child_name("list");
        self.bar.set_visible(false);
    }
}

pub fn parse_leading_amount(text: &str) -> f64 {
    text.split_whitespace()
        .find_map(|part| part.replace(',', "").parse::<f64>().ok())
        .unwrap_or(0.0)
}

pub fn label_is_offline(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("offline") || lower.contains("unreachable")
}

pub fn label_is_pending_sync(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("uninitialized") || lower.contains("syncing")
}

/// What to show for a balance that is still syncing.
///
/// A chain that reports its own progress ("Syncing… 42% (840 of 2000 blocks)") keeps that
/// text, since for Monero the wait can be minutes and a bare word gives no sense of it
/// ending. Anything else, including the "Uninitialized" a loop starts with, becomes the
/// plain word.
pub fn sync_label(text: &str) -> String {
    if text.starts_with("Syncing") {
        text.to_string()
    } else {
        "Syncing…".to_string()
    }
}

/// Is this balance label a confirmed zero, safe to hide?
///
/// Only a real, settled zero counts. "Syncing…" and an offline label both parse as an amount
/// of zero, but neither is a statement that the balance *is* zero: one is "not asked yet" and
/// the other is "could not ask". Hiding those would make rows flicker in and out as a sync
/// lands, and worse, would quietly hide an asset the user does hold whenever their node is
/// unreachable. So the rule is deliberately narrow: hide only what the wallet has actually
/// been told is empty.
pub fn is_confirmed_zero(display: &str) -> bool {
    if label_is_offline(display) || label_is_pending_sync(display) {
        return false;
    }
    parse_leading_amount(display) == 0.0
}

/// Does this label report an amount actually held?
///
/// Not the negation of [`is_confirmed_zero`], and deliberately so. A label can be neither: a
/// syncing row and an unreachable node at zero are both "we do not know yet". Only a positive
/// amount counts here, including one carried over from the last good sync while a node is
/// offline, because that is still a real holding the wallet was told about.
pub fn has_balance(display: &str) -> bool {
    parse_leading_amount(display) > 0.0
}

pub fn format_btc_units(display: &str, units: &str) -> String {
    if units.eq_ignore_ascii_case("sats") {
        let btc = parse_leading_amount(display);
        let sats = (btc * 100_000_000.0).round() as i64;
        let rest = display
            .find(' ')
            .map(|i| &display[i..])
            .unwrap_or("")
            .replace("BTC", "sats");
        return format!("{sats}{rest}");
    }
    display.to_string()
}

/// Inline status strip. Delegates to the shared notice styling so there is exactly one
/// banner look in the app.
pub fn banner(text: &str) -> gtk::Label {
    crate::views::ui::notice(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_leading_amount_reads_display_strings() {
        assert_eq!(parse_leading_amount("0.001 BTC"), 0.001);
        assert_eq!(parse_leading_amount("1.5 ETH (offline)"), 1.5);
        assert_eq!(parse_leading_amount("offline"), 0.0);
        assert_eq!(parse_leading_amount("Uninitialized"), 0.0);
    }

    #[test]
    fn format_btc_units_can_show_sats() {
        let out = format_btc_units("0.00010000 BTC", "sats");
        assert!(out.contains("10000"));
        assert!(out.contains("sats"));
        assert_eq!(format_btc_units("0.5 BTC", "btc"), "0.5 BTC");
    }

    #[test]
    fn offline_and_sync_labels() {
        assert!(label_is_offline("0.0 BTC (offline)"));
        assert!(label_is_pending_sync("Uninitialized"));
        assert!(!label_is_offline("0.1 ETH"));
    }

    #[test]
    fn only_a_settled_zero_counts_as_hideable() {
        // Real zeros, in the shapes the assets screen builds.
        assert!(is_confirmed_zero("0 BTC"));
        assert!(is_confirmed_zero("0 ETH"));
        assert!(is_confirmed_zero("0.00000000 BTC"));

        // Anything held is never hidden.
        assert!(!is_confirmed_zero("0.001 BTC"));
        assert!(!is_confirmed_zero("1.5 ETH"));

        // Not yet asked. Hiding these would make rows flicker in and out as a sync lands.
        assert!(!is_confirmed_zero("Syncing…"));
        assert!(!is_confirmed_zero("Uninitialized"));

        // Could not ask. This is the one that matters: an unreachable node reports zero, and
        // hiding on that would quietly remove an asset the user really does hold.
        assert!(!is_confirmed_zero("0 BTC (offline)"));
        assert!(!is_confirmed_zero("0 ETH (offline)"));
        assert!(!is_confirmed_zero("Node unreachable"));

        // A held balance that cannot be refreshed stays visible too.
        assert!(!is_confirmed_zero("2.5 ETH (offline)"));
    }

    #[test]
    fn a_sync_label_keeps_reported_progress_and_hides_the_placeholder() {
        // Monero reports how far along it is; that is worth showing, and it still counts as
        // pending so nothing hides or totals it.
        let progress = "Syncing… 42% (840 of 2000 blocks)";
        assert_eq!(sync_label(progress), progress);
        assert!(label_is_pending_sync(progress));
        assert!(!is_confirmed_zero(progress));
        // The word every loop starts with is an implementation detail, not a status.
        assert_eq!(sync_label("Uninitialized"), "Syncing…");
        assert_eq!(sync_label("Syncing…"), "Syncing…");
    }
}

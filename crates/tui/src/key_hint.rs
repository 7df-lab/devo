use std::borrow::Cow;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Span;

#[cfg(test)]
const ALT_PREFIX: &str = "⌥ + ";
#[cfg(all(not(test), target_os = "macos"))]
const ALT_PREFIX: &str = "⌥ + ";
#[cfg(all(not(test), not(target_os = "macos")))]
const ALT_PREFIX: &str = "alt + ";
const CTRL_PREFIX: &str = "ctrl + ";
const SHIFT_PREFIX: &str = "shift + ";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KeyBinding {
    key: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyBinding {
    pub(crate) const fn new(key: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { key, modifiers }
    }

    pub fn is_press(&self, event: KeyEvent) -> bool {
        self.key == event.code
            && self.modifiers == event.modifiers
            && (event.kind == KeyEventKind::Press || event.kind == KeyEventKind::Repeat)
    }
}

pub(crate) const fn plain(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::NONE)
}

pub(crate) const fn alt(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::ALT)
}

pub(crate) const fn shift(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::SHIFT)
}

pub(crate) const fn ctrl(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::CONTROL)
}

pub(crate) const fn ctrl_alt(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::CONTROL.union(KeyModifiers::ALT))
}

impl From<KeyBinding> for Span<'static> {
    fn from(binding: KeyBinding) -> Self {
        (&binding).into()
    }
}
impl From<&KeyBinding> for Span<'static> {
    fn from(binding: &KeyBinding) -> Self {
        let KeyBinding { key, modifiers } = binding;
        let key = match key {
            KeyCode::Enter => Cow::Borrowed("enter"),
            KeyCode::Char(' ') => Cow::Borrowed("space"),
            KeyCode::Up => Cow::Borrowed("↑"),
            KeyCode::Down => Cow::Borrowed("↓"),
            KeyCode::Left => Cow::Borrowed("←"),
            KeyCode::Right => Cow::Borrowed("→"),
            KeyCode::PageUp => Cow::Borrowed("pgup"),
            KeyCode::PageDown => Cow::Borrowed("pgdn"),
            _ => Cow::Owned(format!("{key}").to_ascii_lowercase()),
        };
        let mut label = String::with_capacity(
            CTRL_PREFIX.len() + SHIFT_PREFIX.len() + ALT_PREFIX.len() + key.len(),
        );
        if modifiers.contains(KeyModifiers::CONTROL) {
            label.push_str(CTRL_PREFIX);
        }
        if modifiers.contains(KeyModifiers::SHIFT) {
            label.push_str(SHIFT_PREFIX);
        }
        if modifiers.contains(KeyModifiers::ALT) {
            label.push_str(ALT_PREFIX);
        }
        label.push_str(&key);
        Span::styled(label, key_hint_style())
    }
}

fn key_hint_style() -> Style {
    Style::default().dim()
}

pub(crate) fn has_ctrl_or_alt(mods: KeyModifiers) -> bool {
    (mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::ALT)) && !is_altgr(mods)
}

#[cfg(windows)]
#[inline]
pub(crate) fn is_altgr(mods: KeyModifiers) -> bool {
    mods.contains(KeyModifiers::ALT) && mods.contains(KeyModifiers::CONTROL)
}

#[cfg(not(windows))]
#[inline]
pub(crate) fn is_altgr(_mods: KeyModifiers) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use pretty_assertions::assert_eq;
    use ratatui::text::Span;

    use super::*;

    fn label(binding: KeyBinding) -> String {
        let span = Span::from(binding);
        span.content.into_owned()
    }

    #[test]
    fn key_hint_labels_preserve_modifier_order_and_special_names() {
        assert_eq!(label(ctrl_alt(KeyCode::Enter)), "ctrl + ⌥ + enter");
        assert_eq!(label(shift(KeyCode::Char(' '))), "shift + space");
        assert_eq!(label(plain(KeyCode::Up)), "↑");
    }
}

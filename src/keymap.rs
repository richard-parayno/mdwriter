//! App shortcuts. Each action has a default that `settings.keybinds` can override by id ("" turns
//! it off). A shortcut is stored like "Mod+Shift+P": modifiers in the order Mod (⌘ on macOS, Ctrl
//! elsewhere), Ctrl, Alt (⌥), Shift, then a key. Rune's `Chord` matches and draws them.

use std::collections::BTreeMap;

use rune::{Chord, ChordKey, ChordMod, KeyEvent, KeyPlatform, Shortcut};

use crate::backend::Settings;

pub struct Action {
    pub id: &'static str,
    pub label: &'static str,
    pub key: &'static str,
}

pub const ACTIONS: &[Action] = &[
    Action { id: "palette", label: "Command palette", key: "Mod+K" },
    Action { id: "switchProject", label: "Switch project", key: "Mod+O" },
    Action { id: "newPost", label: "New post", key: "Mod+N" },
    Action { id: "sync", label: "Sync with GitHub", key: "Mod+R" },
    Action { id: "settings", label: "Settings", key: "Mod+," },
    Action { id: "pull", label: "Pull", key: "Mod+Shift+L" },
    Action { id: "push", label: "Push", key: "Mod+Shift+P" },
    Action { id: "openPr", label: "Open PR", key: "" },
    Action { id: "review", label: "Review changes", key: "" },
    Action { id: "markers", label: "Toggle change markers", key: "Mod+Shift+D" },
];

/// Editing and window shortcuts that a binding may not take over.
const RESERVED: &[&str] = &["Mod+Q", "Mod+W", "Mod+H", "Mod+M", "Mod+A", "Mod+C", "Mod+V", "Mod+X", "Mod+Z", "Mod+Shift+Z"];

pub fn chord(binding: &str) -> Option<Chord> {
    if binding.is_empty() { return None; }
    Chord::parse(&binding.to_lowercase()).ok()
}

/// Why a shortcut can't be used, or None when it can.
pub fn problem(binding: &str) -> Option<String> {
    let Some(c) = chord(binding) else { return Some("Not a shortcut".into()) };
    let held = [ChordMod::Primary, ChordMod::Ctrl, ChordMod::Super].iter().any(|m| c.has(*m));
    if !held && !matches!(c.key(), ChordKey::Function(_)) {
        return Some("Include ⌘ or ⌃, so typing isn't caught".into());
    }
    let platform = KeyPlatform::current();
    if RESERVED.iter().filter_map(|r| chord(r)).any(|r| r.same_keys(&c, platform)) {
        return Some(format!("{} is kept for editing and the window", display(binding)));
    }
    None
}

/// "Mod+Shift+P" as "⇧⌘P" on macOS and "Ctrl+Shift+P" elsewhere.
pub fn display(binding: &str) -> String {
    chord(binding).map(|c| c.label()).unwrap_or_default()
}

/// The shortcut of every action. An override that is unusable falls back to the default.
pub fn bindings(keybinds: &BTreeMap<String, String>) -> BTreeMap<&'static str, String> {
    ACTIONS.iter().map(|a| {
        let key = match keybinds.get(a.id) {
            Some(o) if o.is_empty() || problem(o).is_none() => o.clone(),
            _ => a.key.to_string(),
        };
        (a.id, key)
    }).collect()
}

/// The action a key press runs, if any.
pub fn action_for(settings: &Settings, ev: &KeyEvent) -> Option<&'static str> {
    bindings(&settings.keybinds).into_iter().find(|(_, key)| chord(key).is_some_and(|c| c.matches(ev))).map(|(id, _)| id)
}

/// A recorded shortcut (Rune writes "ctrl+alt+shift+super+comma") in the stored form ("Mod+,").
pub fn from_shortcut(shortcut: &Shortcut) -> String { stored(&shortcut.to_string(), KeyPlatform::current()) }

fn stored(recorded: &str, platform: KeyPlatform) -> String {
    let mac = platform == KeyPlatform::Mac;
    let mut parts: Vec<&str> = recorded.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let has = |name: &str| parts.contains(&name);
    let mut out = Vec::new();
    if has(if mac { "super" } else { "ctrl" }) { out.push("Mod".to_string()); }
    if has(if mac { "ctrl" } else { "super" }) { out.push(if mac { "Ctrl" } else { "Super" }.to_string()); }
    if has("alt") { out.push("Alt".into()); }
    if has("shift") { out.push("Shift".into()); }
    out.push(match key {
        "comma" => ",".into(), "period" => ".".into(), "slash" => "/".into(), "semicolon" => ";".into(),
        "quote" => "'".into(), "bracketleft" => "[".into(), "bracketright" => "]".into(), "backslash" => "\\".into(),
        "minus" => "-".into(), "equal" => "=".into(), "backquote" => "`".into(),
        k => k[..1].to_uppercase() + &k[1..],
    });
    out.join("+")
}

/// The stored form as the recorder's type, to show the current shortcut in Settings.
pub fn to_shortcut(binding: &str) -> Option<Shortcut> {
    if binding.is_empty() { return None; }
    binding.to_lowercase().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Key, Mods};

    fn press(c: char, shift: bool) -> KeyEvent {
        KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, shift, ctrl: !cfg!(target_os = "macos"), logo: cfg!(target_os = "macos"), ..Mods::default() } }
    }

    #[test]
    fn defaults_run_and_overrides_replace_or_turn_them_off() {
        let mut s = Settings::default();
        assert_eq!(action_for(&s, &press('k', false)), Some("palette"));
        assert_eq!(action_for(&s, &press(',', false)), Some("settings"));
        assert_eq!(action_for(&s, &press('p', true)), Some("push"));
        assert_eq!(action_for(&s, &press('p', false)), None, "Shift is part of the chord");

        s.keybinds.insert("palette".into(), "Mod+J".into());
        s.keybinds.insert("sync".into(), String::new());
        s.keybinds.insert("push".into(), "Mod+C".into());
        assert_eq!(action_for(&s, &press('j', false)), Some("palette"));
        assert_eq!(action_for(&s, &press('k', false)), None);
        assert_eq!(action_for(&s, &press('r', false)), None, "\"\" turns a shortcut off");
        assert_eq!(action_for(&s, &press('p', true)), Some("push"), "a reserved override falls back to the default");
    }

    #[test]
    fn a_shortcut_needs_a_command_key_and_may_not_take_an_editing_one() {
        assert!(problem("Shift+K").unwrap().starts_with("Include"));
        assert!(problem("Mod+Shift+Z").unwrap().ends_with("is kept for editing and the window"));
        assert_eq!(problem("F5"), None);
        assert_eq!(problem("Ctrl+Alt+Up"), None);
        assert_eq!(problem("Mod+,"), None);
    }

    #[test]
    fn recorded_shortcuts_are_stored_in_the_settings_form() {
        assert_eq!(stored("shift+super+p", KeyPlatform::Mac), "Mod+Shift+P");
        assert_eq!(stored("ctrl+super+comma", KeyPlatform::Mac), "Mod+Ctrl+,");
        assert_eq!(stored("ctrl+alt+up", KeyPlatform::Linux), "Mod+Alt+Up");
        assert_eq!(stored("f5", KeyPlatform::Linux), "F5");
        for a in ACTIONS.iter().filter(|a| !a.key.is_empty()) {
            assert_eq!(from_shortcut(&to_shortcut(a.key).unwrap()), a.key);
            assert_eq!(problem(a.key), None);
        }
    }
}

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use std::collections::HashSet;

pub(super) fn parse(value: &str) -> Result<HotKey, String> {
    let value = value
        .split('+')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "win" | "windows" | "meta" => "Super",
            _ => part.trim(),
        })
        .collect::<Vec<_>>()
        .join("+");
    let key = value.parse::<HotKey>().map_err(|e| e.to_string())?;
    if key.mods.is_empty() {
        return Err("Use a modifier such as Alt, Control, Shift or Super with a key".into());
    }
    qt_key(key)?;
    Ok(key)
}

pub(crate) fn normalize(values: &[String]) -> Result<Vec<String>, String> {
    if values.len() > 8 {
        return Err("Use at most eight quick-search shortcuts".into());
    }
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| {
            let key = parse(value)?;
            if !seen.insert(key.id()) {
                return Err(format!("{} is listed more than once", label(value)));
            }
            Ok(key.to_string())
        })
        .collect()
}

pub(crate) fn label(value: &str) -> String {
    let Ok(key) = value.parse::<HotKey>() else {
        return value.into();
    };
    let mut parts: Vec<String> = Vec::new();
    if key.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".into());
    }
    if key.mods.contains(Modifiers::ALT) {
        parts.push(
            if cfg!(target_os = "macos") {
                "Option"
            } else {
                "Alt"
            }
            .into(),
        );
    }
    if key.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".into());
    }
    if key.mods.contains(Modifiers::SUPER) {
        parts.push(
            if cfg!(target_os = "macos") {
                "Command"
            } else if cfg!(windows) {
                "Win"
            } else {
                "Super"
            }
            .into(),
        );
    }
    let name = key.key.to_string();
    parts.push(
        name.trim_start_matches("Key")
            .trim_start_matches("Digit")
            .into(),
    );
    parts.join("+")
}

pub(super) fn qt_key(key: HotKey) -> Result<i32, String> {
    let name = key.key.to_string();
    let base =
        if let Some(letter) = name.strip_prefix("Key").filter(|s| s.len() == 1) {
            i32::from(letter.as_bytes()[0])
        } else if let Some(digit) = name.strip_prefix("Digit").filter(|s| s.len() == 1) {
            i32::from(digit.as_bytes()[0])
        } else if let Some(number) = name
            .strip_prefix('F')
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|n| (1..=24).contains(n))
        {
            0x01000030 + number - 1
        } else {
            match key.key {
                Code::Space => 0x20,
                Code::Enter => 0x01000004,
                Code::Tab => 0x01000001,
                Code::Escape => 0x01000000,
                Code::ArrowLeft => 0x01000012,
                Code::ArrowUp => 0x01000013,
                Code::ArrowRight => 0x01000014,
                Code::ArrowDown => 0x01000015,
                _ => return Err(
                    "Choose a letter, number, function key, Space, Enter, Tab, Escape or arrow key"
                        .into(),
                ),
            }
        };
    let mut result = base;
    for (modifier, mask) in [
        (Modifiers::SHIFT, 0x02000000),
        (Modifiers::CONTROL, 0x04000000),
        (Modifiers::ALT, 0x08000000),
        (Modifiers::SUPER, 0x10000000),
    ] {
        if key.mods.contains(modifier) {
            result |= mask;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_require_modifiers_and_reject_duplicate_aliases() {
        assert!(normalize(&["K".into()]).is_err());
        assert!(normalize(&["Alt+K".into(), "alt+KeyK".into()]).is_err());
        assert_eq!(
            normalize(&["Alt+K".into()]).unwrap(),
            [super::super::DEFAULT]
        );
        assert_eq!(qt_key(parse("Alt+K").unwrap()).unwrap(), 134217803);
        assert_eq!(qt_key(parse("Super+K").unwrap()).unwrap(), 268435531);
        assert_eq!(qt_key(parse("Win+K").unwrap()).unwrap(), 268435531);
        assert_eq!(
            normalize(&["Option+K".into()]).unwrap(),
            [super::super::DEFAULT]
        );
        assert_eq!(
            label("alt+KeyK"),
            if cfg!(target_os = "macos") {
                "Option+K"
            } else {
                "Alt+K"
            }
        );
        assert!(normalize(&[]).unwrap().is_empty());
    }
}

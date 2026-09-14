// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn effective_java_path_none_when_absent() {
    let paths = Paths {
        java_path: None,
        ..Paths::default()
    };
    assert!(paths.effective_java_path().is_none());
}

#[test]
fn effective_java_path_none_when_empty() {
    let paths = Paths {
        java_path: Some(String::new()),
        ..Paths::default()
    };
    assert!(paths.effective_java_path().is_none());
}

#[test]
fn effective_java_path_some_when_set() {
    let paths = Paths {
        java_path: Some("/usr/bin/java".to_owned()),
        ..Paths::default()
    };
    assert_eq!(paths.effective_java_path(), Some("/usr/bin/java"));
}

#[test]
fn content_provider_settings_are_normalized() {
    let content: Content =
        toml::from_str("preferred_provider = \"curseforge\"\npreferred_provider_only = true")
            .unwrap();
    assert_eq!(
        content.preferred_provider_with_curseforge(false),
        "modrinth"
    );
    assert_eq!(
        content.preferred_provider_with_curseforge(true),
        "curseforge"
    );
    assert!(content.preferred_provider_only);
    assert!(content.discovery_provider_enabled_with_curseforge("modrinth", false));
    assert!(!content.discovery_provider_enabled_with_curseforge("curseforge", false));
    assert!(!content.discovery_provider_enabled_with_curseforge("modrinth", true));
    assert!(content.discovery_provider_enabled_with_curseforge("curseforge", true));
    assert_eq!(
        content.discovery_provider_label_with_curseforge(false),
        "Modrinth"
    );
    assert_eq!(
        content.discovery_provider_label_with_curseforge(true),
        "CurseForge"
    );

    let merged = Content::default();
    assert_eq!(
        merged.discovery_provider_label_with_curseforge(false),
        "Modrinth"
    );
    assert_eq!(
        merged.discovery_provider_label_with_curseforge(true),
        "providers"
    );

    let legacy: Content = toml::from_str("preferred_provider = \"CurseForge\"").unwrap();
    assert_eq!(legacy.preferred_provider, ContentProvider::CurseForge);
    let unknown: Content = toml::from_str("preferred_provider = \"unknown\"").unwrap();
    assert_eq!(unknown.preferred_provider, ContentProvider::Modrinth);
}

#[test]
fn resolve_path_absolute() {
    assert_eq!(resolve_path("/opt/rmcl"), PathBuf::from("/opt/rmcl"));
}

#[test]
fn resolve_path_tilde_prefix() {
    let resolved = resolve_path("~/games/rmcl");
    assert!(!resolved.to_string_lossy().starts_with('~'));
    assert!(resolved.to_string_lossy().ends_with("games/rmcl"));
}

#[test]
fn resolve_path_bare_tilde() {
    let resolved = resolve_path("~");
    assert!(!resolved.to_string_lossy().starts_with('~'));
}

#[test]
fn config_deserializes_from_empty_toml() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.defaults.memory_max, "2G");
}

#[test]
fn config_deserializes_partial_toml() {
    let toml_str = r#"
[general]
debug = true

[defaults]
memory_max = "8G"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.defaults.memory_max, "8G");
    assert_eq!(config.defaults.memory_min, "512M");
}

#[test]
fn shortcut_hints_support_global_main_and_selective_hiding() {
    let mut ui: Ui = toml::from_str("hidden_shortcut_hints = []").unwrap();
    assert!(
        ShortcutHintScope::AREAS
            .iter()
            .all(|scope| ui.show_shortcut_hints(*scope))
    );

    ui = toml::from_str("hidden_shortcut_hints = [\"main\"]").unwrap();
    assert!(!ui.show_shortcut_hints(ShortcutHintScope::Instances));
    assert!(ui.show_shortcut_hints(ShortcutHintScope::Popups));

    ui.set_shortcut_hints(ShortcutHintScope::Content, true);
    assert!(ui.show_shortcut_hints(ShortcutHintScope::Content));
    assert!(!ui.show_shortcut_hints(ShortcutHintScope::Accounts));

    ui.hidden_shortcut_hints = [ShortcutHintScope::All].into_iter().collect();
    assert!(
        ShortcutHintScope::AREAS
            .iter()
            .all(|scope| !ui.show_shortcut_hints(*scope))
    );
}

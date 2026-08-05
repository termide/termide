//! Integration tests for diff-against-baseline saves and the layered
//! `defaults → global → project` overlay model.
//!
//! Tests use `tempfile` instead of redirecting XDG so they exercise the
//! same `save_to` / `merge_partial` primitives the running app uses,
//! without depending on the user's real config directory.

use std::fs;
use tempfile::TempDir;
use termide_config::{merge_partial, project_config_path, Config};
use toml::Value;

fn read(path: &std::path::Path) -> String {
    fs::read_to_string(path).expect("read file")
}

#[test]
fn save_to_default_produces_empty_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    Config::default()
        .save_to(&path, &Config::default())
        .unwrap();
    let content = read(&path);
    assert!(
        content.trim().is_empty(),
        "expected empty content for default vs default diff, got:\n{}",
        content
    );
}

#[test]
fn save_to_keeps_only_diff_against_default() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let mut cfg = Config::default();
    cfg.editor.tab_size = 8;
    cfg.save_to(&path, &Config::default()).unwrap();
    let content = read(&path);
    assert!(content.contains("tab_size = 8"), "content:\n{}", content);
    // The diff stays minimal: nothing else from [editor] should appear.
    assert!(
        !content.contains("vim_mode"),
        "diff should not include unchanged [editor] fields, got:\n{}",
        content
    );
}

#[test]
fn save_global_roundtrip_through_load_from() {
    // Round-trip via `load_from` (which re-deserializes the partial TOML
    // and normalizes). The reloaded config should equal what we saved.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let mut cfg = Config::default();
    cfg.editor.tab_size = 8;
    cfg.general.theme = "dark".into();
    cfg.save_to(&path, &Config::default()).unwrap();

    let loaded = Config::load_from(&path).unwrap();
    assert_eq!(loaded.editor.tab_size, 8);
    assert_eq!(loaded.general.theme, "dark");
    // Untouched fields fall back to defaults.
    let default = Config::default();
    assert_eq!(loaded.general.language, default.general.language);
    assert_eq!(loaded.editor.word_wrap, default.editor.word_wrap);
}

#[test]
fn save_project_empty_when_matches_global() {
    let dir = TempDir::new().unwrap();
    let project_root = dir.path();

    // global_layer: defaults with tab_size=2.
    let mut global_layer = Config::default();
    global_layer.editor.tab_size = 2;

    // Effective config matches global_layer — nothing project-specific.
    global_layer
        .clone()
        .save_project(project_root, &global_layer)
        .unwrap();

    let path = project_config_path(project_root);
    let content = read(&path);
    assert!(
        content.trim().is_empty(),
        "project file should be empty when effective == global, got:\n{}",
        content
    );
}

#[test]
fn save_project_records_only_deviation_from_global() {
    let dir = TempDir::new().unwrap();
    let project_root = dir.path();

    // global_layer: tab_size=2.
    let mut global_layer = Config::default();
    global_layer.editor.tab_size = 2;

    // Effective: same as global except tab_size=8.
    let mut effective = global_layer.clone();
    effective.editor.tab_size = 8;
    effective.save_project(project_root, &global_layer).unwrap();

    let path = project_config_path(project_root);
    let content = read(&path);
    assert!(
        content.contains("tab_size = 8"),
        "project file should contain the override, got:\n{}",
        content
    );
    // The default tab_size (4) and the global override (2) must NOT appear.
    assert!(!content.contains("tab_size = 2"));
    assert!(!content.contains("tab_size = 4"));
}

#[test]
fn manual_layered_overlay_resolves_in_priority_order() {
    // Simulates what `Config::load_layered` does step-by-step. Order:
    // defaults → global → project. The project layer must win over the
    // global layer where they conflict.
    let mut value = Value::try_from(Config::default()).unwrap();

    // Global: theme=dark, tab_size=2.
    let global: Value =
        toml::from_str("[general]\ntheme = \"dark\"\n[editor]\ntab_size = 2\n").unwrap();
    merge_partial(&mut value, &global);

    // Project: tab_size=8 (wins over global), word_wrap=false (new override).
    let project: Value = toml::from_str("[editor]\ntab_size = 8\nword_wrap = false\n").unwrap();
    merge_partial(&mut value, &project);

    let mut cfg: Config = value.try_into().unwrap();
    cfg.normalize();

    assert_eq!(cfg.editor.tab_size, 8); // project wins
    assert!(!cfg.editor.word_wrap); // project sets
    assert_eq!(cfg.general.theme, "dark"); // global preserved

    // Untouched defaults survive.
    assert_eq!(cfg.general.language, Config::default().general.language);
}

#[test]
fn command_less_lsp_server_does_not_poison_the_whole_config() {
    // Regression: a project overlay with an lsp server that omits the (now
    // defaulted) `command` field must still deserialize — keeping every other
    // setting — instead of failing `try_into` and discarding the whole
    // document back to defaults.
    let mut value = Value::try_from(Config::default()).unwrap();

    let project: Value =
        toml::from_str("[editor]\ntab_size = 8\n[lsp.servers.broken]\nargs = [\"--stdio\"]\n")
            .unwrap();
    merge_partial(&mut value, &project);

    let cfg: Config = value
        .try_into()
        .expect("a command-less lsp server must not fail config parsing");

    assert_eq!(cfg.editor.tab_size, 8); // sibling setting preserved
    assert_eq!(cfg.lsp.servers["broken"].command, ""); // command defaulted
    assert_eq!(cfg.lsp.servers["broken"].args, vec!["--stdio".to_string()]);
}

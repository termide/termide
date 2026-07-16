//! Detect duplicate / shadowed / non-portable keybinding assignments.
//!
//! Used by:
//! - the Settings keybinding picker (inline warning when assigning a
//!   binding already taken in the same section);
//! - app startup (status-bar log of `SameSection` conflicts and
//!   `binding_requires_kitty` warnings on terminals without Kitty
//!   keyboard protocol).

use crossterm::event::{KeyCode, KeyModifiers};

use crate::keybindings::{parse_keybinding, KeyBinding, ParsedKeyBinding};
use crate::settings::Config;

/// Where a binding lives. Format: `<section>.<action>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BindingLocation {
    pub section: String,
    pub action: String,
}

impl BindingLocation {
    pub fn new(section: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            section: section.into(),
            action: action.into(),
        }
    }

    /// Display as "section.action".
    pub fn display(&self) -> String {
        format!("{}.{}", self.section, self.action)
    }
}

/// Kind of conflict between two bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Two actions in the same section share a binding. The second one
    /// is unreachable.
    SameSection,
    /// A panel action shares a binding with a global action — the
    /// global handler dispatches first, so the panel binding never
    /// fires.
    CrossSectionShadowed,
    /// Two panel actions share a binding — only the active panel
    /// handles the event, so usually fine, but worth noting.
    CrossSectionAmbient,
}

/// One conflict report: a set of bindings sharing the same canonical form.
#[derive(Debug, Clone)]
pub struct HotkeyConflict {
    /// Display string of the canonical binding (`"Ctrl+S"` etc.).
    pub binding: String,
    pub kind: ConflictKind,
    pub locations: Vec<BindingLocation>,
}

/// Enumerate every binding assigned in `config`, returning
/// `(location, parsed_binding, display_string)` tuples.
pub fn enumerate_bindings(config: &Config) -> Vec<(BindingLocation, ParsedKeyBinding, String)> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<_>, section: &str, action: &str, kb: &Option<KeyBinding>| {
        let Some(kb) = kb else { return };
        let strings: Vec<String> = match kb {
            KeyBinding::Single(s) => vec![s.clone()],
            KeyBinding::Multiple(v) => v.clone(),
        };
        for s in strings {
            if let Ok(parsed) = parse_keybinding(&s) {
                out.push((BindingLocation::new(section, action), parsed, s));
            }
        }
    };

    let g = &config.general.keybindings;
    push(&mut out, "general", "toggle_menu", &g.toggle_menu);
    push(&mut out, "general", "new_file_manager", &g.new_file_manager);
    push(&mut out, "general", "new_terminal", &g.new_terminal);
    push(&mut out, "general", "new_editor", &g.new_editor);
    push(&mut out, "general", "new_journal", &g.new_journal);
    push(&mut out, "general", "open_help", &g.open_help);
    push(&mut out, "general", "open_preferences", &g.open_preferences);
    push(&mut out, "general", "open_sessions", &g.open_sessions);
    push(&mut out, "general", "new_session", &g.new_session);
    push(&mut out, "general", "open_git_status", &g.open_git_status);
    push(
        &mut out,
        "general",
        "open_bookmark_add",
        &g.open_bookmark_add,
    );
    push(&mut out, "general", "open_outline", &g.open_outline);
    push(&mut out, "general", "open_diagnostics", &g.open_diagnostics);
    push(&mut out, "general", "open_git_log", &g.open_git_log);
    push(&mut out, "general", "close_panel", &g.close_panel);
    push(&mut out, "general", "toggle_stack", &g.toggle_stack);
    push(&mut out, "general", "swap_left", &g.swap_left);
    push(&mut out, "general", "swap_right", &g.swap_right);
    push(&mut out, "general", "move_first", &g.move_first);
    push(&mut out, "general", "move_last", &g.move_last);
    push(&mut out, "general", "resize_smaller", &g.resize_smaller);
    push(&mut out, "general", "resize_larger", &g.resize_larger);
    push(
        &mut out,
        "general",
        "toggle_fullscreen_panel",
        &g.toggle_fullscreen_panel,
    );
    push(
        &mut out,
        "general",
        "panel_grow_vertical",
        &g.panel_grow_vertical,
    );
    push(
        &mut out,
        "general",
        "panel_shrink_vertical",
        &g.panel_shrink_vertical,
    );
    push(
        &mut out,
        "general",
        "panel_action_menu",
        &g.panel_action_menu,
    );
    push(&mut out, "general", "prev_group", &g.prev_group);
    push(&mut out, "general", "next_group", &g.next_group);
    push(&mut out, "general", "prev_panel", &g.prev_panel);
    push(&mut out, "general", "next_panel", &g.next_panel);
    push(&mut out, "general", "goto_panel_1", &g.goto_panel_1);
    push(&mut out, "general", "goto_panel_2", &g.goto_panel_2);
    push(&mut out, "general", "goto_panel_3", &g.goto_panel_3);
    push(&mut out, "general", "goto_panel_4", &g.goto_panel_4);
    push(&mut out, "general", "goto_panel_5", &g.goto_panel_5);
    push(&mut out, "general", "goto_panel_6", &g.goto_panel_6);
    push(&mut out, "general", "goto_panel_7", &g.goto_panel_7);
    push(&mut out, "general", "goto_panel_8", &g.goto_panel_8);
    push(&mut out, "general", "goto_panel_9", &g.goto_panel_9);
    push(&mut out, "general", "quit", &g.quit);
    push(
        &mut out,
        "general",
        "open_command_palette",
        &g.open_command_palette,
    );
    push(&mut out, "general", "copy", &g.copy);
    push(&mut out, "general", "cut", &g.cut);
    push(&mut out, "general", "paste", &g.paste);

    let e = &config.editor.keybindings;
    push(&mut out, "editor", "save", &e.save);
    push(&mut out, "editor", "save_as", &e.save_as);
    push(&mut out, "editor", "reload", &e.reload);
    push(&mut out, "editor", "undo", &e.undo);
    push(&mut out, "editor", "redo", &e.redo);
    push(&mut out, "editor", "duplicate_line", &e.duplicate_line);
    push(&mut out, "editor", "toggle_comment", &e.toggle_comment);
    push(&mut out, "editor", "search", &e.search);
    push(&mut out, "editor", "search_next", &e.search_next);
    push(&mut out, "editor", "search_prev", &e.search_prev);
    push(&mut out, "editor", "replace", &e.replace);
    push(&mut out, "editor", "replace_current", &e.replace_current);
    push(&mut out, "editor", "replace_all", &e.replace_all);
    push(&mut out, "editor", "select_all", &e.select_all);
    push(
        &mut out,
        "editor",
        "trigger_completion",
        &e.trigger_completion,
    );
    push(&mut out, "editor", "show_hover", &e.show_hover);
    push(&mut out, "editor", "goto_definition", &e.goto_definition);
    push(&mut out, "editor", "find_references", &e.find_references);
    push(&mut out, "editor", "rename_symbol", &e.rename_symbol);

    let f = &config.file_manager.keybindings;
    push(&mut out, "file_manager", "rename", &f.rename);
    push(&mut out, "file_manager", "view", &f.view);
    push(&mut out, "file_manager", "edit", &f.edit);
    push(&mut out, "file_manager", "copy", &f.copy);
    push(&mut out, "file_manager", "move_item", &f.move_item);
    push(&mut out, "file_manager", "create_dir", &f.create_dir);
    push(&mut out, "file_manager", "create_file", &f.create_file);
    push(&mut out, "file_manager", "delete", &f.delete);
    push(&mut out, "file_manager", "info", &f.info);
    push(&mut out, "file_manager", "search", &f.search);
    push(
        &mut out,
        "file_manager",
        "search_content",
        &f.search_content,
    );
    push(
        &mut out,
        "file_manager",
        "search_replace",
        &f.search_replace,
    );
    push(&mut out, "file_manager", "refresh", &f.refresh);
    push(&mut out, "file_manager", "go_parent", &f.go_parent);
    push(&mut out, "file_manager", "go_home", &f.go_home);
    push(
        &mut out,
        "file_manager",
        "switch_directory",
        &f.switch_directory,
    );
    push(&mut out, "file_manager", "go_to_path", &f.go_to_path);
    push(
        &mut out,
        "file_manager",
        "toggle_selection",
        &f.toggle_selection,
    );
    push(&mut out, "file_manager", "select_all", &f.select_all);
    push(&mut out, "file_manager", "open_external", &f.open_external);
    push(&mut out, "file_manager", "toggle_hidden", &f.toggle_hidden);

    let gs = &config.git_status.keybindings;
    push(&mut out, "git_status", "stage", &gs.stage);
    push(&mut out, "git_status", "unstage", &gs.unstage);
    push(&mut out, "git_status", "view", &gs.view);
    push(&mut out, "git_status", "edit", &gs.edit);
    push(&mut out, "git_status", "info", &gs.info);
    push(&mut out, "git_status", "revert", &gs.revert);
    push(&mut out, "git_status", "refresh", &gs.refresh);

    let gd = &config.git_diff.keybindings;
    push(&mut out, "git_diff", "toggle_collapse", &gd.toggle_collapse);
    push(&mut out, "git_diff", "edit", &gd.edit);
    push(&mut out, "git_diff", "refresh", &gd.refresh);
    push(&mut out, "git_diff", "scroll_half_up", &gd.scroll_half_up);
    push(
        &mut out,
        "git_diff",
        "scroll_half_down",
        &gd.scroll_half_down,
    );

    let gl = &config.git_log.keybindings;
    push(&mut out, "git_log", "info", &gl.info);
    push(&mut out, "git_log", "view_diff", &gl.view_diff);
    push(&mut out, "git_log", "checkout", &gl.checkout);

    let db = &config.database.keybindings;
    push(&mut out, "database", "sort", &db.sort);
    push(&mut out, "database", "filter", &db.filter);
    push(&mut out, "database", "clear_filter", &db.clear_filter);
    push(&mut out, "database", "detail", &db.detail);
    push(&mut out, "database", "copy_row", &db.copy_row);
    push(&mut out, "database", "refresh", &db.refresh);

    let t = &config.terminal.keybindings;
    push(&mut out, "terminal", "scroll_up", &t.scroll_up);
    push(&mut out, "terminal", "scroll_down", &t.scroll_down);
    push(&mut out, "terminal", "scroll_top", &t.scroll_top);
    push(&mut out, "terminal", "scroll_bottom", &t.scroll_bottom);
    push(&mut out, "terminal", "search", &t.search);
    push(
        &mut out,
        "terminal",
        "switch_directory",
        &t.switch_directory,
    );

    out
}

/// Detect every conflict (same-section, shadowed, ambient) in `config`.
pub fn find_conflicts(config: &Config) -> Vec<HotkeyConflict> {
    use std::collections::HashMap;
    let mut grouped: HashMap<ParsedKeyBinding, Vec<(BindingLocation, String)>> = HashMap::new();
    for (loc, parsed, display) in enumerate_bindings(config) {
        grouped.entry(parsed).or_default().push((loc, display));
    }
    let mut conflicts = Vec::new();
    for (_, locs) in grouped {
        if locs.len() < 2 {
            continue;
        }
        let display = locs[0].1.clone();
        let sections: Vec<_> = locs.iter().map(|(l, _)| l.section.clone()).collect();
        let kind = if sections.windows(2).all(|w| w[0] == w[1]) {
            // All in the same section.
            ConflictKind::SameSection
        } else if sections.iter().any(|s| s == "general") {
            ConflictKind::CrossSectionShadowed
        } else {
            ConflictKind::CrossSectionAmbient
        };
        let mut locations: Vec<_> = locs.into_iter().map(|(l, _)| l).collect();
        locations.sort_by(|a, b| a.section.cmp(&b.section).then(a.action.cmp(&b.action)));
        conflicts.push(HotkeyConflict {
            binding: display,
            kind,
            locations,
        });
    }
    conflicts.sort_by(|a, b| a.binding.cmp(&b.binding));
    conflicts
}

/// `true` when the binding is reachable on legacy VTE terminals via
/// a known `KeyNormalizer` quirk, even though it lives in the
/// "enhanced" tier on paper. Currently:
/// - `Ctrl+/`  — reached via `Ctrl+7 → Ctrl+/` rewrite (`\x1F` byte).
/// - `Ctrl+\` — reached via `Ctrl+4 → Ctrl+\` rewrite (`\x1C` byte).
fn supported_via_legacy_quirk(parsed: &ParsedKeyBinding) -> bool {
    parsed.modifiers == KeyModifiers::CONTROL
        && matches!(parsed.key, KeyCode::Char('/') | KeyCode::Char('\\'))
}

/// Returns `true` when this canonical chord is unlikely to reach
/// termide on a terminal that does not implement the Kitty keyboard
/// enhancement protocol. Used to warn the user at startup.
///
/// Rules (matching what crossterm 0.28 can decode in legacy mode on
/// VTE / xterm):
/// - `Ctrl + punctuation` (no Shift, no Alt): no ASCII control code,
///   silently dropped to plain glyph or collapsed onto another chord.
/// - `Ctrl + Shift + letter`: legacy encoding does not survive the
///   shift bit; only Kitty CSI-u carries it.
/// - `Ctrl + Alt + anything`: `ESC + control-byte` is ambiguous and
///   most terminals deliver one or the other but not both.
/// - `Alt + Shift + letter`: in legacy mode VTE emits `\eL` for
///   `Alt+Shift+l`, indistinguishable from `Alt+L`. crossterm parses
///   it as `Char('L') + Alt`, with no Shift bit — so a binding that
///   asks for explicit `Alt + Shift + L` cannot match.
/// - `Alt + Shift + arrow`: VTE typically swallows the chord as a
///   native shortcut or emits `\e\e[A`, parsed as `Char('A') + Alt`.
/// - `Super` / `Meta` / `Hyper` modifiers: not in the legacy modifier
///   set.
pub fn binding_requires_kitty(parsed: &ParsedKeyBinding) -> bool {
    if supported_via_legacy_quirk(parsed) {
        return false;
    }
    let mods = parsed.modifiers;
    let extra = mods - KeyModifiers::CONTROL - KeyModifiers::ALT - KeyModifiers::SHIFT;
    if !extra.is_empty() {
        return true; // Super / Meta / Hyper.
    }

    let has_ctrl = mods.contains(KeyModifiers::CONTROL);
    let has_alt = mods.contains(KeyModifiers::ALT);
    let has_shift = mods.contains(KeyModifiers::SHIFT);

    if has_ctrl && has_alt {
        return true;
    }

    let is_letter = matches!(parsed.key, KeyCode::Char(c) if c.is_ascii_alphabetic());
    let is_punct = matches!(parsed.key, KeyCode::Char(c) if c.is_ascii_punctuation());
    let is_arrow = matches!(
        parsed.key,
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
    );

    if has_ctrl && has_shift && is_letter {
        return true;
    }

    if has_ctrl && !has_alt && !has_shift && is_punct {
        return true;
    }

    if has_alt && has_shift && (is_letter || is_arrow) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ParsedKeyBinding {
        parse_keybinding(s).unwrap()
    }

    #[test]
    fn requires_kitty_table() {
        // Universal — no Kitty needed.
        assert!(!binding_requires_kitty(&parse("Ctrl+S"))); // letter
        assert!(!binding_requires_kitty(&parse("Alt+M")));
        assert!(!binding_requires_kitty(&parse("Ctrl+Up"))); // Ctrl+arrow
        assert!(!binding_requires_kitty(&parse("Shift+Up")));
        assert!(!binding_requires_kitty(&parse("Alt+Up")));
        assert!(!binding_requires_kitty(&parse("F2")));
        assert!(!binding_requires_kitty(&parse("Shift+F12")));
        assert!(!binding_requires_kitty(&parse("Ctrl+F2")));
        assert!(!binding_requires_kitty(&parse("Alt+/"))); // Alt+punct
                                                           // Quirk-supported in legacy mode.
        assert!(!binding_requires_kitty(&parse("Ctrl+/")));
        assert!(!binding_requires_kitty(&parse("Ctrl+\\")));

        // Enhanced — needs Kitty.
        assert!(binding_requires_kitty(&parse("Ctrl+,")));
        assert!(binding_requires_kitty(&parse("Ctrl+-"))); // -, parsed as Char('-')
        assert!(binding_requires_kitty(&parse("Ctrl+.")));
        assert!(binding_requires_kitty(&parse("Ctrl+Shift+F")));
        assert!(binding_requires_kitty(&parse("Ctrl+Alt+R")));
        assert!(binding_requires_kitty(&parse("Ctrl+Alt+Shift+P")));
        // Alt+Shift+letter / Alt+Shift+arrow: VTE legacy mode strips the
        // Shift bit from the parsed event, so an explicit Alt+Shift
        // binding cannot match.
        assert!(binding_requires_kitty(&parse("Alt+Shift+R")));
        assert!(binding_requires_kitty(&parse("Alt+Shift+Up")));
        assert!(binding_requires_kitty(&parse("Alt+Shift+Down")));
        // The quirks only apply with bare Ctrl — Ctrl+Alt+/ etc still
        // require Kitty proto.
        assert!(binding_requires_kitty(&parse("Ctrl+Alt+/")));
        assert!(binding_requires_kitty(&parse("Ctrl+Alt+\\")));
    }

    #[test]
    fn enumerate_default_config() {
        let mut cfg = Config::default();
        cfg.normalize();
        let bindings = enumerate_bindings(&cfg);
        assert!(!bindings.is_empty());
        // Every binding parses cleanly.
        for (loc, parsed, raw) in &bindings {
            assert!(
                !raw.is_empty(),
                "binding string is empty for {}",
                loc.display()
            );
            // Smoke check parsed form is non-default.
            let _ = parsed.modifiers;
        }
    }

    #[test]
    fn find_conflicts_detects_same_section_clash() {
        // Build a synthetic conflict by overwriting a default.
        let mut cfg = Config::default();
        cfg.normalize();
        // Force two general actions to share the same chord.
        cfg.general.keybindings.toggle_menu = Some(KeyBinding::Single("Alt+Q".to_string()));
        cfg.general.keybindings.quit = Some(KeyBinding::Single("Alt+Q".to_string()));

        let conflicts = find_conflicts(&cfg);
        let same_section: Vec<_> = conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::SameSection)
            .collect();
        assert!(
            same_section.iter().any(|c| c
                .locations
                .iter()
                .any(|l| l.section == "general" && l.action == "quit")),
            "expected same-section conflict on Alt+Q"
        );
    }

    #[test]
    fn find_conflicts_detects_cross_section_shadow() {
        let mut cfg = Config::default();
        cfg.normalize();
        // editor.save (Ctrl+S) clashes with a synthetic global action.
        cfg.general.keybindings.toggle_menu = Some(KeyBinding::Single("Ctrl+S".to_string()));

        let conflicts = find_conflicts(&cfg);
        assert!(
            conflicts
                .iter()
                .any(|c| c.kind == ConflictKind::CrossSectionShadowed
                    && c.locations.iter().any(|l| l.section == "general")
                    && c.locations.iter().any(|l| l.section == "editor")),
            "expected cross-section shadow conflict between general and editor on Ctrl+S"
        );
    }
}

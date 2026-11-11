---
name: add-setting
description: Add a user setting to termide end to end — the config field and its default, the settings window field, the UI strings in 15 languages, applying it at startup and on change, tests, docs and the changelog fragment. Use when asked to make something configurable, add a config key, or expose an option on the settings window.
---

# Add a setting

A setting touches about a dozen places, and a missed one fails silently: a
field that is never rendered, a label that is blank, a value read only at
startup. Work through them in order. Reference commit: `b2044642`
(`feat(terminal): make the scrollback length a setting`).

## 1. The config field — `crates/config/src/settings.rs`

- Add the field to its section struct (`TerminalSettings`, `EditorSettings`,
  `AiSettings`, …) with a doc comment that says what it does and when a
  change takes effect.
- Give it a default the way the struct already does: `#[serde(default)]`
  for a type's own default, `#[serde(default = "default_x")]` plus a
  `fn default_x()` otherwise. A struct that derives `Default` while a field
  has a non-trivial default needs a hand-written `impl Default` — the
  derived one would disagree with serde.
- Loading merges the files into the serialized defaults, so an old config
  without the key must keep working. If the key replaces an older form,
  accept both (a custom `Deserialize`, as `SideModel` does) rather than drop
  the old one.
- Test that an empty section and a section without the key read the
  default, and that the key is read (`scrollback_lines_default_to_…`).

## 2. The settings window — `crates/modal/src/settings/`

- `fields.rs`, `fields_for_tab`: append a `FieldDescriptor { label, field_type }`
  to the tab. `FieldType` is `Bool`, `Number`, `Enum`, `OptionalText` or
  `OptionalNumber`. **Append**: fields are addressed by index everywhere, and
  inserting shifts every later index.
- `fields.rs`, `get_field_value`: the value as shown, by index.
- By type, where the value is written back:
  - `Bool` → `toggle_field`;
  - `Enum` → `enum_options`, `apply_enum_value`, `cycle_enum_forward` /
    `cycle_enum_backward`;
  - `Number` → `state.rs` `apply_number`; `OptionalNumber` →
    `apply_optional_number`; `OptionalText` → `apply_text`.
- `state.rs`, `content_rows`: add `Field(i)` to the tab's rows, under the
  right `Header`. Nothing ties the rows to the descriptors;
  `every_declared_field_is_rendered_somewhere` fails when you forget, and
  fails too when the field count of a tab is asserted elsewhere — update it.
- Test editing the field through the modal (`start_edit`, set
  `edit_input`, `commit_edit`, then read `modal.config`), as
  `the_scrollback_field_edits_the_terminal_setting` does.

## 3. UI strings — `crates/i18n/`

- Add the key (`settings_<tab>_<field>`) to all 15 files in `i18n/` next to
  its neighbours, translated; the `Translation` trait in `src/lib.rs`; and
  the key list in `src/runtime.rs`.
- A string with a `{placeholder}` goes under `[formats]`, not `[strings]`,
  in every file; under `[strings]` it renders empty.
- `every_language_has_every_key` and the placeholder tests in
  `runtime.rs` check completeness: run `cargo nextest run -p termide-i18n`.

## 4. Applying it

- Where the value is used, read it from the config the code already has.
  For process-wide state outside the config (a static the panels read),
  publish it in `crates/app/src/app/mod.rs` in three places: both startup
  paths (`App::new…`, next to `publish_shell`) and the settings-change path
  (after `if config.terminal.default_shell != old.terminal.default_shell`),
  so a change applies without a restart.
- Say in the doc comment and the docs whether open panels take the change
  or only new ones do.

## 5. Docs and changelog

- The key and its default in the config section of the matching
  `doc/en/<area>.md`, and the same in `doc/ru/` (`doc/zh/` when the change is
  a line).
- A fragment `changelog.d/added/<kebab-title>.md`: one bullet, the effect
  on the user, the key, where it is on the settings window
  (see `changelog.d/README.md`).

## 6. Validate

```
cargo nextest run -p termide-config -p termide-modal -p termide-i18n
cargo clippy -p termide-config -p termide-modal -p termide-app --all-targets -- -D warnings
```

Then open the settings window through `tools/tui-harness` or by hand and
check the field shows, edits and saves.

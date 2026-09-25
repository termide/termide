//! Field descriptors and per-field helpers for the settings modal.
//!
//! This module holds the pure data side of settings — which fields exist in
//! each tab, how to read/write them on a `Config`, and the type markers used
//! by the renderer. It deliberately contains no UI state or rendering logic.

use termide_config::Config;
use termide_i18n as i18n;
use termide_theme::Theme;

use super::SettingsTab;

/// Type of a settings field for rendering and editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldType {
    /// Boolean toggle — [✓] / [✗]
    Bool,
    /// Unsigned integer (u16, u32, u64, usize)
    Number,
    /// Enum cycling through a fixed list of variants
    Enum,
    /// Optional string — shows "(auto)" placeholder when None
    OptionalText,
    /// Optional unsigned integer: an empty or zero entry clears it back to
    /// None. The None display is chosen per field (e.g. a default value).
    OptionalNumber,
}

/// Descriptor for a single settings field.
#[derive(Clone, Copy)]
pub(super) struct FieldDescriptor {
    pub label: &'static str,
    pub field_type: FieldType,
}

/// A single renderable row in the content area.
#[derive(Debug, Clone, Copy)]
pub(super) enum ContentRow {
    /// Non-selectable group header.
    Header(&'static str),
    /// Non-selectable blank row used as a spacer between groups.
    Spacer,
    /// A scalar field (index into `fields_for_tab`).
    Field(usize),
    /// LSP: "+ Add server" action row.
    LspAddServer,
    /// LSP: existing server (index into `lsp_server_keys`).
    LspServer(usize),
}

impl ContentRow {
    pub(super) fn is_selectable(&self) -> bool {
        !matches!(self, ContentRow::Header(_) | ContentRow::Spacer)
    }
}

/// Returns the field descriptors for a given tab.
pub(super) fn fields_for_tab(tab: SettingsTab) -> Vec<FieldDescriptor> {
    let t = i18n::t();
    match tab {
        SettingsTab::General => vec![
            FieldDescriptor {
                label: t.settings_general_vim_mode(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_general_theme(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_general_language(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_general_icon_mode(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_general_auto_stack_threshold(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_general_min_panel_width(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_general_project_retention(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_general_bell(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_general_resource_interval(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_general_always_detachable(),
                field_type: FieldType::Bool,
            },
        ],
        SettingsTab::Editor => vec![
            FieldDescriptor {
                label: t.settings_editor_tab_size(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_editor_word_wrap(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_editor_auto_indent(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_editor_auto_close_brackets(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_editor_show_git_diff(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_editor_show_blame(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_editor_large_file_threshold(),
                field_type: FieldType::Number,
            },
        ],
        SettingsTab::FileManager => vec![
            FieldDescriptor {
                label: t.settings_fm_extended_view_width(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_fm_content_search_max_size(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_fm_dir_size_in_wide_view(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_fm_dir_size_budget_ms(),
                field_type: FieldType::Number,
            },
        ],
        SettingsTab::Terminal => vec![FieldDescriptor {
            label: t.settings_terminal_default_shell(),
            field_type: FieldType::OptionalText,
        }],
        SettingsTab::Lsp => vec![
            FieldDescriptor {
                label: t.settings_lsp_enabled(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_lsp_auto_completion(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_lsp_completion_delay(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_lsp_hover_delay(),
                field_type: FieldType::Number,
            },
        ],
        SettingsTab::Logging => vec![
            FieldDescriptor {
                label: t.settings_logging_file_path(),
                field_type: FieldType::OptionalText,
            },
            FieldDescriptor {
                label: t.settings_logging_min_level(),
                field_type: FieldType::Enum,
            },
        ],
        SettingsTab::Vfs => vec![FieldDescriptor {
            label: t.settings_vfs_connection_timeout(),
            field_type: FieldType::Number,
        }],
        SettingsTab::Ai => vec![
            FieldDescriptor {
                label: t.settings_agent_provider(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_agent_base_url(),
                field_type: FieldType::OptionalText,
            },
            FieldDescriptor {
                // A dropdown: the endpoint's models when the panel has fetched
                // them (with a "type an id" escape), just the escape otherwise.
                label: t.settings_agent_model(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_agent_api_key_env(),
                field_type: FieldType::OptionalText,
            },
            FieldDescriptor {
                label: t.settings_agent_context_window(),
                field_type: FieldType::OptionalNumber,
            },
            FieldDescriptor {
                label: t.settings_agent_max_tokens(),
                field_type: FieldType::Number,
            },
            FieldDescriptor {
                label: t.settings_agent_reasoning(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_agent_autofold(),
                field_type: FieldType::Bool,
            },
            FieldDescriptor {
                label: t.settings_web_backend(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_web_engine(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_web_display(),
                field_type: FieldType::Enum,
            },
            FieldDescriptor {
                label: t.settings_web_chrome_path(),
                field_type: FieldType::OptionalText,
            },
            FieldDescriptor {
                label: t.settings_agent_permission_mode(),
                field_type: FieldType::Enum,
            },
        ],
        SettingsTab::Keybindings => vec![],
    }
}

/// Returns the current string value of a field.
pub(super) fn get_field_value(config: &Config, tab: SettingsTab, index: usize) -> String {
    match tab {
        SettingsTab::General => match index {
            0 => bool_str(config.general.vim_mode),
            1 => config.general.theme.clone(),
            2 => i18n::get_language_name(&config.general.language)
                .map(|s| s.to_string())
                .unwrap_or_else(|| config.general.language.clone()),
            3 => format!("{:?}", config.general.icon_mode).to_lowercase(),
            4 => config.general.auto_stack_threshold.to_string(),
            5 => config.general.min_panel_width.to_string(),
            6 => config.general.project_retention_days.to_string(),
            7 => bool_str(config.general.bell_on_operation_complete),
            8 => config.general.resource_monitor_interval.to_string(),
            9 => bool_str(config.general.always_detachable),
            _ => String::new(),
        },
        SettingsTab::Editor => match index {
            0 => config.editor.tab_size.to_string(),
            1 => bool_str(config.editor.word_wrap),
            2 => bool_str(config.editor.auto_indent),
            3 => bool_str(config.editor.auto_close_brackets),
            4 => bool_str(config.editor.show_git_diff),
            5 => bool_str(config.editor.show_blame),
            6 => config.editor.large_file_threshold_mb.to_string(),
            _ => String::new(),
        },
        SettingsTab::FileManager => match index {
            0 => config.file_manager.extended_view_width.to_string(),
            1 => config
                .file_manager
                .content_search_max_file_size_mb
                .to_string(),
            2 => bool_str(config.file_manager.dir_size_in_wide_view),
            3 => config.file_manager.dir_size_budget_ms.to_string(),
            _ => String::new(),
        },
        SettingsTab::Terminal => match index {
            0 => config
                .terminal
                .default_shell
                .clone()
                .unwrap_or_else(|| "(auto)".to_string()),
            _ => String::new(),
        },
        SettingsTab::Lsp => match index {
            0 => bool_str(config.lsp.enabled),
            1 => bool_str(config.lsp.auto_completion),
            2 => config.lsp.completion_delay_ms.to_string(),
            3 => config.lsp.hover_delay_ms.to_string(),
            _ => String::new(),
        },
        SettingsTab::Logging => match index {
            0 => config
                .logging
                .file_path
                .clone()
                .unwrap_or_else(|| "(none)".to_string()),
            1 => config.logging.min_level.clone(),
            _ => String::new(),
        },
        SettingsTab::Vfs => match index {
            0 => config.vfs.connection_timeout_secs.to_string(),
            _ => String::new(),
        },
        SettingsTab::Ai => match index {
            0 => provider_label(&config.ai.provider),
            1 => empty_or(&config.ai.base_url),
            2 => empty_or(&config.ai.model),
            3 => empty_or(&config.ai.api_key_env),
            4 => config.ai.context_window_fallback.map_or_else(
                || {
                    format!(
                        "(default {})",
                        termide_config::DEFAULT_CONTEXT_WINDOW_FALLBACK
                    )
                },
                |n| n.to_string(),
            ),
            5 => config
                .ai
                .output_limit()
                .map_or_else(|| "(no limit)".to_string(), |n| n.to_string()),
            6 => bool_str(config.ai.prefer_reasoning),
            7 => bool_str(config.ai.autofold),
            8 => config.ai.web.backend.clone(),
            9 => config.ai.web.engine.clone(),
            12 => permission_mode_label(config.ai.permission_mode()),
            10 => config.ai.web.display.clone(),
            11 => {
                if config.ai.web.chrome_path.is_empty() {
                    "(auto)".to_string()
                } else {
                    config.ai.web.chrome_path.clone()
                }
            }
            _ => String::new(),
        },
        SettingsTab::Keybindings => String::new(),
    }
}

/// A string field's value, or the `(unset)` placeholder when it is empty.
fn empty_or(value: &str) -> String {
    if value.is_empty() {
        "(unset)".to_string()
    } else {
        value.to_string()
    }
}

/// Display label for a provider value: the wire protocol is stored, but the
/// row shows that the endpoint is free-form (base_url picks the real server).
fn provider_label(value: &str) -> String {
    match value {
        "anthropic_compatible" | "anthropic" => "Anthropic compatible".to_string(),
        "openai_compatible" | "openai" => "OpenAI compatible".to_string(),
        "claude_code" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        other => other.to_string(),
    }
}

/// The AI provider values offered in the dropdown, and their labels, sorted by
/// label. `claude_code` and `codex` drive the matching CLI over ACP (they own
/// their own model, endpoint and auth), so they are named after the tool, not
/// "subscription" — the CLI may sign in with a subscription or an API key.
pub(super) const PROVIDER_VALUES: [&str; 4] = [
    "anthropic_compatible",
    "claude_code",
    "codex",
    "openai_compatible",
];

pub(super) use termide_config::is_cli_provider;

/// Reset the fields a CLI-adapter provider does not use back to their defaults,
/// so they disappear from the transcript UI and from the saved config (only
/// non-default values are written). The endpoint, model and auth all live in
/// the CLI, not here.
fn clear_cli_irrelevant_ai_fields(config: &mut Config) {
    let defaults = termide_config::AiSettings::default();
    config.ai.base_url = defaults.base_url;
    // `model` is kept: for a CLI provider it is the model pre-selected on the
    // agent's own login, applied over ACP once the session starts.
    config.ai.api_key_env = defaults.api_key_env;
    config.ai.context_window_fallback = defaults.context_window_fallback;
    config.ai.max_tokens_per_turn = defaults.max_tokens_per_turn;
    config.ai.prefer_reasoning = defaults.prefer_reasoning;
}

/// Apply a provider change: store it, and when it is a CLI adapter, clear the
/// fields it does not use.
fn set_ai_provider(config: &mut Config, provider: &str) {
    config.ai.provider = provider.to_string();
    if is_cli_provider(provider) {
        clear_cli_irrelevant_ai_fields(config);
    }
}

fn bool_str(v: bool) -> String {
    if v {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

/// Toggle a bool field.
pub(super) fn toggle_field(config: &mut Config, tab: SettingsTab, index: usize) {
    match tab {
        SettingsTab::General => match index {
            0 => config.general.vim_mode = !config.general.vim_mode,
            7 => {
                config.general.bell_on_operation_complete =
                    !config.general.bell_on_operation_complete
            }
            9 => config.general.always_detachable = !config.general.always_detachable,
            _ => {}
        },
        SettingsTab::Editor => match index {
            1 => config.editor.word_wrap = !config.editor.word_wrap,
            2 => config.editor.auto_indent = !config.editor.auto_indent,
            3 => config.editor.auto_close_brackets = !config.editor.auto_close_brackets,
            4 => config.editor.show_git_diff = !config.editor.show_git_diff,
            5 => config.editor.show_blame = !config.editor.show_blame,
            _ => {}
        },
        SettingsTab::Lsp => match index {
            0 => config.lsp.enabled = !config.lsp.enabled,
            1 => config.lsp.auto_completion = !config.lsp.auto_completion,
            _ => {}
        },
        SettingsTab::FileManager => {
            if index == 2 {
                config.file_manager.dir_size_in_wide_view =
                    !config.file_manager.dir_size_in_wide_view;
            }
        }
        SettingsTab::Ai => match index {
            6 => config.ai.prefer_reasoning = !config.ai.prefer_reasoning,
            7 => config.ai.autofold = !config.ai.autofold,
            _ => {}
        },
        _ => {}
    }
}

/// The choices behind an enum field: what to store and what to show.
///
/// Cycling through variants with Left/Right is fine for three of them and
/// unusable for twenty-five themes, so the same list also backs a dropdown.
pub(super) struct EnumOptions {
    /// Values as they are written into the config.
    pub values: Vec<String>,
    /// Labels as they are shown to the user.
    pub labels: Vec<String>,
    /// Index of the value currently held by the config, or `None` when the
    /// config holds something the list does not offer.
    ///
    /// That is not hypothetical: the stock config ships `theme = "default"`,
    /// which is a fallback name rather than a theme in `all_theme_names()`.
    /// Cycling with Left/Right silently did nothing in that state, because it
    /// looked the current value up by position and found none.
    pub current: Option<usize>,
}

/// Enumerate the choices for an enum field, or `None` if it is not one.
pub(super) fn enum_options(config: &Config, tab: SettingsTab, index: usize) -> Option<EnumOptions> {
    let (values, labels, current_value) = match (tab, index) {
        (SettingsTab::General, 1) => {
            let names: Vec<String> = Theme::all_theme_names();
            (names.clone(), names, config.general.theme.clone())
        }
        (SettingsTab::General, 2) => {
            let langs = i18n::get_language_list();
            (
                langs.iter().map(|(c, _)| c.to_string()).collect(),
                langs.iter().map(|(_, n)| n.to_string()).collect(),
                config.general.language.clone(),
            )
        }
        (SettingsTab::General, 3) => {
            let values: Vec<String> = ["auto", "emoji", "unicode"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let current = format!("{:?}", config.general.icon_mode).to_lowercase();
            (values.clone(), values, current)
        }
        (SettingsTab::Logging, 1) => {
            let values: Vec<String> = ["trace", "debug", "info", "warn", "error"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            (values.clone(), values, config.logging.min_level.clone())
        }
        (SettingsTab::Ai, 0) => {
            // OpenAI/Anthropic compatible are wire protocols the built-in loop
            // speaks (base_url picks the actual server); Claude Code and Codex
            // drive their CLI over ACP. Listed by label, alphabetically.
            let values: Vec<String> = PROVIDER_VALUES.iter().map(|s| s.to_string()).collect();
            let labels: Vec<String> = PROVIDER_VALUES.iter().map(|v| provider_label(v)).collect();
            (values, labels, config.ai.provider.clone())
        }
        (SettingsTab::Ai, 8) => {
            let values = strings(&termide_config::WEB_BACKENDS);
            (values.clone(), values, config.ai.web.backend.clone())
        }
        (SettingsTab::Ai, 9) => {
            // The shipped engines, plus a user-defined one when it is set.
            let mut values = strings(&termide_config::builtin_web_engines());
            if !values.contains(&config.ai.web.engine) {
                values.push(config.ai.web.engine.clone());
            }
            (values.clone(), values, config.ai.web.engine.clone())
        }
        (SettingsTab::Ai, 10) => {
            let values = strings(&termide_config::WEB_DISPLAYS);
            (values.clone(), values, config.ai.web.display.clone())
        }
        (SettingsTab::Ai, AI_PERMISSION_MODE_FIELD) => {
            let values = strings(&termide_config::permission_modes());
            let labels = values.iter().map(|v| permission_mode_label(v)).collect();
            (values, labels, config.ai.permission_mode().to_string())
        }
        _ => return None,
    };

    let current = values.iter().position(|v| *v == current_value);
    Some(EnumOptions {
        values,
        labels,
        current,
    })
}

/// Store the value chosen in the dropdown.
pub(super) fn apply_enum_value(config: &mut Config, tab: SettingsTab, index: usize, value: &str) {
    match (tab, index) {
        (SettingsTab::General, 1) => config.general.theme = value.to_string(),
        (SettingsTab::General, 2) => config.general.language = value.to_string(),
        (SettingsTab::General, 3) => {
            config.general.icon_mode = match value {
                "emoji" => termide_config::IconMode::Emoji,
                "unicode" => termide_config::IconMode::Unicode,
                _ => termide_config::IconMode::Auto,
            }
        }
        (SettingsTab::Logging, 1) => config.logging.min_level = value.to_string(),
        (SettingsTab::Ai, 0) => set_ai_provider(config, value),
        (SettingsTab::Ai, 2) => config.ai.model = value.to_string(),
        (SettingsTab::Ai, 8) => config.ai.web.backend = value.to_string(),
        (SettingsTab::Ai, 9) => config.ai.web.engine = value.to_string(),
        (SettingsTab::Ai, 10) => config.ai.web.display = value.to_string(),
        (SettingsTab::Ai, AI_PERMISSION_MODE_FIELD) => config.ai.set_permission_mode(value),
        _ => {}
    }
}

/// The AI tab's field for the permission mode new sessions start in.
pub(super) const AI_PERMISSION_MODE_FIELD: usize = 12;

/// A permission mode's localized name, as the agent panel's mode picker
/// shows it.
fn permission_mode_label(mode: &str) -> String {
    let t = i18n::t();
    match mode {
        "accept-edits" => t.agent_mode_accept_edits(),
        "auto" => t.agent_mode_auto(),
        "plan" => t.agent_mode_plan(),
        _ => t.agent_mode_ask(),
    }
    .to_string()
}

/// Step the permission mode to the next (or previous) one, wrapping.
fn cycle_permission_mode(config: &mut Config, forward: bool) {
    let modes = termide_config::permission_modes();
    let mut value = config.ai.permission_mode().to_string();
    step_value(&mut value, &strings(&modes), forward);
    config.ai.set_permission_mode(&value);
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

/// Step a string enum field through `options`, wrapping; a value outside
/// the list starts from the first option.
fn step_value(value: &mut String, options: &[String], forward: bool) {
    let len = options.len();
    if len == 0 {
        return;
    }
    let next = match options.iter().position(|option| option == value) {
        Some(pos) if forward => (pos + 1) % len,
        Some(pos) => (pos + len - 1) % len,
        None => 0,
    };
    *value = options[next].clone();
}

/// Cycle one of the AI tab's web enum fields (8 to 10).
fn cycle_web_field(config: &mut Config, index: usize, forward: bool) {
    let Some(options) = enum_options(config, SettingsTab::Ai, index) else {
        return;
    };
    let field = match index {
        8 => &mut config.ai.web.backend,
        9 => &mut config.ai.web.engine,
        10 => &mut config.ai.web.display,
        _ => return,
    };
    step_value(field, &options.values, forward);
}

/// The AI `model` field's index in the AI tab, and the dropdown value that
/// stands for "type an id by hand" instead of choosing a listed model.
pub(super) const AI_MODEL_FIELD: usize = 2;
pub(super) const MODEL_TYPE_SENTINEL: &str = "\u{0}type-a-model-id";

/// The dropdown for the AI `model` field: the fetched models (with the current
/// value kept present), then a "type an id" escape that opens inline editing.
/// With no fetched models — a CLI provider, or an endpoint that cannot list —
/// only the current value and the escape show, so typing still works.
pub(super) fn ai_model_enum_options(config: &Config, model_options: &[String]) -> EnumOptions {
    let current = config.ai.model.clone();
    let mut values: Vec<String> = model_options.to_vec();
    if !current.is_empty() && !values.contains(&current) {
        values.insert(0, current.clone());
    }
    let mut labels: Vec<String> = values.clone();
    values.push(MODEL_TYPE_SENTINEL.to_string());
    labels.push(i18n::t().agent_model_other().to_string());
    let current_index = values.iter().position(|v| *v == current);
    EnumOptions {
        values,
        labels,
        current: current_index,
    }
}

/// Cycle an enum field to the next variant.
pub(super) fn cycle_enum_forward(config: &mut Config, tab: SettingsTab, index: usize) {
    match tab {
        SettingsTab::General => match index {
            1 => {
                let names = Theme::all_theme_names();
                if let Some(pos) = names.iter().position(|n| n == &config.general.theme) {
                    config.general.theme = names[(pos + 1) % names.len()].clone();
                }
            }
            2 => {
                let langs = i18n::get_language_list();
                if let Some(pos) = langs
                    .iter()
                    .position(|(c, _)| *c == config.general.language)
                {
                    let next = (pos + 1) % langs.len();
                    config.general.language = langs[next].0.to_string();
                }
            }
            3 => {
                config.general.icon_mode = match config.general.icon_mode {
                    termide_config::IconMode::Auto => termide_config::IconMode::Emoji,
                    termide_config::IconMode::Emoji => termide_config::IconMode::Unicode,
                    termide_config::IconMode::Unicode => termide_config::IconMode::Auto,
                }
            }
            _ => {}
        },
        SettingsTab::Logging => {
            if index == 1 {
                config.logging.min_level = match config.logging.min_level.as_str() {
                    "debug" => "info".to_string(),
                    "info" => "warn".to_string(),
                    "warn" => "error".to_string(),
                    _ => "debug".to_string(),
                };
            }
        }
        SettingsTab::Ai => match index {
            0 => cycle_ai_provider(config, true),
            8..=10 => cycle_web_field(config, index, true),
            AI_PERMISSION_MODE_FIELD => cycle_permission_mode(config, true),
            _ => {}
        },
        _ => {}
    }
}

/// Step the AI provider to the next (or previous) value in the dropdown order,
/// wrapping, and clear the CLI-irrelevant fields when it lands on one.
fn cycle_ai_provider(config: &mut Config, forward: bool) {
    let pos = PROVIDER_VALUES
        .iter()
        .position(|v| *v == config.ai.provider)
        // Legacy short names (`openai`, `anthropic`) map to their compatible
        // value so the step lands somewhere sensible.
        .unwrap_or_else(|| match config.ai.provider.as_str() {
            "anthropic" => 0,
            _ => PROVIDER_VALUES.len() - 1,
        });
    let len = PROVIDER_VALUES.len();
    let next = if forward {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    };
    set_ai_provider(config, PROVIDER_VALUES[next]);
}

/// Cycle an enum field to the previous variant.
pub(super) fn cycle_enum_backward(config: &mut Config, tab: SettingsTab, index: usize) {
    match tab {
        SettingsTab::General => match index {
            1 => {
                let names = Theme::all_theme_names();
                if let Some(pos) = names.iter().position(|n| n == &config.general.theme) {
                    let prev = if pos == 0 { names.len() - 1 } else { pos - 1 };
                    config.general.theme = names[prev].clone();
                }
            }
            2 => {
                let langs = i18n::get_language_list();
                if let Some(pos) = langs
                    .iter()
                    .position(|(c, _)| *c == config.general.language)
                {
                    let prev = if pos == 0 { langs.len() - 1 } else { pos - 1 };
                    config.general.language = langs[prev].0.to_string();
                }
            }
            3 => {
                config.general.icon_mode = match config.general.icon_mode {
                    termide_config::IconMode::Auto => termide_config::IconMode::Unicode,
                    termide_config::IconMode::Emoji => termide_config::IconMode::Auto,
                    termide_config::IconMode::Unicode => termide_config::IconMode::Emoji,
                }
            }
            _ => {}
        },
        SettingsTab::Logging => {
            if index == 1 {
                config.logging.min_level = match config.logging.min_level.as_str() {
                    "debug" => "error".to_string(),
                    "info" => "debug".to_string(),
                    "warn" => "info".to_string(),
                    _ => "warn".to_string(),
                };
            }
        }
        SettingsTab::Ai => match index {
            0 => cycle_ai_provider(config, false),
            8..=10 => cycle_web_field(config, index, false),
            AI_PERMISSION_MODE_FIELD => cycle_permission_mode(config, false),
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod field_index_tests {
    use super::*;

    /// The descriptor list, the value getter and the toggle each match on the
    /// same bare index, in three separate `match` arms. Nothing checks that
    /// they agree, so a field added at the wrong index reads one setting and
    /// writes another. This pins the newest one down end to end.
    #[test]
    fn always_detachable_reads_and_writes_the_same_field() {
        let index = 9;
        let mut config = Config::default();

        let fields = fields_for_tab(SettingsTab::General);
        assert_eq!(fields.len(), index + 1, "always_detachable must be last");
        assert!(matches!(fields[index].field_type, FieldType::Bool));

        assert!(!config.general.always_detachable);
        assert_eq!(
            get_field_value(&config, SettingsTab::General, index),
            bool_str(false)
        );

        toggle_field(&mut config, SettingsTab::General, index);
        assert!(
            config.general.always_detachable,
            "toggling index {index} must flip always_detachable, not a neighbour"
        );
        assert_eq!(
            get_field_value(&config, SettingsTab::General, index),
            bool_str(true)
        );

        // Neighbours must be untouched by that toggle.
        assert_eq!(
            config.general.resource_monitor_interval,
            Config::default().general.resource_monitor_interval
        );
        assert_eq!(
            config.general.bell_on_operation_complete,
            Config::default().general.bell_on_operation_complete
        );
    }

    #[test]
    fn web_fields_read_and_write_their_own_settings() {
        let mut config = Config::default();
        let fields = fields_for_tab(SettingsTab::Ai);
        assert_eq!(
            fields.len(),
            13,
            "the permission mode follows the web fields"
        );
        assert!(matches!(fields[11].field_type, FieldType::OptionalText));
        assert!(matches!(fields[12].field_type, FieldType::Enum));

        assert_eq!(get_field_value(&config, SettingsTab::Ai, 8), "auto");
        assert_eq!(get_field_value(&config, SettingsTab::Ai, 9), "duckduckgo");
        assert_eq!(get_field_value(&config, SettingsTab::Ai, 10), "headless");
        assert_eq!(get_field_value(&config, SettingsTab::Ai, 11), "(auto)");

        apply_enum_value(&mut config, SettingsTab::Ai, 9, "bing");
        assert_eq!(config.ai.web.engine, "bing");
        cycle_enum_forward(&mut config, SettingsTab::Ai, 8);
        assert_eq!(config.ai.web.backend, "chrome");
        cycle_enum_backward(&mut config, SettingsTab::Ai, 10);
        assert_eq!(config.ai.web.display, "visible");
        assert_eq!(config.ai.provider, Config::default().ai.provider);

        // A user-defined engine stays selectable.
        config.ai.web.engine = "intranet".into();
        let options = enum_options(&config, SettingsTab::Ai, 9).unwrap();
        assert_eq!(options.values.last().map(String::as_str), Some("intranet"));
        assert_eq!(options.current, Some(options.values.len() - 1));
    }
}

#[cfg(test)]
mod enum_option_tests {
    use super::*;

    /// Every field declared as an enum must be able to list its choices —
    /// otherwise its dropdown opens empty and the value becomes uneditable
    /// from the UI.
    #[test]
    fn every_enum_field_can_enumerate_its_choices() {
        let config = Config::default();
        let tabs = [
            SettingsTab::General,
            SettingsTab::Editor,
            SettingsTab::FileManager,
            SettingsTab::Terminal,
            SettingsTab::Lsp,
            SettingsTab::Logging,
            SettingsTab::Vfs,
        ];

        for tab in tabs {
            for (index, desc) in fields_for_tab(tab).iter().enumerate() {
                if desc.field_type != FieldType::Enum {
                    assert!(
                        enum_options(&config, tab, index).is_none(),
                        "{tab:?} field {index} is not an enum but offers choices"
                    );
                    continue;
                }
                let options = enum_options(&config, tab, index)
                    .unwrap_or_else(|| panic!("{tab:?} field {index} lists no choices"));
                assert!(!options.values.is_empty());
                assert_eq!(options.values.len(), options.labels.len());
                if let Some(current) = options.current {
                    assert!(current < options.values.len());
                }
            }
        }
    }

    #[test]
    fn ai_provider_lists_all_four_sorted_by_label() {
        let config = Config::default();
        let options = enum_options(&config, SettingsTab::Ai, 0).unwrap();
        assert_eq!(
            options.values,
            vec![
                "anthropic_compatible",
                "claude_code",
                "codex",
                "openai_compatible"
            ]
        );
        assert_eq!(
            options.labels,
            vec![
                "Anthropic compatible",
                "Claude Code",
                "Codex",
                "OpenAI compatible"
            ]
        );
        // Labels are in alphabetical order.
        let mut sorted = options.labels.clone();
        sorted.sort();
        assert_eq!(options.labels, sorted);
    }

    #[test]
    fn choosing_a_cli_provider_clears_the_unused_fields() {
        let mut config = Config::default();
        config.ai.base_url = "https://example/v1".into();
        config.ai.model = "gpt-5".into();
        config.ai.api_key_env = "MY_KEY".into();
        config.ai.context_window_fallback = Some(123);
        config.ai.max_tokens_per_turn = 999;
        config.ai.prefer_reasoning = true;

        apply_enum_value(&mut config, SettingsTab::Ai, 0, "claude_code");

        assert_eq!(config.ai.provider, "claude_code");
        let defaults = termide_config::AiSettings::default();
        assert_eq!(config.ai.base_url, defaults.base_url);
        // The model is kept — it is the model to pre-select on the CLI agent.
        assert_eq!(config.ai.model, "gpt-5");
        assert_eq!(config.ai.api_key_env, defaults.api_key_env);
        assert_eq!(
            config.ai.context_window_fallback,
            defaults.context_window_fallback
        );
        assert_eq!(config.ai.max_tokens_per_turn, defaults.max_tokens_per_turn);
        assert_eq!(config.ai.prefer_reasoning, defaults.prefer_reasoning);
        // A wire-protocol provider leaves the fields alone.
        config.ai.model = "gpt-5".into();
        apply_enum_value(&mut config, SettingsTab::Ai, 0, "openai_compatible");
        assert_eq!(config.ai.model, "gpt-5");
    }

    #[test]
    fn the_permission_mode_for_new_sessions_is_chosen_from_the_four() {
        let mut config = Config::default();
        let field = AI_PERMISSION_MODE_FIELD;
        // Ask by default, shown by its localized name.
        assert_eq!(config.ai.permission_mode(), "ask");
        let options = enum_options(&config, SettingsTab::Ai, field).unwrap();
        assert_eq!(options.values, ["ask", "accept-edits", "auto", "plan"]);
        assert_eq!(options.current, Some(0));
        assert_eq!(
            get_field_value(&config, SettingsTab::Ai, field),
            i18n::t().agent_mode_ask()
        );
        apply_enum_value(&mut config, SettingsTab::Ai, field, "auto");
        assert_eq!(config.ai.permission_mode(), "auto");
        cycle_enum_forward(&mut config, SettingsTab::Ai, field);
        assert_eq!(config.ai.permission_mode(), "plan");
        cycle_enum_forward(&mut config, SettingsTab::Ai, field);
        assert_eq!(config.ai.permission_mode(), "ask");
        cycle_enum_backward(&mut config, SettingsTab::Ai, field);
        assert_eq!(config.ai.permission_mode(), "plan");
    }

    #[test]
    fn cycling_the_provider_wraps_through_every_value() {
        let mut config = Config::default();
        config.ai.provider = "anthropic_compatible".into();
        cycle_enum_backward(&mut config, SettingsTab::Ai, 0);
        // Backward from the first wraps to the last.
        assert_eq!(config.ai.provider, "openai_compatible");
        cycle_enum_forward(&mut config, SettingsTab::Ai, 0);
        assert_eq!(config.ai.provider, "anthropic_compatible");
        cycle_enum_forward(&mut config, SettingsTab::Ai, 0);
        // Landing on a CLI provider clears the unused fields.
        assert_eq!(config.ai.provider, "claude_code");
    }

    /// Choosing from the dropdown and cycling with Left/Right must write the
    /// same field, or the two ways of setting a value would disagree.
    #[test]
    fn applying_a_choice_matches_what_the_getter_reports() {
        let mut config = Config::default();
        let options = enum_options(&config, SettingsTab::Logging, 1).unwrap();

        for (index, value) in options.values.iter().enumerate() {
            apply_enum_value(&mut config, SettingsTab::Logging, 1, value);
            let back = enum_options(&config, SettingsTab::Logging, 1).unwrap();
            assert_eq!(back.current, Some(index), "round trip failed for {value}");
            assert_eq!(get_field_value(&config, SettingsTab::Logging, 1), *value);
        }
    }
}

#[cfg(test)]
mod unlisted_value_tests {
    use super::*;

    /// The stock config names a theme that is not in the theme list, and the
    /// dropdown must still open on it — marking nothing as current rather than
    /// pointing at an unrelated entry.
    #[test]
    fn a_value_outside_the_list_marks_nothing_as_current() {
        let mut config = Config::default();
        config.general.theme = "no-such-theme".to_string();

        let options = enum_options(&config, SettingsTab::General, 1).unwrap();
        assert!(!options.values.is_empty());
        assert_eq!(options.current, None);

        // Choosing from the list still lands somewhere real.
        let first = options.values[0].clone();
        apply_enum_value(&mut config, SettingsTab::General, 1, &first);
        let after = enum_options(&config, SettingsTab::General, 1).unwrap();
        assert_eq!(after.current, Some(0));
    }
}

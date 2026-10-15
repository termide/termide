//! `/name` in the input: which of the built-in commands, prompt templates,
//! command scripts and skills a name runs, and the names more than one kind
//! defines.
//!
//! Precedence is built-in, template, script, skill. A built-in command comes
//! first so a cloned repository cannot take over `/clear` or `/compact`; a
//! template or script is written to be typed after `/`, a skill is first of
//! all the model's, so it comes last. `/skill:<name>` always reaches the skill,
//! so a skill whose short name is taken can still be called.

use std::collections::BTreeMap;

use termide_agent_core::{CommandScript, PromptTemplate, SkillInfo};

/// The prefix that names a skill whatever else shares its name.
pub(crate) const SKILL_PREFIX: &str = "skill:";

/// A skill's description as the `/` list and the AI menu show it: marked when
/// the skill is kept to the user (`disable-model-invocation`), since the
/// model never loads such a skill itself.
#[must_use]
pub fn user_skill_description(skill: &SkillInfo) -> String {
    let t = termide_i18n::t();
    if skill.model_invocable {
        skill.description.clone()
    } else if skill.description.is_empty() {
        t.agent_user_skill().to_string()
    } else {
        t.agent_user_skill_fmt(&skill.description)
    }
}

/// What defines a `/name`, in precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SlashKind {
    Builtin,
    Template,
    Script,
    Skill,
}

impl SlashKind {
    /// The kind as a notice names it.
    pub(crate) fn label(self) -> &'static str {
        let t = termide_i18n::t();
        match self {
            Self::Builtin => t.agent_slash_kind_builtin(),
            Self::Template => t.agent_slash_kind_template(),
            Self::Script => t.agent_slash_kind_script(),
            Self::Skill => t.agent_slash_kind_skill(),
        }
    }
}

/// What a `/name` that is not built in runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SlashTarget {
    Template(PromptTemplate),
    Script(CommandScript),
    Skill(SkillInfo),
}

/// The template, script or skill `name` runs; built-in commands are matched
/// before this is asked. `skill:<name>` looks among the skills only.
pub(crate) fn resolve(
    name: &str,
    prompts: Vec<PromptTemplate>,
    commands: Vec<CommandScript>,
    skills: Vec<SkillInfo>,
) -> Option<SlashTarget> {
    let find_skill = |name: &str| {
        skills
            .iter()
            .find(|skill| skill.name == name)
            .cloned()
            .map(SlashTarget::Skill)
    };
    if let Some(skill) = name.strip_prefix(SKILL_PREFIX) {
        return find_skill(skill);
    }
    if let Some(template) = prompts.into_iter().find(|p| p.name == name) {
        return Some(SlashTarget::Template(template));
    }
    if let Some(script) = commands.into_iter().find(|c| c.name == name) {
        return Some(SlashTarget::Script(script));
    }
    find_skill(name)
}

/// A `/name` more than one kind defines: the kind it runs and the ones it
/// hides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Conflict {
    pub name: String,
    pub runs: SlashKind,
    pub hidden: Vec<SlashKind>,
}

impl Conflict {
    /// The notice that explains it, with the way to reach a hidden skill.
    pub(crate) fn describe(&self) -> String {
        let t = termide_i18n::t();
        let hidden: Vec<&str> = self.hidden.iter().map(|kind| kind.label()).collect();
        let mut text =
            t.agent_notice_slash_shadowed_fmt(&self.name, self.runs.label(), &hidden.join(", "));
        if self.hidden.contains(&SlashKind::Skill) {
            text.push_str("; ");
            text.push_str(&t.agent_notice_slash_skill_hint_fmt(&self.name));
        }
        text
    }
}

/// Every name defined by more than one kind, sorted by name. The same name
/// at several levels of one kind is not a conflict: the higher level hiding
/// the lower is how levels work.
pub(crate) fn conflicts(
    builtins: &[&str],
    prompts: &[PromptTemplate],
    commands: &[CommandScript],
    skills: &[SkillInfo],
) -> Vec<Conflict> {
    let mut kinds: BTreeMap<&str, Vec<SlashKind>> = BTreeMap::new();
    let names = builtins
        .iter()
        .map(|name| (*name, SlashKind::Builtin))
        .chain(
            prompts
                .iter()
                .map(|p| (p.name.as_str(), SlashKind::Template)),
        )
        .chain(
            commands
                .iter()
                .map(|c| (c.name.as_str(), SlashKind::Script)),
        )
        .chain(skills.iter().map(|s| (s.name.as_str(), SlashKind::Skill)));
    for (name, kind) in names {
        let entry = kinds.entry(name).or_default();
        if !entry.contains(&kind) {
            entry.push(kind);
        }
    }
    kinds
        .into_iter()
        .filter(|(_, kinds)| kinds.len() > 1)
        .map(|(name, mut kinds)| {
            kinds.sort();
            let runs = kinds.remove(0);
            Conflict {
                name: name.to_string(),
                runs,
                hidden: kinds,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn template(name: &str) -> PromptTemplate {
        PromptTemplate {
            name: name.into(),
            description: String::new(),
            argument_hint: String::new(),
            body: format!("template {name}"),
        }
    }

    fn script(name: &str) -> CommandScript {
        CommandScript {
            name: name.into(),
            description: String::new(),
            argument_hint: String::new(),
            path: PathBuf::from(name),
            trusted: true,
            timeout_secs: 60,
        }
    }

    fn skill(name: &str) -> SkillInfo {
        SkillInfo {
            name: name.into(),
            description: String::new(),
            argument_hint: String::new(),
            model_invocable: true,
            path: PathBuf::from(name),
        }
    }

    #[test]
    fn a_skill_kept_to_the_user_is_marked() {
        let mut described = skill("triage");
        described.description = "Triage issues".into();
        assert_eq!(user_skill_description(&described), "Triage issues");
        described.model_invocable = false;
        let marked = user_skill_description(&described);
        assert!(marked.contains("Triage issues") && marked != "Triage issues");
        let mut bare = skill("wait-what");
        bare.model_invocable = false;
        assert!(!user_skill_description(&bare).is_empty());
    }

    #[test]
    fn a_template_beats_a_script_beats_a_skill() {
        let run = |name: &str| {
            resolve(
                name,
                vec![template("review")],
                vec![script("review"), script("failing")],
                vec![skill("review"), skill("failing"), skill("deploy")],
            )
        };
        assert_eq!(
            run("review"),
            Some(SlashTarget::Template(template("review")))
        );
        assert_eq!(run("failing"), Some(SlashTarget::Script(script("failing"))));
        assert_eq!(run("deploy"), Some(SlashTarget::Skill(skill("deploy"))));
        assert_eq!(run("nope"), None);
        // The prefix reaches a skill whatever shadows it, and only a skill.
        assert_eq!(
            run("skill:review"),
            Some(SlashTarget::Skill(skill("review")))
        );
        assert_eq!(run("skill:nope"), None);
    }

    #[test]
    fn conflicts_name_the_kind_that_runs_and_the_hidden_ones() {
        let found = conflicts(
            &["compact", "clear"],
            &[template("review"), template("compact")],
            &[script("review")],
            &[skill("review"), skill("clear"), skill("deploy")],
        );
        assert_eq!(
            found,
            vec![
                Conflict {
                    name: "clear".into(),
                    runs: SlashKind::Builtin,
                    hidden: vec![SlashKind::Skill],
                },
                Conflict {
                    name: "compact".into(),
                    runs: SlashKind::Builtin,
                    hidden: vec![SlashKind::Template],
                },
                Conflict {
                    name: "review".into(),
                    runs: SlashKind::Template,
                    hidden: vec![SlashKind::Script, SlashKind::Skill],
                },
            ]
        );
        assert!(found[0].describe().contains("/skill:clear"));
        assert!(!found[1].describe().contains("/skill:"));
    }
}

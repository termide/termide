//! `skill`: load a skill's instructions by name.
//!
//! The system prompt lists skills by name and description only; the body of
//! `SKILL.md` enters the context when the model asks for it, so a skill costs
//! one line per request until it is needed. A name is cheaper and safer for
//! the model than a path — the list is an enum in the schema — and the tool
//! returns the body verbatim, without `read`'s line numbers, together with
//! the files that come with the skill, which the model reads by path.
//! Arguments fill the body's `$ARGUMENTS` and `$1`…`$9`, as a prompt
//! template's do.

use serde_json::{json, Value};
use termide_agent_core::ToolText;
use termide_agent_core::{
    CancelToken, SkillInfo, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate,
};

use crate::args::{optional_str, required_str};

pub struct SkillTool {
    skills: Vec<SkillInfo>,
}

impl SkillTool {
    #[must_use]
    pub fn new(skills: Vec<SkillInfo>) -> Self {
        Self { skills }
    }
}

impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        &ToolText::seed("skill").description
    }

    fn parameters(&self) -> Value {
        let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "enum": names, "description": "Skill name as listed in the system prompt" },
                "args": { "type": "string", "description": "Arguments for the skill, shaped as the hint after its name; omit when it shows none" }
            },
            "required": ["name"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("skill").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("skill").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        match self.load(call) {
            Ok((text, details)) => ToolResultMessage::text(call, text).with_details(details),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

impl SkillTool {
    fn load(&self, call: &ToolCall) -> Result<(String, Value), String> {
        let name = required_str(call, "name")?;
        let args = optional_str(call, "args")?.unwrap_or("");
        let Some(skill) = self.skills.iter().find(|s| s.name == name) else {
            let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
            return Err(format!(
                "no skill named {name}; available: {}",
                names.join(", ")
            ));
        };
        let loaded = skill.load(args)?;
        Ok((
            loaded.text,
            json!({ "name": skill.name, "path": skill.path, "files": loaded.files }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn run(tool: &SkillTool, args: Value) -> ToolResultMessage {
        let call = ToolCall {
            id: "c".into(),
            name: "skill".into(),
            arguments: args,
            extra_content: None,
        };
        let ctx = ToolContext::new(PathBuf::from("/"));
        tool.execute(&call, &ctx, &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn a_skill_loads_without_front_matter_and_lists_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("deploy");
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: deploy\ndescription: Ship it\n---\n# Deploy\n\nRun the script.\n",
        )
        .unwrap();
        std::fs::write(skill.join("scripts/release.sh"), "#!/bin/sh\n").unwrap();
        std::fs::write(skill.join("checklist.md"), "- tag\n").unwrap();
        let tool = SkillTool::new(vec![SkillInfo {
            name: "deploy".into(),
            description: "Ship it".into(),
            argument_hint: String::new(),
            path: skill.join("SKILL.md"),
        }]);

        assert_eq!(
            tool.parameters()["properties"]["name"]["enum"],
            json!(["deploy"])
        );
        let result = run(&tool, json!({ "name": "deploy" }));
        assert!(!result.is_error, "{}", result.plain_text());
        let text = result.plain_text();
        assert!(text.starts_with("# Deploy\n\nRun the script."), "{text}");
        assert!(!text.contains("description:"));
        assert!(
            text.contains("- checklist.md\n- scripts/release.sh\n"),
            "{text}"
        );
        assert_eq!(
            result.details.unwrap()["files"],
            json!(["checklist.md", "scripts/release.sh"])
        );

        let missing = run(&tool, json!({ "name": "nope" }));
        assert!(missing.is_error);
        assert!(missing.plain_text().contains("available: deploy"));
    }

    #[test]
    fn arguments_fill_the_placeholders_or_follow_the_body() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, body: &str| {
            let skill = dir.path().join(name);
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(skill.join("SKILL.md"), body).unwrap();
            SkillInfo {
                name: name.into(),
                description: String::new(),
                argument_hint: String::new(),
                path: skill.join("SKILL.md"),
            }
        };
        let tool = SkillTool::new(vec![
            write(
                "review",
                "---\nargument-hint: <path>\n---\nReview $1 ($ARGUMENTS).\n",
            ),
            write("plain", "Do the thing.\n"),
        ]);
        assert_eq!(
            tool.parameters()["properties"]["args"]["type"],
            json!("string")
        );

        let filled = run(&tool, json!({ "name": "review", "args": "src/x.rs fast" }));
        assert_eq!(filled.plain_text(), "Review src/x.rs (src/x.rs fast).");
        let appended = run(&tool, json!({ "name": "plain", "args": "src/x.rs" }));
        assert_eq!(appended.plain_text(), "Do the thing.\n\nsrc/x.rs");
        let bare = run(&tool, json!({ "name": "plain" }));
        assert_eq!(bare.plain_text(), "Do the thing.");
        let wrong = run(&tool, json!({ "name": "plain", "args": 3 }));
        assert!(wrong.is_error);
    }
}

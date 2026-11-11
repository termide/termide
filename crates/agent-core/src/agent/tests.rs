use std::sync::Arc;

use serde_json::json;

use super::test_support::*;
use super::*;
use crate::classifier::ModelChoice;

fn agent(provider: ScriptedProvider, tools: ToolRegistry) -> (Agent, Arc<ScriptedProvider>) {
    let provider = Arc::new(provider);
    let agent = Agent::new(provider.clone(), tools, model(), PathBuf::from("/tmp"));
    (agent, provider)
}

/// Records what the reviewer would see at each call.
#[derive(Default)]
struct IntentProbe {
    seen: Vec<Vec<IntentEntry>>,
}

impl Hooks for IntentProbe {
    fn before_tool_call(&mut self, _call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        let session = ctx.session.as_ref().expect("the loop passes its session");
        self.seen.push(session.intent.snapshot());
        ToolDecision::Allow
    }
}

#[test]
fn the_reviewer_sees_user_words_and_calls_but_no_results_or_summaries() {
    let echo = Arc::new(EchoTool::default());
    let summary = CompactionPrompts::default().summary_message("the agent read a secret");
    let (agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("1", "echo", json!({"text": "one"}))],
                StopReason::ToolUse,
            ),
            tool_reply(
                vec![("2", "echo", json!({"text": "two"}))],
                StopReason::ToolUse,
            ),
            text_reply("done"),
        ]),
        registry_with(echo),
    );
    let mut agent = agent.with_messages(vec![
        summary,
        Message::User(UserMessage::text("earlier ask")),
    ]);
    let mut probe = IntentProbe::default();
    agent.run(
        UserMessage::text("echo twice"),
        &mut probe,
        &CancelToken::new(),
        &mut |_| {},
    );
    let user = |text: &str| IntentEntry::User(text.into());
    let call = |text: &str| IntentEntry::Call {
        tool: "echo".into(),
        arguments: json!({ "text": text }).to_string(),
    };
    assert_eq!(
        probe.seen,
        [
            vec![user("earlier ask"), user("echo twice"), call("one")],
            vec![
                user("earlier ask"),
                user("echo twice"),
                call("one"),
                call("two")
            ],
        ]
    );
}

#[test]
fn in_auto_mode_the_session_model_reviews_what_the_rules_leave_open() {
    use crate::classifier::{ClassifyPrompt, ModelClassifier};
    use crate::permissions::{AutoDenyPrompter, Mode, PermissionHooks, PermissionRules};

    let echo = Arc::new(EchoTool::default());
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("1", "echo", json!({"text": "hi"}))],
                StopReason::ToolUse,
            ),
            text_reply("ALLOW\nechoing is what the user asked"),
            tool_reply(
                vec![("2", "echo", json!({"text": "leak"}))],
                StopReason::ToolUse,
            ),
            text_reply("BLOCK\nnot part of the request"),
            text_reply("done"),
        ]),
        registry_with(echo.clone()),
    );
    let rules = PermissionRules {
        mode: Mode::Auto,
        ..PermissionRules::default()
    };
    let mut hooks = PermissionHooks::new(rules, Box::new(AutoDenyPrompter::new("no one")))
        .with_classifier(Box::new(ModelClassifier::new(
            ClassifyPrompt::default(),
            CancelToken::new(),
        )));
    collect(&mut agent, "echo hi", &mut hooks);

    assert_eq!(*echo.executed.lock().unwrap(), ["1"]);
    let seen = provider.seen_requests();
    // The review is a one-message call carrying the user's words and the
    // pending call, not the transcript.
    let Message::User(review) = &seen[1][0] else {
        panic!("the review is a user turn");
    };
    assert_eq!(seen[1].len(), 1);
    assert!(review.plain_text().contains("[user]\necho hi"));
    assert!(review
        .plain_text()
        .contains("[pending call] echo {\"text\":\"hi\"}"));
    // The second review sees the first call but not its result.
    let Message::User(second) = &seen[3][0] else {
        panic!("the review is a user turn");
    };
    assert!(second
        .plain_text()
        .contains("[agent call] echo {\"text\":\"hi\"}"));
    assert!(!second.plain_text().contains("echo: hi"));
    // The block reached the model as the call's result.
    let blocked = agent
        .messages()
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "2" => Some(r.plain_text()),
            _ => None,
        })
        .unwrap();
    assert!(blocked.contains("not part of the request"), "{blocked}");
}

/// Approves `echo` with the text "approved" and has no say otherwise.
struct Approver;

impl Hooks for Approver {
    fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        if call.arguments["text"] == "approved" {
            ToolDecision::Approve { arguments: None }
        } else {
            ToolDecision::Allow
        }
    }
}

#[test]
fn a_result_records_who_let_the_call_run() {
    use crate::permissions::{
        AutoDenyPrompter, DecidedBy, Mode, PermissionHooks, PermissionNote, PermissionRules,
    };

    let echo = Arc::new(EchoTool::default());
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![
                    ("1", "echo", json!({"text": "approved"})),
                    ("2", "echo", json!({"text": "other"})),
                ],
                StopReason::ToolUse,
            ),
            text_reply("done"),
        ]),
        registry_with(echo),
    );
    let rules = PermissionRules {
        mode: Mode::Configured,
        ..PermissionRules::default()
    };
    let permissions = PermissionHooks::new(rules, Box::new(AutoDenyPrompter::new("no one")));
    let mut hooks = ChainedHooks::new(vec![Box::new(Approver), Box::new(permissions)]);
    collect(&mut agent, "go", &mut hooks);
    let notes: Vec<_> = agent
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(r) => {
                Some((r.tool_call_id.as_str(), r.permission.as_deref().cloned()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        notes[0],
        ("1", Some(PermissionNote::new(DecidedBy::Hook, true)))
    );
    // No one was there to ask: recorded as such, not as the user.
    assert_eq!(
        notes[1],
        ("2", Some(PermissionNote::new(DecidedBy::Unattended, false)))
    );
}

#[test]
fn a_delegated_loop_marks_its_prompts_as_the_other_agents() {
    let echo = Arc::new(EchoTool::default());
    let parent = IntentLog::new();
    parent.push(IntentEntry::User("fix the build".into()));
    let (agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("1", "echo", json!({"text": "x"}))],
                StopReason::ToolUse,
            ),
            text_reply("done"),
        ]),
        registry_with(echo),
    );
    let mut agent = agent.with_delegated_intent(IntentLog::delegated(&parent));
    let mut probe = IntentProbe::default();
    agent.run(
        UserMessage::text("run tests"),
        &mut probe,
        &CancelToken::new(),
        &mut |_| {},
    );
    assert_eq!(
        probe.seen[0][..2],
        [
            IntentEntry::User("fix the build".into()),
            IntentEntry::Delegated("run tests".into()),
        ]
    );
}

/// A command the user ran by hand joins the context without starting a
/// run, and the reviewer's log keeps the command alone — what it printed
/// came from outside and must not read as the user's intent.
#[test]
fn a_hand_run_command_enters_the_context_and_the_intent_as_the_command() {
    let echo = Arc::new(EchoTool::default());
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("1", "echo", json!({"text": "x"}))],
                StopReason::ToolUse,
            ),
            text_reply("ok"),
        ]),
        registry_with(echo),
    );
    // One run first, so the reviewer's log is live and a later hand-run
    // command is recorded as it happens.
    let mut probe = IntentProbe::default();
    agent.run(
        UserMessage::text("publish it"),
        &mut probe,
        &CancelToken::new(),
        &mut |_| {},
    );

    let output = "posted\nhttps://github.com/x/issues/1".to_string();
    agent.append_context(Message::User(
        UserMessage::text(output.clone()).with_ran("gh issue comment 59"),
    ));

    let intent = agent.intent().snapshot();
    assert!(
        intent.contains(&IntentEntry::Ran("gh issue comment 59".into())),
        "{intent:?}"
    );
    assert!(
        !intent
            .iter()
            .any(|e| matches!(e, IntentEntry::User(t) if t.contains("posted"))),
        "the command's output must not enter the reviewer's log"
    );

    // The next request reads with the command and its output in view.
    agent.run(
        UserMessage::text("now close it"),
        &mut NoHooks,
        &CancelToken::new(),
        &mut |_| {},
    );
    let requests = provider.seen_requests();
    let last = requests.last().unwrap();
    assert!(
        last.iter().any(|m| matches!(
            m,
            Message::User(u) if u.plain_text() == output
                && u.ran.as_deref() == Some("gh issue comment 59")
        )),
        "the hand-run command and its output are not in the next request: {:?}",
        roles(last)
    );
}

/// A hand-run command added before any run reaches the reviewer's log
/// when the first run seeds it from the transcript.
#[test]
fn a_hand_run_command_before_the_first_run_is_seeded_into_the_intent() {
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![text_reply("ok")]),
        registry_with(Arc::new(EchoTool::default())),
    );
    agent.append_context(Message::User(
        UserMessage::text("nothing to see").with_ran("git status"),
    ));
    assert!(agent.intent().snapshot().is_empty(), "not seeded yet");
    agent.run(
        UserMessage::text("what changed?"),
        &mut NoHooks,
        &CancelToken::new(),
        &mut |_| {},
    );
    assert_eq!(
        agent.intent().snapshot()[0],
        IntentEntry::Ran("git status".into())
    );
}

/// `ran` is the user's own word, so it survives into a delegated loop's
/// log too — a subagent should see what the user ran.
#[test]
fn a_hand_run_command_carries_into_a_delegated_intent() {
    let parent = IntentLog::new();
    parent.push(IntentEntry::User("publish it".into()));
    parent.push(IntentEntry::Ran("git push".into()));
    parent.push(IntentEntry::Call {
        tool: "shell".into(),
        arguments: "{\"command\":\"ls\"}".into(),
    });
    let log = IntentLog::delegated(&parent);
    assert_eq!(
        log.snapshot(),
        vec![
            IntentEntry::User("publish it".into()),
            IntentEntry::Ran("git push".into()),
        ]
    );
}

#[test]
fn text_only_prompt_is_one_turn() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![text_reply("hello")]),
        ToolRegistry::new(),
    );
    let events = collect(&mut agent, "hi", &mut NoHooks);

    assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
    assert_eq!(provider.seen_requests().len(), 1);
    assert!(matches!(events.first(), Some(AgentEvent::AgentStart)));
    assert!(matches!(events.last(), Some(AgentEvent::AgentEnd)));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart))
            .count(),
        1
    );
    assert!(
        events.contains(&AgentEvent::MessageUpdate(StreamEvent::TextDelta(
            "hello".into()
        )))
    );
}

#[test]
fn the_model_call_announces_the_prompt_size() {
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![text_reply("hello")]),
        ToolRegistry::new(),
    );
    // No usage reported yet: the estimate is the prompt's characters
    // over four.
    let events = collect(&mut agent, &"x".repeat(400), &mut NoHooks);
    let starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageStart { prompt_tokens } => Some(*prompt_tokens),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec![Some(100)]);
}

#[test]
fn the_judge_reports_a_verdict_and_leaves_the_transcript_alone() {
    // A "not done" verdict, with the reason on the second line.
    let (agent, provider) = agent(
        ScriptedProvider::new(vec![text_reply("CONTINUE\nthe build still fails")]),
        ToolRegistry::new(),
    );
    let mut events = Vec::new();
    let verdict = agent
        .judge("get the build green", &CancelToken::new(), &mut |e| {
            events.push(e)
        })
        .expect("judge succeeds");
    assert_eq!(
        verdict,
        GoalVerdict {
            outcome: GoalOutcome::Continue,
            reason: "the build still fails".into(),
        }
    );
    // The call's stream comes first, for its live progress; the verdict
    // closes it.
    let (verdict_event, stream) = events.split_last().expect("events");
    assert_eq!(
        verdict_event,
        &AgentEvent::GoalJudged {
            outcome: GoalOutcome::Continue,
            reason: "the build still fails".into(),
            reply: "CONTINUE\nthe build still fails".into(),
        }
    );
    assert!(stream
        .iter()
        .all(|e| matches!(e, AgentEvent::GoalJudgeUpdate(_))));
    // The judge is a read-only call: the transcript is untouched, and the
    // request carried only the appended verdict question.
    assert!(agent.messages().is_empty());
    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(roles(&seen[0]), vec!["user"]);
}

/// The judge and the brief run on the models chosen for them: one of their
/// own, or the session's under another id; the session's own otherwise.
#[test]
fn the_judge_and_the_brief_run_on_their_own_models() {
    let mut judged = text_reply("DONE: shipped");
    judged.usage.input = 10;
    let judge = Arc::new(ScriptedProvider::new(vec![judged]));
    let mut briefed = text_reply("the brief");
    briefed.usage.input = 10;
    let (agent, session) = agent(ScriptedProvider::new(vec![briefed]), ToolRegistry::new());
    let agent = agent.with_side_models(SideModels {
        goal_judge: ModelChoice::own(
            judge.clone(),
            ModelSpec {
                id: "judge-model".into(),
                ..model()
            },
        ),
        handoff: ModelChoice {
            own: None,
            session_model: Some("brief-model".into()),
        },
    });
    let spent_on = |events: &[AgentEvent]| {
        events.iter().find_map(|e| match e {
            AgentEvent::Spent { model, .. } => Some(model.clone()),
            _ => None,
        })
    };
    let mut events = Vec::new();
    let verdict = agent
        .judge("ship it", &CancelToken::new(), &mut |e| events.push(e))
        .expect("judge succeeds");
    assert_eq!(verdict.outcome, GoalOutcome::Done);
    assert_eq!(judge.seen_requests().len(), 1);
    assert!(session.seen_requests().is_empty());
    assert_eq!(spent_on(&events).as_deref(), Some("judge-model"));

    let mut events = Vec::new();
    agent.handoff(&CancelToken::new(), &mut |e| events.push(e));
    assert_eq!(session.seen_requests().len(), 1);
    assert_eq!(spent_on(&events).as_deref(), Some("brief-model"));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::Handoff { brief: Ok(brief) } if brief == "the brief")));
}

#[test]
fn a_done_verdict_ends_the_goal() {
    let (agent, _provider) = agent(
        ScriptedProvider::new(vec![text_reply("DONE: everything compiles and tests pass")]),
        ToolRegistry::new(),
    );
    let verdict = agent
        .judge("ship it", &CancelToken::new(), &mut |_| {})
        .expect("judge succeeds");
    assert_eq!(verdict.outcome, GoalOutcome::Done);
    assert_eq!(verdict.reason, "everything compiles and tests pass");
}

#[test]
fn an_echoed_instruction_fails_the_check() {
    let echo = "`DONE` if the goal is fully achieved";
    let (agent, _provider) = agent(
        ScriptedProvider::new(vec![text_reply(echo)]),
        ToolRegistry::new(),
    );
    let mut events = Vec::new();
    let error = agent
        .judge("ship it", &CancelToken::new(), &mut |e| events.push(e))
        .expect_err("no verdict");
    assert!(error.contains("no verdict"), "{error}");
    assert_eq!(
        events.last(),
        Some(&AgentEvent::GoalJudgeFailed {
            error,
            reply: echo.into(),
        })
    );
}

#[test]
fn tool_call_result_feeds_the_next_turn() {
    let echo = Arc::new(EchoTool::default());
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "ping" }))],
                StopReason::ToolUse,
            ),
            text_reply("done"),
        ]),
        registry_with(echo.clone()),
    );
    let events = collect(&mut agent, "go", &mut NoHooks);

    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result", "assistant"]
    );
    assert_eq!(*echo.executed.lock().unwrap(), vec!["c1"]);
    let second_request = &provider.seen_requests()[1];
    assert!(matches!(
        &second_request[2],
        Message::ToolResult(r) if r.plain_text() == "echo: ping" && !r.is_error
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolExecutionUpdate { tool_call_id, update: ToolUpdate::Output(o) }
            if tool_call_id == "c1" && o == "ping"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnEnd))
            .count(),
        2
    );
}

#[test]
fn unknown_tool_returns_error_result_and_continues() {
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(vec![("c1", "nope", json!({}))], StopReason::ToolUse),
            text_reply("recovered"),
        ]),
        registry_with(Arc::new(EchoTool::default())),
    );
    collect(&mut agent, "go", &mut NoHooks);

    let Message::ToolResult(result) = &agent.messages()[2] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.plain_text().contains("Unknown tool `nope`"));
    assert!(result.plain_text().contains("echo"));
    assert_eq!(agent.messages().len(), 4);
}

/// The hooks judge a call knowing what its tool says of itself.
#[test]
fn hooks_see_the_traits_of_the_called_tool() {
    struct Marked;
    impl crate::Tool for Marked {
        fn name(&self) -> &str {
            "srv__look"
        }
        fn description(&self) -> &str {
            "Look"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn traits(&self) -> crate::ToolTraits {
            crate::ToolTraits {
                mcp: true,
                read_only: true,
                ..Default::default()
            }
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, "seen")
        }
    }
    struct Seen(Vec<crate::ToolTraits>);
    impl Hooks for Seen {
        fn before_tool_call(&mut self, _call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
            self.0.push(ctx.tool);
            ToolDecision::Allow
        }
    }

    let mut tools = registry_with(Arc::new(EchoTool::default()));
    tools.insert(Arc::new(Marked));
    let mut seen = Seen(Vec::new());
    let ctx = ToolContext::new("/");
    let cancel = CancelToken::new();
    for name in ["srv__look", "echo"] {
        let call = ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: json!({}),
            extra_content: None,
        };
        judge_tool_call(&tools, &call, &mut seen, &ctx, &cancel);
    }
    let marked = crate::ToolTraits {
        mcp: true,
        read_only: true,
        ..Default::default()
    };
    assert_eq!(seen.0, [marked, crate::ToolTraits::default()]);
}

/// A call its tool's check refuses fails at once: no hook is asked, so
/// no prompt or reviewer waits on a call bound to fail.
#[test]
fn a_call_its_tool_refuses_skips_the_hooks() {
    struct Picky;
    impl crate::Tool for Picky {
        fn name(&self) -> &str {
            "picky"
        }
        fn description(&self) -> &str {
            "Picky"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn check(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<(), String> {
            match call.arguments["what"].as_str() {
                Some("known") => Ok(()),
                other => Err(format!("no such thing: {other:?}")),
            }
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, "done")
        }
    }
    struct Asked(usize);
    impl Hooks for Asked {
        fn before_tool_call(&mut self, _call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            self.0 += 1;
            ToolDecision::Allow
        }
    }

    let mut tools = ToolRegistry::new();
    tools.insert(Arc::new(Picky));
    let call = |what: &str| ToolCall {
        id: "c".into(),
        name: "picky".into(),
        arguments: json!({ "what": what }),
        extra_content: None,
    };
    let mut asked = Asked(0);
    let (ctx, cancel) = (ToolContext::new("/"), CancelToken::new());
    let run = |what: &str, asked: &mut Asked| {
        execute_tool(&tools, &call(what), asked, &ctx, &cancel, &mut |_| {})
    };
    let refused = run("unknown", &mut asked);
    assert!(refused.is_error);
    assert!(refused.plain_text().contains("no such thing"));
    assert_eq!(asked.0, 0);
    let done = run("known", &mut asked);
    assert_eq!(done.plain_text(), "done");
    assert_eq!(asked.0, 1);
}

#[test]
fn hook_can_block_a_tool_call() {
    struct DenyEcho;
    impl Hooks for DenyEcho {
        fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            if call.name == "echo" {
                ToolDecision::Block {
                    reason: "user denied".into(),
                }
            } else {
                ToolDecision::Allow
            }
        }
    }

    let echo = Arc::new(EchoTool::default());
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::ToolUse,
            ),
            text_reply("ok"),
        ]),
        registry_with(echo.clone()),
    );
    collect(&mut agent, "go", &mut DenyEcho);

    assert!(echo.executed.lock().unwrap().is_empty());
    let Message::ToolResult(result) = &agent.messages()[2] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.plain_text().contains("user denied"));
}

#[test]
fn length_stop_fails_tool_calls_without_running_them() {
    let echo = Arc::new(EchoTool::default());
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::Length,
            ),
            text_reply("retry"),
        ]),
        registry_with(echo.clone()),
    );
    collect(&mut agent, "go", &mut NoHooks);

    assert!(echo.executed.lock().unwrap().is_empty());
    let Message::ToolResult(result) = &agent.messages()[2] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.plain_text().contains("cut off"));
}

/// A reply whose reasoning goes round the same two thoughts.
fn looping_reply() -> AssistantMessage {
    let head = "The table is only filled under the x86-ir verb, which matters here.\n";
    let circle = "Actually, I should present the scope question to the owner first.\n\
OK let me just start implementing and see what happens.\n";
    AssistantMessage {
        content: vec![
            AssistantContent::Thinking {
                text: format!("{head}{}", circle.repeat(60)),
                signature: Some("sealed".into()),
                redacted: false,
            },
            AssistantContent::Text {
                text: "half an answer".into(),
            },
        ],
        stop_reason: StopReason::Stop,
        ..text_reply("")
    }
}

#[test]
fn a_looping_reply_is_cut_and_tried_again_without_reasoning() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        looping_reply(),
        text_reply("I will ask the owner."),
    ]));
    let mut agent = Agent::new(
        provider.clone(),
        ToolRegistry::new(),
        ModelSpec {
            thinking: ThinkingLevel::High,
            ..model()
        },
        PathBuf::from("."),
    );
    let events = collect(&mut agent, "go", &mut NoHooks);

    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "user", "assistant"]
    );
    // The cut reply keeps the reasoning before the circling, unsealed,
    // and none of the answer; it is no failure.
    let Message::Assistant(cut) = &agent.messages()[1] else {
        panic!("expected the cut reply");
    };
    assert_eq!(cut.stop_reason, StopReason::Length);
    assert_eq!(cut.plain_text(), "");
    let [AssistantContent::Thinking {
        text, signature, ..
    }] = cut.content.as_slice()
    else {
        panic!("expected the kept reasoning alone: {:?}", cut.content);
    };
    assert!(text.starts_with("The table is only filled"));
    assert!(text.matches("scope question").count() <= 2, "{text}");
    assert!(signature.is_none());
    // The nudge names what the model went round, and the next call is
    // made without reasoning.
    let Message::User(nudge) = &agent.messages()[2] else {
        panic!("expected the nudge");
    };
    assert!(nudge.plain_text().contains("- Actually, I should present"));
    assert_eq!(
        *provider.thinking_seen.lock().unwrap(),
        vec![ThinkingLevel::High, ThinkingLevel::Off]
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ThinkingLoop { retrying: true, report } if report.thoughts.len() == 2
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        AgentEvent::ThinkingLoop {
            retrying: false,
            ..
        }
    )));
}

#[test]
fn a_second_loop_stops_the_run() {
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![looping_reply(), looping_reply(), text_reply("never")]),
        ToolRegistry::new(),
    );
    let events = collect(&mut agent, "go", &mut NoHooks);

    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "user", "assistant"]
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ThinkingLoop {
            retrying: false,
            ..
        }
    )));
    assert!(matches!(events.last(), Some(AgentEvent::AgentEnd)));
}

#[test]
fn a_stop_by_the_user_is_no_loop() {
    let mut provider = ScriptedProvider::new(vec![looping_reply()]);
    provider.cancel_on_call = Some(0);
    let (mut agent, _) = agent(provider, ToolRegistry::new());
    let events = collect(&mut agent, "go", &mut NoHooks);
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ThinkingLoop { .. })));
    assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
}

/// A reply that ran to the output limit with only some reasoning: what a
/// server that holds back an unfinished tool call sends.
fn empty_cut_off_reply() -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantContent::Thinking {
            text: "Let me check the journal mode.".into(),
            signature: Some("sealed".into()),
            redacted: false,
        }],
        stop_reason: StopReason::Length,
        ..text_reply("")
    }
}

#[test]
fn an_empty_reply_cut_off_by_the_limit_is_told_and_tried_again() {
    let provider = Arc::new(ScriptedProvider::new(vec![
        empty_cut_off_reply(),
        text_reply("Done in a smaller step."),
    ]));
    let mut agent = Agent::new(
        provider.clone(),
        ToolRegistry::new(),
        ModelSpec {
            thinking: ThinkingLevel::High,
            ..model()
        },
        PathBuf::from("."),
    );
    let events = collect(&mut agent, "go", &mut NoHooks);

    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "user", "assistant"]
    );
    let Message::Assistant(cut) = &agent.messages()[1] else {
        panic!("expected the cut reply");
    };
    let [AssistantContent::Thinking { signature, .. }] = cut.content.as_slice() else {
        panic!("expected the reasoning alone: {:?}", cut.content);
    };
    assert!(signature.is_none());
    let Message::User(nudge) = &agent.messages()[2] else {
        panic!("expected the nudge");
    };
    assert!(nudge.plain_text().contains("output limit"));
    // The reasoning level stays: the reasoning was not what ran away.
    assert_eq!(
        *provider.thinking_seen.lock().unwrap(),
        vec![ThinkingLevel::High, ThinkingLevel::High]
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::EmptyCutOff { retrying: true })));
}

#[test]
fn a_second_empty_cut_off_or_loop_stops_the_run() {
    for second in [empty_cut_off_reply(), looping_reply()] {
        let (mut agent, _) = agent(
            ScriptedProvider::new(vec![empty_cut_off_reply(), second, text_reply("never")]),
            ToolRegistry::new(),
        );
        let events = collect(&mut agent, "go", &mut NoHooks);

        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "user", "assistant"]
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::EmptyCutOff { retrying: false }
                | AgentEvent::ThinkingLoop {
                    retrying: false,
                    ..
                }
        )));
        assert!(matches!(events.last(), Some(AgentEvent::AgentEnd)));
    }
}

#[test]
fn provider_error_halts_the_run() {
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![AssistantMessage::failed(
            "scripted",
            "test",
            StopReason::Error,
            "boom",
        )]),
        ToolRegistry::new(),
    );
    let events = collect(&mut agent, "go", &mut NoHooks);

    assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
    assert!(matches!(events.last(), Some(AgentEvent::AgentEnd)));
    let Message::Assistant(reply) = &agent.messages()[1] else {
        panic!("expected assistant");
    };
    assert_eq!(reply.error_message.as_deref(), Some("boom"));
}

#[test]
fn retry_asks_again_without_the_failed_reply() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            AssistantMessage::failed("scripted", "test", StopReason::Error, "limit"),
            text_reply("done"),
        ]),
        ToolRegistry::new(),
    );
    collect(&mut agent, "go", &mut NoHooks);
    agent.retry(&mut NoHooks, &CancelToken::new(), &mut |_| {});

    // The second call saw the request alone, not the failure after it.
    assert_eq!(roles(&provider.seen_requests()[1]), vec!["user"]);
    assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
    let Message::Assistant(reply) = &agent.messages()[1] else {
        panic!("expected assistant");
    };
    assert_eq!(reply.plain_text(), "done");
}

#[test]
fn retry_closes_the_calls_of_a_failed_reply_unrun() {
    let echo = Arc::new(EchoTool::default());
    let mut failed = tool_reply(
        vec![("c1", "echo", json!({ "text": "x" }))],
        StopReason::Error,
    );
    failed.error_message = Some("stream interrupted".into());
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![failed, text_reply("done")]),
        registry_with(echo.clone()),
    );
    collect(&mut agent, "go", &mut NoHooks);
    agent.retry(&mut NoHooks, &CancelToken::new(), &mut |_| {});

    assert!(echo.executed.lock().unwrap().is_empty());
    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result", "assistant"]
    );
    let Message::ToolResult(result) = &agent.messages()[2] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.plain_text().contains("Not run"));
}

#[test]
fn cancellation_during_stream_skips_tool_execution() {
    let echo = Arc::new(EchoTool::default());
    let mut provider = ScriptedProvider::new(vec![
        tool_reply(
            vec![("c1", "echo", json!({ "text": "x" }))],
            StopReason::ToolUse,
        ),
        text_reply("never"),
    ]);
    provider.cancel_on_call = Some(0);
    let (mut agent, provider) = agent(provider, registry_with(echo.clone()));
    collect(&mut agent, "go", &mut NoHooks);

    assert!(echo.executed.lock().unwrap().is_empty());
    assert_eq!(provider.seen_requests().len(), 1);
    let Message::ToolResult(result) = &agent.messages()[2] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.plain_text().contains("cancelled"));
}

#[test]
fn steering_queued_mid_run_is_delivered_before_the_next_model_call() {
    let echo = Arc::new(EchoTool::default());
    let mut registry = ToolRegistry::new();
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_reply(
            vec![("c1", "echo", json!({ "text": "x" }))],
            StopReason::ToolUse,
        ),
        text_reply("adjusted"),
    ]));
    let mut agent = Agent::new(provider.clone(), registry.clone(), model(), "/tmp".into());
    let steering_echo = Arc::new(EchoTool {
        executed: Default::default(),
        steer_on_execute: Some((agent.queues(), "actually, stop after this".into())),
        pause_on_execute: None,
    });
    registry.insert(steering_echo);
    *agent.tools_mut() = registry;
    drop(echo);

    let events = collect(&mut agent, "go", &mut NoHooks);

    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result", "user", "assistant"]
    );
    let second_request = &provider.seen_requests()[1];
    assert!(matches!(
        &second_request[3],
        Message::User(u) if u.plain_text() == "actually, stop after this"
    ));
    assert!(events.contains(&AgentEvent::QueueUpdate {
        steering: 0,
        follow_up: 0
    }));
}

#[test]
fn the_context_note_rides_on_each_request_but_stays_out_of_the_transcript() {
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(EchoTool::default()));
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_reply(
            vec![("c1", "echo", json!({ "text": "x" }))],
            StopReason::ToolUse,
        ),
        text_reply("done"),
    ]));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let mut agent = Agent::new(provider.clone(), registry, model(), "/tmp".into())
        .with_session_id(Some("s1".into()))
        .with_context_note(Arc::new(move |session| {
            assert_eq!(session, Some("s1"));
            let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(format!("note {n}"))
        }));

    collect(&mut agent, "go", &mut NoHooks);

    let requests = provider.seen_requests();
    let last_text = |messages: &[Message]| match messages.last() {
        Some(Message::User(user)) => match user.content.last() {
            Some(UserContent::Text { text }) => text.clone(),
            _ => String::new(),
        },
        Some(Message::ToolResult(result)) => match result.content.last() {
            Some(ToolResultContent::Text { text }) => text.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    assert_eq!(
        last_text(&requests[0]),
        "<system-reminder>\nnote 0\n</system-reminder>"
    );
    assert_eq!(
        last_text(&requests[1]),
        "<system-reminder>\nnote 1\n</system-reminder>"
    );
    assert!(!format!("{:?}", agent.messages()).contains("system-reminder"));
}

#[test]
fn pause_stops_after_the_current_step_and_resume_continues() {
    let mut registry = ToolRegistry::new();
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_reply(
            vec![("c1", "echo", json!({ "text": "x" }))],
            StopReason::ToolUse,
        ),
        text_reply("done after resume"),
    ]));
    let mut agent = Agent::new(provider.clone(), registry.clone(), model(), "/tmp".into());
    let echo = Arc::new(EchoTool {
        executed: Default::default(),
        steer_on_execute: None,
        pause_on_execute: Some(agent.queues()),
    });
    registry.insert(echo);
    *agent.tools_mut() = registry;

    // The tool asks to pause: the loop runs that step, then stops before the
    // next model call, emitting Paused.
    let events = collect(&mut agent, "go", &mut NoHooks);
    assert!(events.contains(&AgentEvent::Paused));
    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result"]
    );
    assert_eq!(provider.seen_requests().len(), 1);

    // Resuming continues on the existing transcript — the model is called
    // again with no new user message — and finishes.
    let mut resumed = Vec::new();
    agent.resume(&mut NoHooks, &CancelToken::new(), &mut |e| resumed.push(e));
    assert!(!resumed.contains(&AgentEvent::Paused));
    assert_eq!(provider.seen_requests().len(), 2);
    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result", "assistant"]
    );
}

/// A tool under a given name that logs its start and end; with a
/// meeting, it waits until that many calls have started, so calls run one
/// after another would never all meet.
struct ProbeTool {
    name: &'static str,
    log: Arc<std::sync::Mutex<Vec<String>>>,
    /// The calls started so far, and how many must start to meet.
    meeting: Option<(Arc<Meeting>, usize)>,
}

type Meeting = (std::sync::Mutex<usize>, std::sync::Condvar);

impl crate::Tool for ProbeTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "probe"
    }
    fn parameters(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        self.log.lock().unwrap().push(format!("start {}", call.id));
        let mut met = true;
        if let Some((meeting, expected)) = &self.meeting {
            let (started, changed) = &**meeting;
            let mut count = started.lock().unwrap();
            *count += 1;
            changed.notify_all();
            let (count, timeout) = changed
                .wait_timeout_while(count, std::time::Duration::from_secs(5), |n| *n < *expected)
                .unwrap();
            met = !timeout.timed_out();
            drop(count);
        }
        // The first call finishes last, so ends come out of order.
        if call.id == "r1" {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        on_update(ToolUpdate::Output(format!("{} working", call.id)));
        self.log.lock().unwrap().push(format!("end {}", call.id));
        ToolResultMessage::text(call, if met { "met" } else { "alone" })
    }
}

fn probes(meeting: usize) -> (ToolRegistry, Arc<std::sync::Mutex<Vec<String>>>) {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let meet = Arc::new((std::sync::Mutex::new(0), std::sync::Condvar::new()));
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(ProbeTool {
        name: "read",
        log: Arc::clone(&log),
        meeting: Some((meet, meeting)),
    }));
    registry.insert(Arc::new(ProbeTool {
        name: "edit",
        log: Arc::clone(&log),
        meeting: None,
    }));
    (registry, log)
}

fn results(agent: &Agent) -> Vec<(String, String)> {
    agent
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(r) => Some((r.tool_call_id.clone(), r.plain_text())),
            _ => None,
        })
        .collect()
}

/// Reads of one step run side by side: they all start before any
/// ends, each ends as it finishes, and their results keep the model's
/// order. A blocked one among them is answered in its place.
#[test]
fn calls_that_change_nothing_run_side_by_side() {
    struct DenyR3;
    impl Hooks for DenyR3 {
        fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            if call.id == "r3" {
                ToolDecision::Block {
                    reason: "denied".into(),
                }
            } else {
                ToolDecision::Allow
            }
        }
    }
    let (registry, _) = probes(2);
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![
                    ("r1", "read", json!({ "path": "a" })),
                    ("r2", "read", json!({ "path": "b" })),
                    ("r3", "read", json!({ "path": "c" })),
                ],
                StopReason::ToolUse,
            ),
            text_reply("done"),
        ]),
        registry,
    );
    let events = collect(&mut agent, "go", &mut DenyR3);
    let results = results(&agent);
    let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["r1", "r2", "r3"]);
    assert_eq!(results[0].1, "met");
    assert_eq!(results[1].1, "met");
    assert!(results[2].1.contains("denied"));
    // Ends come as calls finish: the slow first one last; its progress
    // still reaches the panel.
    let ends: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd { result } => Some(result.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ends.last(), Some(&"r1"), "{ends:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolExecutionUpdate { tool_call_id, .. } if tool_call_id == "r1"
    )));
}

/// A call that may change something runs alone: what comes before it
/// has ended when it starts, and what comes after starts once it ended.
#[test]
fn a_change_runs_alone_between_the_reads_around_it() {
    let (registry, log) = probes(1);
    let (mut agent, _) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![
                    ("r1", "read", json!({ "path": "a" })),
                    ("e1", "edit", json!({ "path": "a" })),
                    ("r2", "read", json!({ "path": "a" })),
                ],
                StopReason::ToolUse,
            ),
            text_reply("done"),
        ]),
        registry,
    );
    collect(&mut agent, "go", &mut NoHooks);
    assert_eq!(
        *log.lock().unwrap(),
        ["start r1", "end r1", "start e1", "end e1", "start r2", "end r2"]
    );
}

#[test]
fn what_runs_alongside_is_what_changes_nothing_and_asks_nothing() {
    let tools = ToolRegistry::new();
    let call = |name: &str, arguments| ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments,
        extra_content: None,
    };
    for together in [
        call("read", json!({ "path": "a" })),
        call("fetch", json!({ "url": "https://x" })),
        call("task", json!({ "agent": "a", "prompt": "p" })),
        call("shell", json!({ "command": "rg foo src" })),
    ] {
        assert!(runs_alongside(&together, &tools), "{}", together.name);
    }
    for alone in [
        call("edit", json!({})),
        call("write", json!({})),
        call("question", json!({})),
        call("shell", json!({ "command": "cargo build" })),
        call("mcp_tool", json!({})),
    ] {
        assert!(!runs_alongside(&alone, &tools), "{}", alone.name);
    }
}

/// Two calls in one step, the first asking to pause: the pause stops
/// before the second instead of waiting out the whole step.
fn pausing_agent(replies: Vec<AssistantMessage>) -> (Agent, Arc<EchoTool>, Arc<ScriptedProvider>) {
    let mut registry = ToolRegistry::new();
    let provider = Arc::new(ScriptedProvider::new(replies));
    let mut agent = Agent::new(provider.clone(), registry.clone(), model(), "/tmp".into());
    let echo = Arc::new(EchoTool {
        executed: Default::default(),
        steer_on_execute: None,
        pause_on_execute: Some(agent.queues()),
    });
    registry.insert(echo.clone());
    *agent.tools_mut() = registry;
    (agent, echo, provider)
}

fn two_calls() -> AssistantMessage {
    tool_reply(
        vec![
            ("c1", "echo", json!({ "text": "a" })),
            ("c2", "echo", json!({ "text": "b" })),
        ],
        StopReason::ToolUse,
    )
}

#[test]
fn a_pause_stops_between_the_calls_of_a_step_and_resume_runs_the_rest() {
    let (mut agent, echo, provider) =
        pausing_agent(vec![two_calls(), text_reply("done after resume")]);
    let events = collect(&mut agent, "go", &mut NoHooks);
    assert!(events.contains(&AgentEvent::Paused));
    // Only the first call ran; the second waits, unanswered.
    assert_eq!(*echo.executed.lock().unwrap(), vec!["c1"]);
    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result"]
    );

    // Resuming runs the waiting call first, then asks the model.
    let mut resumed = Vec::new();
    agent.resume(&mut NoHooks, &CancelToken::new(), &mut |e| resumed.push(e));
    assert_eq!(*echo.executed.lock().unwrap(), vec!["c1", "c2"]);
    assert_eq!(provider.seen_requests().len(), 2);
    assert_eq!(
        roles(agent.messages()),
        vec![
            "user",
            "assistant",
            "tool_result",
            "tool_result",
            "assistant"
        ]
    );
}

#[test]
fn a_new_request_over_a_pause_answers_the_calls_it_left() {
    let (mut agent, echo, provider) = pausing_agent(vec![two_calls(), text_reply("fresh")]);
    collect(&mut agent, "go", &mut NoHooks);
    // A new request instead of a resume: the waiting call is not run, but
    // still gets a result, so the transcript stays valid.
    collect(&mut agent, "something else", &mut NoHooks);
    assert_eq!(*echo.executed.lock().unwrap(), vec!["c1"]);
    let seen = provider.seen_requests();
    assert_eq!(
        roles(&seen[1]),
        vec!["user", "assistant", "tool_result", "tool_result", "user"]
    );
    assert!(agent.messages().iter().any(|m| matches!(
        m,
        Message::ToolResult(r) if r.tool_call_id == "c2" && r.is_error
    )));
}

#[test]
fn one_at_a_time_delivers_a_single_steering_message_per_turn() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            text_reply("first"),
            text_reply("second"),
            text_reply("third"),
        ]),
        ToolRegistry::new(),
    );
    agent.set_config(AgentConfig {
        steering_mode: QueueMode::OneAtATime,
        follow_up_mode: QueueMode::OneAtATime,
    });
    agent.queues().steer(UserMessage::text("s1"));
    agent.queues().steer(UserMessage::text("s2"));

    collect(&mut agent, "go", &mut NoHooks);

    // Prompt + s1 share the first turn (steering is polled before the
    // first call), s2 gets its own turn, then nothing is left.
    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(roles(&seen[0]), vec!["user", "user"]);
    assert_eq!(roles(&seen[1]), vec!["user", "user", "assistant", "user"]);
    assert_eq!(agent.queues().lens(), (0, 0));
}

#[test]
fn queued_messages_arrive_merged_by_default() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![text_reply("first")]),
        ToolRegistry::new(),
    );
    agent.queues().steer(UserMessage::text("fix the test"));
    agent.queues().steer(UserMessage::text("and the docs"));

    collect(&mut agent, "go", &mut NoHooks);

    // The prompt, then both queued messages as one.
    let seen = provider.seen_requests();
    assert_eq!(roles(&seen[0]), vec!["user", "user"]);
    let Message::User(merged) = &seen[0][1] else {
        panic!("a user message");
    };
    assert_eq!(merged.plain_text(), "fix the test\n\nand the docs");
}

#[test]
fn all_mode_drains_every_steering_message_at_once() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![text_reply("first")]),
        ToolRegistry::new(),
    );
    agent.set_config(AgentConfig {
        steering_mode: QueueMode::All,
        follow_up_mode: QueueMode::All,
    });
    agent.queues().steer(UserMessage::text("s1"));
    agent.queues().steer(UserMessage::text("s2"));

    collect(&mut agent, "go", &mut NoHooks);

    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(roles(&seen[0]), vec!["user", "user", "user"]);
}

#[test]
fn follow_up_runs_only_after_the_agent_would_stop() {
    let echo = Arc::new(EchoTool::default());
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::ToolUse,
            ),
            text_reply("finished task one"),
            text_reply("finished task two"),
        ]),
        registry_with(echo),
    );
    agent.queues().follow_up(UserMessage::text("now task two"));

    let events = collect(&mut agent, "task one", &mut NoHooks);

    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 3);
    // The follow-up is absent while tool calls are still being served.
    assert_eq!(roles(&seen[1]), vec!["user", "assistant", "tool_result"]);
    assert!(matches!(
        seen[2].last(),
        Some(Message::User(u)) if u.plain_text() == "now task two"
    ));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::AgentEnd))
            .count(),
        1
    );
}

#[test]
fn should_stop_after_turn_ends_the_run_early() {
    struct StopAfterFirst;
    impl Hooks for StopAfterFirst {
        fn should_stop_after_turn(&mut self, _message: &AssistantMessage) -> bool {
            true
        }
    }

    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::ToolUse,
            ),
            text_reply("unreachable"),
        ]),
        registry_with(Arc::new(EchoTool::default())),
    );
    collect(&mut agent, "go", &mut StopAfterFirst);

    assert_eq!(provider.seen_requests().len(), 1);
    assert_eq!(
        roles(agent.messages()),
        vec!["user", "assistant", "tool_result"]
    );
}

#[test]
fn a_refused_summary_falls_back_to_cutting_the_context_down() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            text_reply("first answer"),
            // The summary call goes on with the work instead.
            tool_reply(
                vec![("c1", "shell", json!({ "command": "ls" }))],
                StopReason::ToolUse,
            ),
            text_reply("second answer"),
        ]),
        ToolRegistry::new(),
    );
    agent.set_model(ModelSpec {
        context_window: 200,
        ..model()
    });
    agent.set_compaction(CompactionPolicy {
        reserve_tokens: 8092,
        keep_recent_tokens: 1,
        ..Default::default()
    });
    collect(&mut agent, &"x".repeat(1000), &mut NoHooks);
    let events = collect(&mut agent, "next", &mut NoHooks);

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionFailed { error, .. } if error.contains("called a tool")
    )));
    let Some(AgentEvent::Fitted {
        reason,
        budget,
        steps,
        ..
    }) = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Fitted { .. }))
    else {
        panic!("no fitting")
    };
    assert_eq!(*reason, CompactionReason::Threshold);
    assert_eq!(*budget, chars_for_tokens(100));
    assert_eq!(steps.last().map(|(step, _)| *step), Some(FitStep::Oldest));
    // The turn went on with the context cut down: the long prompt gave
    // way to a note, the answer and the new request stayed.
    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 3);
    assert!(matches!(
        &seen[2][0],
        Message::User(u) if u.plain_text().contains("earlier messages were left out")
    ));
    assert!(matches!(
        seen[2].last(),
        Some(Message::User(u)) if u.plain_text() == "next"
    ));
}

#[test]
fn what_is_summarised_is_cut_down_to_fit_the_window() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![text_reply(
            "SUMMARY OF EARLIER WORK: the files were read and the plan made.",
        )]),
        ToolRegistry::new(),
    );
    // 8 192 tokens, a 2 048 reserve: 15 360 characters of room.
    let read = ToolCall {
        id: "r1".into(),
        name: "read".into(),
        arguments: json!({ "path": "big.rs" }),
        extra_content: None,
    };
    let mut agent = {
        agent.set_compaction(CompactionPolicy {
            reserve_tokens: 2048,
            keep_recent_tokens: 10,
            ..Default::default()
        });
        agent.with_messages(vec![
            Message::User(UserMessage::text("read big.rs")),
            Message::Assistant(tool_reply(
                vec![("r1", "read", read.arguments.clone())],
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::text(&read, "b".repeat(40_000))),
            Message::Assistant(text_reply("Read it.")),
            Message::User(UserMessage::text("go on")),
        ])
    };
    let mut events = Vec::new();
    agent
        .compact(
            CompactionReason::Threshold,
            None,
            &CancelToken::new(),
            &mut |e| events.push(e),
        )
        .unwrap();
    let sent = &provider.seen_requests()[0];
    let Message::ToolResult(output) = &sent[2] else {
        panic!("{sent:?}")
    };
    assert!(output
        .plain_text()
        .contains("40000 characters of output left out"));
    let Some(AgentEvent::Compacted { call, .. }) = events
        .iter()
        .find(|e| matches!(e, AgentEvent::Compacted { .. }))
    else {
        panic!("no compaction")
    };
    assert_eq!(
        call.input_cut.first().map(|(s, _)| *s),
        Some(FitStep::ReadOutputs)
    );
}

#[test]
fn threshold_compaction_runs_before_the_model_call() {
    let (mut agent, provider) = agent(
        ScriptedProvider::new(vec![
            text_reply("first answer"),
            text_reply("SUMMARY OF EARLIER WORK: the user sent a long prompt and got an answer."),
            text_reply("second answer"),
        ]),
        ToolRegistry::new(),
    );
    // A 200-token window with the reserve capped at a quarter leaves a
    // 150-token threshold; the 1 000-char prompt (~250 tokens) crosses it
    // on the second run.
    agent.set_model(ModelSpec {
        context_window: 200,
        ..model()
    });
    agent.set_compaction(CompactionPolicy {
        reserve_tokens: 8092,
        keep_recent_tokens: 1,
        ..Default::default()
    });
    let long_prompt = "x".repeat(1000);
    collect(&mut agent, &long_prompt, &mut NoHooks);
    let events = collect(&mut agent, "next", &mut NoHooks);

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionStart {
            reason: CompactionReason::Threshold,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Compacted { summary, kept: 1, .. } if summary.starts_with("SUMMARY OF EARLIER WORK")
        )));
    let seen = provider.seen_requests();
    assert_eq!(seen.len(), 3);
    // The summary call ends with the summary request and carries no tail.
    assert!(matches!(
        seen[1].last(),
        Some(Message::User(u)) if u.plain_text() == CompactionPrompts::default().request
    ));
    // The new prompt is already in the transcript and is the kept tail,
    // so the summarised part is the first exchange.
    assert_eq!(roles(&seen[1]), vec!["user", "assistant", "user"]);
    // The real call starts from the summary and keeps the new prompt.
    assert!(matches!(
        &seen[2][0],
        Message::User(u) if u.plain_text().starts_with("Summary of the earlier conversation")
    ));
    assert_eq!(roles(&seen[2]), vec!["user", "user"]);
    assert_eq!(roles(agent.messages()), vec!["user", "user", "assistant"]);
}

#[test]
fn overflow_error_compacts_and_retries_once() {
    let (agent, provider) = agent(
        ScriptedProvider::new(vec![
            AssistantMessage::failed(
                "scripted",
                "test",
                StopReason::Error,
                "HTTP 400: maximum context length exceeded",
            ),
            text_reply("SUMMARY: the old question was answered; the new question is pending."),
            text_reply("recovered"),
        ]),
        ToolRegistry::new(),
    );
    let mut agent = agent.with_messages(vec![
        Message::User(UserMessage::text("old question")),
        Message::Assistant(text_reply("old answer")),
    ]);
    let events = collect(&mut agent, "new question", &mut NoHooks);

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionStart {
            reason: CompactionReason::Overflow,
            ..
        }
    )));
    assert_eq!(provider.seen_requests().len(), 3);
    let Message::Assistant(last) = agent.messages().last().unwrap() else {
        panic!("expected assistant");
    };
    assert_eq!(last.plain_text(), "recovered");
    assert!(
        agent.messages().iter().all(|m| !matches!(
            m,
            Message::Assistant(a) if a.stop_reason == StopReason::Error
        )),
        "the failed reply never entered the transcript"
    );
}

#[test]
fn a_refused_summary_call_reports_how_it_went_and_what_it_said() {
    // The model goes on with the work instead of summarising: a line of
    // narration and a call, as in a session where this slipped through.
    let mut went_on = tool_reply(
        vec![("c1", "shell", json!({ "command": "rg emit_fn" }))],
        StopReason::ToolUse,
    );
    went_on.content.insert(
        0,
        AssistantContent::Text {
            text: "Found it:".into(),
        },
    );
    went_on
        .content
        .insert(0, AssistantContent::thinking("where is the loop"));
    let (agent, _) = agent(ScriptedProvider::new(vec![went_on]), ToolRegistry::new());
    let mut agent = agent.with_messages(vec![
        Message::User(UserMessage::text("a")),
        Message::Assistant(text_reply("b")),
    ]);
    let mut events = Vec::new();
    let result = agent.compact(
        CompactionReason::Threshold,
        None,
        &CancelToken::new(),
        &mut |e| events.push(e),
    );
    assert!(result.is_err());
    let Some(AgentEvent::CompactionFailed { call, reply, .. }) = events
        .into_iter()
        .find(|e| matches!(e, AgentEvent::CompactionFailed { .. }))
    else {
        panic!("no failure reported");
    };
    let call = call.expect("a call was made");
    assert_eq!(call.reason, CompactionReason::Threshold);
    assert_eq!(call.stop_reason, StopReason::ToolUse);
    assert_eq!(call.tool_calls, vec!["shell".to_string()]);
    assert_eq!(call.thinking_chars, "where is the loop".len());
    assert_eq!(reply, "Found it:");
    assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
}

#[test]
fn failed_compaction_leaves_the_transcript_alone() {
    let (agent, _) = agent(
        ScriptedProvider::new(vec![AssistantMessage::failed(
            "scripted",
            "test",
            StopReason::Error,
            "boom",
        )]),
        ToolRegistry::new(),
    );
    let mut agent = agent.with_messages(vec![
        Message::User(UserMessage::text("a")),
        Message::Assistant(text_reply("b")),
    ]);
    let mut events = Vec::new();
    let result = agent.compact(
        CompactionReason::Manual,
        None,
        &CancelToken::new(),
        &mut |e| events.push(e),
    );
    assert!(result.is_err());
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::CompactionFailed { .. })));
    assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);

    let mut empty = Agent::new(
        Arc::new(ScriptedProvider::default()),
        ToolRegistry::new(),
        model(),
        PathBuf::from("/tmp"),
    );
    assert!(empty
        .compact(
            CompactionReason::Manual,
            None,
            &CancelToken::new(),
            &mut |_| {}
        )
        .is_err());
}

#[test]
fn compaction_streams_its_progress_and_reports_the_new_size() {
    let (agent, _) = agent(
        ScriptedProvider::new(vec![text_reply(
            "SUMMARY: the first question was asked and answered in full.",
        )]),
        ToolRegistry::new(),
    );
    let mut answered = text_reply("second answer");
    answered.usage = Usage {
        input: 50_000,
        ..Usage::default()
    };
    let mut agent = agent.with_messages(vec![
        Message::User(UserMessage::text("first question")),
        Message::Assistant(text_reply("first answer")),
        Message::User(UserMessage::text("second question")),
        Message::Assistant(answered),
    ]);
    let mut events = Vec::new();
    agent
        .compact(
            CompactionReason::Manual,
            None,
            &CancelToken::new(),
            &mut |e| events.push(e),
        )
        .unwrap();

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionStart { prompt_tokens, .. } if *prompt_tokens > 0
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionUpdate(StreamEvent::TextDelta(text)) if text.starts_with("SUMMARY")
    )));
    // The kept reply's usage measured the old context: the new size is
    // the estimate of the summary and the tail, not that figure.
    let after = events.iter().find_map(|e| match e {
        AgentEvent::Compacted { tokens_after, .. } => Some(*tokens_after),
        _ => None,
    });
    assert!(after.is_some_and(|tokens| tokens > 0 && tokens < 1_000));
    assert_eq!(context_tokens(agent.messages()), after.unwrap());
}

#[test]
fn clear_queue_returns_pending_messages() {
    let queues = QueueHandle::default();
    queues.steer(UserMessage::text("a"));
    queues.follow_up(UserMessage::text("b"));
    queues.follow_up(UserMessage::text("c"));
    assert!(queues.has_pending());

    let (steering, follow_up) = queues.clear();
    assert_eq!(steering.len(), 1);
    assert_eq!(follow_up.len(), 2);
    assert!(!queues.has_pending());
}
#[test]
fn chained_hooks_thread_replacements_and_stop_at_a_verdict() {
    struct Rewriter;
    impl Hooks for Rewriter {
        fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            ToolDecision::Replace {
                arguments: serde_json::json!({ "command": format!("{} --dry-run", call.arguments["command"].as_str().unwrap()) }),
            }
        }
        fn after_tool_call(
            &mut self,
            _call: &ToolCall,
            mut result: ToolResultMessage,
        ) -> ToolResultMessage {
            result.content.push(crate::ToolResultContent::Text {
                text: " +rewriter".into(),
            });
            result
        }
    }
    struct Judge(Vec<String>);
    impl Hooks for Judge {
        fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            let command = call.arguments["command"].as_str().unwrap().to_string();
            self.0.push(command.clone());
            if command.contains("rm") {
                ToolDecision::Block {
                    reason: "no rm".into(),
                }
            } else if command.contains("ls") {
                ToolDecision::Approve { arguments: None }
            } else {
                ToolDecision::Allow
            }
        }
        fn should_stop_after_turn(&mut self, _message: &AssistantMessage) -> bool {
            true
        }
    }
    struct Never;
    impl Hooks for Never {
        fn before_tool_call(&mut self, _call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            panic!("a verdict must stop the chain");
        }
    }
    let call = |command: &str| ToolCall {
        id: "c".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": command }),
        extra_content: None,
    };
    let ctx = ToolContext::new(PathBuf::from("/p"));

    // Replace flows into the next hook and out of the chain.
    let mut chain = ChainedHooks::new(vec![Box::new(Rewriter), Box::new(Judge(vec![]))]);
    assert_eq!(
        chain.before_tool_call(&call("cargo build"), &ctx),
        ToolDecision::Replace {
            arguments: serde_json::json!({ "command": "cargo build --dry-run" })
        }
    );
    // Approve keeps the rewritten arguments and skips the rest.
    let mut chain = ChainedHooks::new(vec![
        Box::new(Rewriter),
        Box::new(Judge(vec![])),
        Box::new(Never),
    ]);
    assert_eq!(
        chain.before_tool_call(&call("ls"), &ctx),
        ToolDecision::Approve {
            arguments: Some(serde_json::json!({ "command": "ls --dry-run" }))
        }
    );
    assert!(matches!(
        chain.before_tool_call(&call("rm x"), &ctx),
        ToolDecision::Block { .. }
    ));
    let result = chain.after_tool_call(&call("ls"), ToolResultMessage::text(&call("ls"), "out"));
    assert_eq!(result.plain_text(), "out +rewriter");
    assert!(chain.should_stop_after_turn(&text_reply("x")));
    let mut plain = ChainedHooks::new(vec![Box::new(NoHooks)]);
    assert_eq!(
        plain.before_tool_call(&call("x"), &ctx),
        ToolDecision::Allow
    );
    assert!(!plain.should_stop_after_turn(&text_reply("x")));
}

---
rule_denied: denied by the permission rules, which the user set; do not try to reach the same thing another way
plan_mode: plan mode: only reading is allowed; describe the change in the plan and wait for the user to leave plan mode
reviewer_blocked: blocked by the auto-mode reviewer: {{reason}} Do not reach the same outcome another way; continue with a safer alternative, or tell the user what you need them to run or allow.
user_denied: denied by the user
user_denied_session: denied by the user for this session
user_denied_reason: denied by the user: {{reason}}
unattended_subagent: a subagent cannot prompt; it may only do what the permission rules and mode already allow
unattended_headless: running headless with no one to ask; allowed only what the rules and mode permit
---
What the model reads in place of a tool's output when a call is refused, one
line per case above; termide puts "Tool call blocked: " in front. `{{reason}}`
takes the reviewer's reason or the words you denied with. A key left out
keeps the shipped text.

- rule_denied: a `deny` rule matched.
- plan_mode: plan mode refused a call that could change something.
- reviewer_blocked: the auto mode reviewer blocked the call.
- user_denied, user_denied_session, user_denied_reason: you denied it on the
  permission card — once, for the session, or with words of your own.
- unattended_subagent, unattended_headless: the call would have asked, and a
  subagent or a headless run had no one to ask.

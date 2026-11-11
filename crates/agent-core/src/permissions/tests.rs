use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn ctx() -> ToolContext {
    ToolContext::new(PathBuf::from("/proj"))
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: args,
        extra_content: None,
    }
}

fn bash(command: &str) -> ToolCall {
    call("shell", json!({ "command": command }))
}

fn rules(toml_text: &str) -> PermissionRules {
    toml::from_str(toml_text).unwrap()
}

/// Records requests and replays scripted answers.
struct Scripted {
    answers: Vec<PermissionAnswer>,
    asked: Arc<Mutex<Vec<PermissionRequest>>>,
}

impl PermissionPrompter for Scripted {
    fn ask(&mut self, request: &PermissionRequest) -> PermissionAnswer {
        self.asked.lock().unwrap().push(request.clone());
        if self.answers.is_empty() {
            PermissionAnswer::Deny
        } else {
            self.answers.remove(0)
        }
    }
}

fn hooks(
    rules: PermissionRules,
    answers: Vec<PermissionAnswer>,
) -> (PermissionHooks, Arc<Mutex<Vec<PermissionRequest>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let hooks = PermissionHooks::new(
        rules,
        Box::new(Scripted {
            answers,
            asked: asked.clone(),
        }),
    );
    (hooks, asked)
}

#[test]
fn toml_shape_round_trips() {
    let rules = rules(
        r#"
            mode = "edit"
            [shell]
            "git status*" = "allow"
            "git push*" = "ask"
            "rm -rf *" = "deny"
            [edit]
            "src/**" = "allow"
            ".env" = "deny"
            "#,
    );
    assert_eq!(rules.mode, Mode::Edit);
    assert_eq!(
        rules.evaluate("shell", "git push origin main"),
        Some(Decision::Ask)
    );
    assert_eq!(rules.evaluate("edit", "src/lib.rs"), Some(Decision::Allow));
    assert_eq!(rules.evaluate("edit", "README.md"), None);
    let text = toml::to_string(&rules).unwrap();
    assert_eq!(toml::from_str::<PermissionRules>(&text).unwrap(), rules);
    assert_eq!(PermissionRules::default().mode, Mode::Auto);
    // The earlier names still read.
    assert_eq!(rules_of_mode("accept-edits"), Mode::Edit);
    // `auto` is its own mode now, not a spelling of `all`.
    assert_eq!(rules_of_mode("auto"), Mode::Auto);
}

fn rules_of_mode(mode: &str) -> Mode {
    rules(&format!("mode = \"{mode}\"")).mode
}

#[test]
fn strictest_matching_rule_wins() {
    let mut rules = PermissionRules::default();
    rules.add("shell", "git *", Decision::Allow);
    rules.add("shell", "git push*", Decision::Ask);
    rules.add("shell", "*--force*", Decision::Deny);
    assert_eq!(rules.evaluate("shell", "git status"), Some(Decision::Allow));
    assert_eq!(
        rules.evaluate("shell", "git push origin"),
        Some(Decision::Ask)
    );
    assert_eq!(
        rules.evaluate("shell", "git push --force"),
        Some(Decision::Deny)
    );
}

#[test]
fn wildcard_semantics() {
    assert!(wildcard_match("src/**", "src/a/b.rs"));
    assert!(wildcard_match("**/.env*", ".env"));
    assert!(wildcard_match("**/.env*", "config/.env.local"));
    assert!(!wildcard_match("**/.env*", "environment.rs"));
    assert!(wildcard_match("cargo *", "cargo build --release"));
    assert!(!wildcard_match("cargo *", "cargo"));
    assert!(wildcard_match("cargo*", "cargo"));
    assert!(wildcard_match("?.rs", "a.rs"));
    assert!(wildcard_match("*", ""));
}

/// Every mode against every kind of call, with configured rules that
/// allow a command and an edit, and deny a secret: the table the
/// documentation gives.
#[test]
fn each_mode_decides_by_its_table() {
    use Decision::{Allow, Ask, Deny};
    let read_in = call("read", json!({ "path": "src/a.rs" }));
    let read_out = call("read", json!({ "path": "/etc/passwd" }));
    let web = call("fetch", json!({ "url": "https://docs.rs/x" }));
    let edit_in = call("edit", json!({ "path": "src/a.rs" }));
    let edit_out = call("write", json!({ "path": "/tmp/x" }));
    let secret = call("edit", json!({ "path": ".env" }));
    let look = bash("git status");
    let build = bash("cargo build");
    let other = bash("make install");
    let mcp = call("fs__search", json!({ "query": "x" }));
    let calls = [
        &read_in, &read_out, &web, &edit_in, &edit_out, &secret, &look, &build, &other, &mcp,
    ];
    let table = [
        (
            Mode::Ask,
            [Allow, Ask, Ask, Ask, Ask, Deny, Allow, Ask, Ask, Ask],
        ),
        (
            Mode::Plan,
            [Allow, Ask, Allow, Deny, Deny, Deny, Allow, Deny, Deny, Deny],
        ),
        (
            Mode::Edit,
            [Allow, Ask, Allow, Allow, Ask, Deny, Allow, Ask, Ask, Ask],
        ),
        (
            Mode::Configured,
            [Allow, Ask, Ask, Allow, Ask, Deny, Allow, Allow, Ask, Ask],
        ),
        (
            Mode::All,
            [
                Allow, Allow, Allow, Allow, Allow, Deny, Allow, Allow, Allow, Allow,
            ],
        ),
    ];
    for (mode, expected) in table {
        let mut rules = PermissionRules {
            mode,
            ..Default::default()
        };
        rules.add("shell", "cargo *", Decision::Allow);
        rules.add("edit", "src/**", Decision::Allow);
        rules.add("edit", "**/.env*", Decision::Deny);
        let (hooks, _) = hooks(rules, vec![]);
        let got: Vec<Decision> = calls.iter().map(|c| hooks.decide(c, &ctx())).collect();
        assert_eq!(got, expected, "{mode:?}");
    }
}

/// Moving on a launched program asks no one outside plan mode; a launch
/// and an evaluation stay with the mode, and a rule still wins. Their
/// subject names what runs, so "allow always" trusts that command.
#[test]
fn debug_steps_go_through_and_launches_stay_with_the_mode() {
    let step = call("debug", json!({ "action": "step_over" }));
    let start = call(
        "debug",
        json!({ "action": "start", "command": "debug-server", "params": { "port": "1" } }),
    );
    let evaluate = call(
        "debug",
        json!({ "action": "evaluate", "expression": "f(\n1)" }),
    );
    for (mode, expected) in [
        (Mode::Ask, [Decision::Allow, Decision::Ask, Decision::Ask]),
        (Mode::Auto, [Decision::Allow, Decision::Ask, Decision::Ask]),
        (Mode::Plan, [Decision::Deny, Decision::Deny, Decision::Deny]),
    ] {
        let (hooks, _) = hooks(
            PermissionRules {
                mode,
                ..Default::default()
            },
            vec![],
        );
        let got = [&step, &start, &evaluate].map(|c| hooks.decide(c, &ctx()));
        assert_eq!(got, expected, "{mode:?}");
    }
    let mut rules = PermissionRules {
        mode: Mode::Configured,
        ..Default::default()
    };
    rules.add("debug", "step_over", Decision::Deny);
    rules.add("debug", "start debug-server", Decision::Allow);
    let (hooks, _) = hooks(rules, vec![]);
    assert_eq!(hooks.decide(&step, &ctx()), Decision::Deny);
    assert_eq!(hooks.decide(&start, &ctx()), Decision::Allow);

    assert_eq!(subject_of(&start, &ctx()), "start debug-server");
    assert_eq!(
        subject_of(
            &call(
                "debug",
                json!({ "action": "start", "file": "/proj/app/main.py" })
            ),
            &ctx()
        ),
        "start app/main.py"
    );
    assert_eq!(subject_of(&evaluate, &ctx()), "evaluate f( 1)");
    assert_eq!(subject_of(&step, &ctx()), "step_over");
}

#[test]
fn session_answers_count_in_every_mode_but_all() {
    let mut rules = PermissionRules {
        mode: Mode::Ask,
        ..Default::default()
    };
    rules.add_session("shell", "make *", Decision::Allow);
    rules.add_session("shell", "rm *", Decision::Deny);
    let (hooks, _) = hooks(rules, vec![]);
    let handle = hooks.mode_handle();
    // `make *` is a broad pattern, yet in `auto` too an answer the user
    // gave for the session holds.
    for mode in [Mode::Ask, Mode::Edit, Mode::Configured, Mode::Auto] {
        handle.set(mode);
        assert_eq!(hooks.decide(&bash("make all"), &ctx()), Decision::Allow);
        assert_eq!(hooks.decide(&bash("rm x"), &ctx()), Decision::Deny);
    }
    // Everything is allowed in `all` anyway; a session refusal is not a
    // configured one.
    handle.set(Mode::All);
    assert_eq!(hooks.decide(&bash("rm x"), &ctx()), Decision::Allow);
}

#[test]
fn shell_commands_are_split_and_each_part_judged() {
    let mut rules = PermissionRules::default();
    rules.add("shell", "cargo *", Decision::Allow);
    rules.add("shell", "git push*", Decision::Ask);
    let (hooks, _) = hooks(rules, vec![]);
    let decide = |cmd: &str| hooks.decide(&bash(cmd), &ctx());

    assert_eq!(decide("cargo build"), Decision::Allow);
    assert_eq!(decide("cargo build && cargo test"), Decision::Allow);
    assert_eq!(
        decide("cargo build && git status"),
        Decision::Allow,
        "git status is read-only"
    );
    assert_eq!(decide("cargo build && rm -rf target"), Decision::Ask);
    assert_eq!(decide("cargo build; git push"), Decision::Ask);
    assert_eq!(
        decide("cargo run -- $(cat cmd)"),
        Decision::Ask,
        "substitution"
    );
    assert_eq!(decide("cargo run -- `cat cmd`"), Decision::Ask);
    assert_eq!(
        decide("echo 'a && b'"),
        Decision::Allow,
        "quotes are not separators"
    );
    assert_eq!(
        decide("ls > out.txt"),
        Decision::Ask,
        "redirection is a write"
    );
    assert_eq!(decide("find . -name '*.rs'"), Decision::Allow);
    assert_eq!(decide("find . -delete"), Decision::Ask);
    assert_eq!(decide("git log | head"), Decision::Allow);
    assert_eq!(
        decide("env rm -rf ."),
        Decision::Ask,
        "env runs its command"
    );
    assert_eq!(decide("git branch -D main"), Decision::Ask);
    assert_eq!(decide("git branch -vv"), Decision::Allow);
    assert_eq!(
        decide("if [ -f Cargo.toml ]; then cargo build; fi"),
        Decision::Allow
    );
    assert_eq!(decide("[ -d target ] && cargo test"), Decision::Allow);
    assert_eq!(decide("for f in a b; do echo $f; done"), Decision::Allow);
    // The header is no part, but it tells what `$f` reads.
    assert_eq!(decide("for f in a b; do cat $f; done"), Decision::Allow);
    assert_eq!(decide("if true; then rm -rf x; fi"), Decision::Ask);
    assert_eq!(
        decide("for f in $(ls); do cat $f; done"),
        Decision::Ask,
        "substitution in a loop header"
    );
}

#[test]
fn commands_run_through_a_back_door_are_not_look_only() {
    let (hooks, _) = hooks(PermissionRules::default(), vec![]);
    let decide = |cmd: &str| hooks.decide(&bash(cmd), &ctx());
    for command in [
        // bash 5.3, ksh93 and mksh run a command in `${ …; }`.
        "echo ${ touch x; }",
        "echo \"${ touch x; }\"",
        "echo ${|touch x; }",
        "cat <<EOF\n${ touch x; }\nEOF",
        // An array subscript is arithmetic, which substitutes even what
        // single quotes kept from the shell.
        "test -v 'a[$(touch x)]'",
        "[ -v 'a[$(touch x)]' ]",
        "[[ 'a[$(touch x)]' -eq 0 ]]",
        // Quotes and backslashes join an option back together.
        "find . -del''ete",
        "find . -de\\lete",
        "find . '-exec' rm {} +",
        "sort -'o' out in",
        "rg --'pre'=sh x",
    ] {
        assert_eq!(decide(command), Decision::Ask, "{command}");
    }
    // A value set by an earlier command of a session, taken as code.
    for command in [
        "[[ $x -eq 0 ]]",
        "[[ x -gt 3 ]]",
        "[ \"$n\" -lt 2 ]",
        "test 1 -ne \"$n\"",
        "test -v \"$x\"",
        "[[ -v $x ]]",
        "[ -v 'a[i]' ]",
        "echo ${x@P}",
        "echo ${a[@]@P}",
        "echo \"${!x}\"",
        "echo ${a[i]}",
        "echo ${a[$i]}",
        "echo ${s:i}",
        "echo ${s:0:n}",
    ] {
        assert_eq!(decide(command), Decision::Ask, "{command}");
    }
    for command in [
        "[ 1 -eq 1 ]",
        "[[ -10 -lt +3 ]]",
        "[ -v HOME ]",
        "[ -z \"$x\" ] && ls",
        "echo ${a[0]} ${a[@]} ${a[*]} ${#a[@]}",
        "echo ${s:1:2} ${s:-x} ${s:=y} ${s:+z}",
    ] {
        assert_eq!(decide(command), Decision::Allow, "{command}");
    }
    assert_eq!(decide("echo ${HOME} \"${PWD}\""), Decision::Allow);
    assert_eq!(decide("[ -n \"${PWD}\" ] && ls"), Decision::Allow);
    assert_eq!(decide("grep 'a[0-9]' src/x"), Decision::Allow);
}

fn ctx_in(shell: ShellSyntax) -> ToolContext {
    ToolContext {
        tool: crate::ToolTraits {
            shell,
            ..crate::ToolTraits::default()
        },
        ..ctx()
    }
}

#[test]
fn a_fish_command_is_judged_as_fish_runs_it() {
    let mut rules = PermissionRules::default();
    rules.add("shell", "ls *", Decision::Allow);
    rules.add("shell", "echo *", Decision::Allow);
    rules.add("shell", "rm *", Decision::Deny);
    let (hooks, _) = hooks(rules, vec![]);
    let fish = ctx_in(ShellSyntax::Fish);
    let decide = |cmd: &str| hooks.decide(&bash(cmd), &fish);
    for command in [
        // `(…)` substitutes in fish: no `allow` vouches for it.
        "ls (touch x)",
        "ls a(touch x)b",
        "cat (ls)",
        // `\'` keeps a fish string open past what POSIX takes for its end.
        r"echo 'a\' b' ; touch x",
        // An escape standing for another character.
        r"find . \x2ddelete",
        // What the reading cannot vouch for.
        "switch $x; case a; touch a; end",
        "cat <<EOF",
    ] {
        assert_eq!(decide(command), Decision::Ask, "{command}");
    }
    for command in [
        "ls -la",
        "git status; and git log -1",
        "test -f Cargo.toml; or echo missing",
        "for f in src/*.rs; wc -l $f; end",
        "begin; ls; echo done; end",
        "if test -d target; ls target; else; echo none; end",
        "echo `ls` '(x)'",
    ] {
        assert_eq!(decide(command), Decision::Allow, "{command}");
    }
    assert_eq!(decide("ls; and rm -r x"), Decision::Deny);
    assert_eq!(
        hooks.decide(&bash("ls (touch x)"), &ctx()),
        Decision::Allow,
        "POSIX reads the parenthesis as text"
    );
    assert!(!is_read_only_call(&bash("ls (touch x)"), ShellSyntax::Fish));
    assert!(!is_read_only_call(&bash("ls; end"), ShellSyntax::Fish));
    assert!(is_read_only_call(&bash("ls; and pwd"), ShellSyntax::Fish));
}

#[test]
fn a_zsh_parenthesis_inside_a_command_may_run_code() {
    let (hooks, _) = hooks(PermissionRules::default(), vec![]);
    let zsh = ctx_in(ShellSyntax::Zsh);
    let decide = |cmd: &str| hooks.decide(&bash(cmd), &zsh);
    for command in [
        "ls *(e:'touch x':)",
        "ls (e:'touch x':)",
        "echo =(touch x)",
        "echo ${(e)x}",
        "echo \"${(e)x}\"",
        "cat <<EOF\n${(e)x}\nEOF",
    ] {
        assert_eq!(decide(command), Decision::Ask, "{command}");
    }
    assert_eq!(decide("ls src | wc -l"), Decision::Allow);
    assert_eq!(decide("echo \"${HOME}\""), Decision::Allow);
}

#[test]
fn parts_resolve_their_program_from_the_directory_cd_left() {
    let resolved = |command: &str| -> Vec<(String, bool)> {
        shell_parts(command, Path::new("/proj"))
            .into_iter()
            .map(|p| (p.resolved, p.savable))
            .collect()
    };
    // Outside the project: absolute; inside: project-relative.
    assert_eq!(
        resolved("cd /tmp && ./astro/bin/pip install x && cd /proj/src && ./tool -v"),
        vec![
            ("cd /tmp".to_string(), true),
            ("/tmp/astro/bin/pip install x".to_string(), true),
            ("cd /proj/src".to_string(), true),
            ("src/tool -v".to_string(), true),
        ]
    );
    // `pushd` and `popd` are followed too; a bare name stays as written.
    assert_eq!(
        resolved("pushd /opt && popd && ./x; make"),
        vec![
            ("pushd /opt".to_string(), true),
            ("popd".to_string(), true),
            ("x".to_string(), true),
            ("make".to_string(), true),
        ]
    );
    // A `cd` that cannot be followed leaves the directory unknown, and a
    // relative program after it has no rule to be saved under.
    for unknown in [
        "cd $DIR && ./run",
        "cd - && ./run",
        "cd \"my dir\" && ./run",
    ] {
        let parts = resolved(unknown);
        assert_eq!(parts[1], ("./run".to_string(), false), "{unknown}");
    }
    // A substitution is never saved either.
    assert!(!resolved("echo $(date)")[0].1);
}

/// The command from a real session: it asked about `cd *`, the first
/// part, and asked again next time, since the rest stayed unanswered.
#[test]
fn a_compound_command_asks_about_each_part_left_and_remembers_each() {
    let command = "cd /tmp && python3 -m venv astro && ./astro/bin/pip install -q pyswisseph 2>&1 | tail -3; ./astro/bin/python -c \"import swisseph;print(swisseph.version)\"";
    let (mut hooks, asked) = hooks(
        PermissionRules::default(),
        vec![PermissionAnswer::AllowSession],
    );
    assert_eq!(
        hooks.before_tool_call(&bash(command), &ctx()),
        ToolDecision::Allow
    );
    let request = asked.lock().unwrap()[0].clone();
    // `cd` and `tail` only look; the rest is asked about, each with its
    // own rule, the programs resolved.
    let patterns: Vec<String> = request
        .parts
        .iter()
        .filter_map(|p| p.pattern.clone())
        .collect();
    assert_eq!(
        patterns,
        [
            "python3 *",
            "/tmp/astro/bin/pip *",
            "/tmp/astro/bin/python *"
        ]
    );
    assert_eq!(request.parts[0].text, "python3 -m venv astro");
    assert!(request.can_remember());
    // The same command, and another with the same programs, pass now.
    assert_eq!(hooks.decide(&bash(command), &ctx()), Decision::Allow);
    assert_eq!(
        hooks.decide(&bash("cd /tmp/astro && ./bin/pip list"), &ctx()),
        Decision::Allow
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn a_part_from_an_unknown_directory_is_answered_for_this_call_only() {
    let (mut hooks, asked) = hooks(
        PermissionRules::default(),
        vec![PermissionAnswer::AllowSession],
    );
    hooks.before_tool_call(&bash("cd $BUILD && ./run"), &ctx());
    let request = asked.lock().unwrap()[0].clone();
    assert_eq!(request.parts.len(), 1);
    assert_eq!(request.parts[0].pattern, None);
    assert!(!request.can_remember());
    // Nothing was recorded, so it asks again.
    assert_eq!(
        hooks.decide(&bash("cd $BUILD && ./run"), &ctx()),
        Decision::Ask
    );
    // And a rule written for the text cannot vouch for it.
    let mut rules = PermissionRules::default();
    rules.add("shell", "./run*", Decision::Allow);
    let (hooks, _) = super::tests::hooks(rules, vec![]);
    assert_eq!(
        hooks.decide(&bash("cd $BUILD && ./run"), &ctx()),
        Decision::Ask
    );
}

#[test]
fn pointing_a_stream_elsewhere_is_not_writing_a_file() {
    for look in [
        "ls 2>&1",
        "rg TODO 2>/dev/null",
        "ls 2> /dev/null",
        "cat x &>/dev/null",
        "echo hi >&2",
        "cd /tmp",
    ] {
        assert!(is_read_only_command(look), "{look}");
    }
    for write in ["ls > out", "ls 2>err.txt", "echo x >> log", "cat x 2> err"] {
        assert!(!is_read_only_command(write), "{write}");
    }
}

/// The look-only list must not let through a command that writes or runs
/// another program through its arguments.
#[test]
fn arguments_that_write_or_run_disqualify_a_look_only_command() {
    for look in [
        "env",
        "date",
        "date +%F",
        "sort -rn counts.txt",
        "tree -L 2",
        "uniq -c words.txt",
        "rg TODO src",
        "rg --pretty TODO",
        "rg --pretty --hyperlink-format=default TODO",
        "git log --oneline --one",
        "file README.md",
        "find . -name '*.rs' -print",
        "git status",
        "git diff --stat",
        "git log --oneline -5",
        "git branch",
        "git branch -a -vv",
        "git branch --list 'feat/*'",
        "git branch --merged",
        "git branch --sort=-committerdate --format=%(refname:short)",
        "git remote",
        "git remote -v",
        "git remote show origin",
        "git remote get-url origin",
    ] {
        assert!(is_read_only_command(look), "{look}");
    }
    for write in [
        "env rm -rf .",
        "env FOO=1 sh -c 'rm -rf .'",
        "date -s 12:00",
        "date --set=12:00",
        "sort -o out.txt in.txt",
        "sort -ro out.txt in.txt",
        "sort --output=out.txt in.txt",
        "tree -o listing.txt",
        "tree -R -H .",
        "uniq in.txt out.txt",
        "rg --pre ./run.sh TODO",
        "rg --pre=./run.sh TODO",
        "rg --pre-glob '*.pdf' TODO",
        "rg --hostname-bin=./name.sh --hyperlink-format=default TODO",
        "rg --hostname-bin ./name.sh TODO",
        "sort --compress-program=./run.sh big.txt",
        "sort --compress-prog ./run.sh big.txt",
        "sort --out=out.txt in.txt",
        "git diff --out=patch.diff",
        "git grep --open=vim foo",
        "file -C -m magic",
        "find . -fprint list.txt",
        "find . -fls list.txt",
        "find . -execdir rm {} +",
        "find . -okdir rm {} ;",
        "find . -fprintf list.txt %p",
        "find . -fprint0 list.txt",
        "find . -delete",
        "git -c core.pager=./run.sh log",
        "git --exec-path=./bin status",
        "git --output=x diff",
        "git diff --output=patch.diff",
        "git log --output=log.txt",
        "git branch feature",
        "git branch -D feature",
        "git branch --delete feature",
        "git branch -m old new",
        "git branch --set-upstream-to=origin/main",
        "git remote add fork https://example.com/fork.git",
        "git remote remove origin",
        "git remote set-url origin https://example.com/x.git",
        "git remote prune origin",
        "git -C /tmp status",
    ] {
        assert!(!is_read_only_command(write), "{write}");
    }
}

/// `sed` looks only while it prints: addresses with `p`, `l`, `=`, `q`,
/// `Q` or `d`; `git tag` only while it lists.
#[test]
fn a_printing_sed_and_a_listing_git_tag_are_look_only() {
    for look in [
        "sed -n '600,660p' crates/i18n/src/runtime.rs",
        "sed -n 5p file",
        "sed -n '1,40p;80,90p' a.rs",
        "sed -n '/fn render/,/^}/p' src/main.rs",
        "sed -n \"/struct Foo/,+0p\" x",
        "sed -nE '$p'",
        "sed '10q' notes.txt",
        "sed -n -e 1p -e '$=' f",
        "sed --quiet --expression=3,7p f",
        "sed '/^#/d' config",
        "git tag",
        "git tag --sort=-v:refname",
        "git tag -l 'v0.*'",
        "git tag -n5 --list",
        "git tag --contains=HEAD",
        "git tag --merged main",
        "git tag --sort -v:refname",
        "git branch -a --no-merged main",
        "git branch --merged main",
        "git branch --sort -committerdate --format %(refname:short)",
        "git branch --points-at HEAD",
    ] {
        assert!(is_read_only_command(look), "{look}");
    }
    for write in [
        "sed -i 's/a/b/' f",
        "sed -i.bak 1d f",
        "sed -ni 1p f",
        "sed --in-place 1d f",
        "sed -I '' 1d f",
        "sed 's/a/b/' f",
        "sed -n '1w out.txt' f",
        "sed -n '1p;w out' f",
        "sed -f script.sed f",
        "sed -n '1e date' f",
        "sed -n \"${N}p\" f",
        "sed -n '1,5{p}' f",
        "sed",
        "git tag v1.0",
        "git tag -d v1.0",
        "git tag -a v1 -m msg",
        "git tag -f v1",
        "git branch --merged main feature",
        "git tag --contains HEAD v2",
    ] {
        assert!(!is_read_only_command(write), "{write}");
    }
}

/// Git's reading subcommands are look-only; the forms of the same
/// subcommands that write, and `-c`, which can run a program, are not.
#[test]
fn reading_git_subcommands_are_look_only() {
    for look in [
        "git --no-pager log -3",
        "git -P show HEAD:a.rs",
        "git ls-tree HEAD src",
        "git cat-file -p HEAD",
        "git rev-list --count HEAD",
        "git shortlog -sn",
        "git describe --tags",
        "git grep -n foo",
        "git reflog",
        "git reflog -5",
        "git reflog show main",
        "git stash list",
        "git stash show -p",
        "git worktree list",
        "git merge-base HEAD main",
        "git for-each-ref refs/tags",
        "git show-ref",
        "git name-rev HEAD",
        "git count-objects -v",
        "git cherry -v main",
        "git range-diff a...b",
        "git config --get user.email",
        "git config -l",
        "git config get user.name",
        "git config user.email",
        "git config --global user.email",
    ] {
        assert!(is_read_only_command(look), "{look}");
    }
    for write in [
        "git -c core.pager=sh log",
        "git -C ../other log",
        "git --no-pager commit -m x",
        "git grep -O foo",
        "git grep --open-files-in-pager=vim foo",
        "git reflog expire --all",
        "git reflog delete HEAD@{1}",
        "git stash",
        "git stash drop",
        "git stash pop",
        "git worktree add ../x",
        "git worktree remove ../x",
        "git config user.email a@b",
        "git config --unset user.email",
        "git config --get x --add y",
        "git config -e",
        "git config set user.name x",
        "git config --global user.email a@b",
        "git config edit",
        "git rev-list --output=f HEAD",
    ] {
        assert!(!is_read_only_command(write), "{write}");
    }
}

/// Forge CLI commands (`gh`, `glab`, `tea`) that read are look-only;
/// those that change something, open a browser or send a raw request are
/// not.
#[test]
fn reading_forge_commands_are_look_only() {
    for look in [
        "gh issue list --state open --limit 30",
        "gh issue view 60 --comments",
        "gh pr list",
        "gh pr view 12 --json title -q .title",
        "gh pr diff 12",
        "gh pr checks 12",
        "gh run list -R termide/termide",
        "gh run view 123 --log",
        "gh release list",
        "gh repo view",
        "gh search issues crash",
        "gh -R termide/termide issue list",
        "gh issue list --state closed --limit 5",
        "glab issue list",
        "glab issue view 7 -c",
        "glab mr view 3 -F json",
        "glab mr diff 3",
        "glab ci list",
        "glab ci trace 991",
        "glab release view v1",
        "glab repo view -R group/proj",
        "glab milestone list",
        "tea issues",
        "tea issues 5",
        "tea issues list --state closed",
        "tea i ls -o json",
        "tea pulls",
        "tea pr 12",
        "tea pulls review-comments 12",
        "tea repos owner/name",
        "tea releases",
        "tea notifications",
        "tea comments list 5",
        "tea actions runs logs 3",
        "tea actions workflows list",
        "tea whoami",
    ] {
        assert!(is_read_only_command(look), "{look}");
    }
    for write in [
        "gh",
        "gh issue create -t x",
        "gh issue close 60",
        "gh issue view 60 --web",
        "gh pr merge 12",
        "gh pr checkout 12",
        "gh run rerun 123",
        "gh run download 123",
        "gh release create v1",
        "gh repo clone a/b",
        "gh api repos/a/b/issues -f title=x",
        "gh auth status --show-token",
        "gh -R a/b issue close 5",
        "glab",
        "glab api projects",
        "glab issue close 7",
        "glab issue view 7 --web",
        "glab mr merge 3",
        "glab mr approve 3",
        "glab ci view",
        "glab ci status",
        "glab ci retry 991",
        "glab release download v1",
        "glab repo clone a/b",
        "tea",
        "tea api /repos",
        "tea open",
        "tea issues close 5",
        "tea issues create -t x",
        "tea pulls merge 12",
        "tea pulls checkout 12",
        "tea pulls clean 12",
        "tea comments 5 hello",
        "tea notifications read",
        "tea times add 5 1h",
        "tea actions runs delete 3",
        "tea actions workflows dispatch ci",
        "tea clone a/b",
    ] {
        assert!(!is_read_only_command(write), "{write}");
    }
}

/// A refusal in plan mode names the part of a command that is not
/// look-only.
#[test]
fn a_plan_mode_refusal_names_what_is_not_look_only() {
    let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
    let reason = match guard.before_tool_call(
        &call("shell", json!({ "command": "rg x src && sed -i 1d f" })),
        &ctx(),
    ) {
        ToolDecision::Block { reason } => reason,
        _ => panic!("refused"),
    };
    assert!(
        reason.starts_with(&Refusals::default().plan_mode),
        "{reason}"
    );
    assert!(
        reason.ends_with("(not look-only: `sed -i 1d f`)"),
        "{reason}"
    );
    let ToolDecision::Block { reason } =
        guard.before_tool_call(&call("edit", json!({ "path": "f" })), &ctx())
    else {
        panic!("refused");
    };
    assert_eq!(reason, Refusals::default().plan_mode);
}

/// An inline script is one command, not the shell commands its lines
/// would be.
#[test]
fn an_inline_script_is_not_split_into_commands() {
    let parts = |command: &str| split_shell(command).parts;
    // A here-document's body is skipped, whatever it holds.
    assert_eq!(
        parts("python3 - <<'EOF'\nimport os; print(os.getcwd())\nif x | y:\n    pass\nEOF"),
        vec!["python3 - <<'EOF'"]
    );
    // The commands after it are commands again.
    assert_eq!(
        parts("cat <<EOF > notes.txt\nsome; text\nEOF\nls -la"),
        vec!["cat <<EOF > notes.txt", "ls -la"]
    );
    assert_eq!(
        parts("cat <<-EOF\n\tbody\n\tEOF\npwd"),
        vec!["cat <<-EOF", "pwd"]
    );
    // An escaped quote does not end a quoted script.
    assert_eq!(
        parts("python3 -c \"print(\\\"a; b\\\")\nx = 1 | 2\""),
        vec!["python3 -c \"print(\\\"a; b\\\")\nx = 1 | 2\""]
    );
    assert_eq!(parts("echo a\\;b && ls"), vec!["echo a\\;b", "ls"]);
    // A here-string is one word.
    assert_eq!(parts("cat <<< \"a;b\""), vec!["cat <<< \"a;b\""]);
    // An unquoted delimiter's body still expands a substitution; a quoted
    // one's does not.
    assert!(split_shell("cat <<EOF\n$(rm -rf x)\nEOF").has_substitution);
    assert!(!split_shell("cat <<'EOF'\n$(rm -rf x)\nEOF").has_substitution);
}

#[test]
fn split_shell_details() {
    let parsed = split_shell("a && b || c | d; e\nf");
    assert_eq!(parsed.parts, vec!["a", "b", "c", "d", "e", "f"]);
    assert!(!parsed.has_substitution);
    let quoted = split_shell("echo \"x; $(id)\" && ls");
    assert_eq!(quoted.parts, vec!["echo \"x; $(id)\"", "ls"]);
    assert!(quoted.has_substitution);
    assert_eq!(split_shell("   ").parts, vec![""]);
}

/// A look-only command that reads beyond the project asks, in the modes
/// where the `read` tool does, plan mode included, while one reading
/// inside it does not; a rule kept for it names the one command.
#[test]
fn look_only_commands_reading_outside_the_project_ask() {
    let home = std::env::var("HOME").unwrap_or_default();
    let outside = [
        "cat /etc/passwd",
        "head -n 5 ~/.ssh/id_rsa",
        "cat ~",
        "cat ~root/.profile",
        "grep -r token /Users/someone",
        "grep token ../other/.env",
        "grep -e x /etc/hosts",
        "rg secret ..",
        "rg --ignore-file=/etc/x y",
        "cat src/../../x",
        "cat .*/x",
        "cat {..,x}/y",
        "tr a b < /etc/passwd",
        "wc -l </etc/passwd",
        "cat -- /etc/passwd",
        "cat $HOME/.netrc",
        "cd .. && ls",
        "cd /etc && cat passwd",
        "cd $X && ls",
        "cd /tmp && git status",
        "find / -name id_rsa",
        "ls /",
        "env",
        "printenv",
        "date -f /etc/shadow",
    ];
    let inside = [
        "cat src/main.rs",
        "cat /proj/src/main.rs",
        "head -n 5 README.md",
        "grep -rn /api/ src",
        "grep -e /api/ src",
        "rg '/etc/passwd' src",
        "rg -g '!/target' TODO",
        "sed -n '/^fn/p' src/lib.rs",
        "ls",
        "ls -la src",
        "git log --oneline -5",
        "cat < README.md",
        "grep x 2>/dev/null",
        "cat /dev/null",
        "cd src && cat lib.rs",
        "echo $HOME",
        "pwd",
        "printenv PATH",
        "cd /tmp && echo hi | tail -3",
    ];
    for mode in [
        Mode::Ask,
        Mode::Plan,
        Mode::Edit,
        Mode::Configured,
        Mode::Auto,
    ] {
        let (hooks, _) = hooks(
            PermissionRules {
                mode,
                ..PermissionRules::default()
            },
            vec![],
        );
        for command in outside {
            if home.is_empty() && command.contains('~') {
                continue;
            }
            assert_eq!(
                hooks.decide(&bash(command), &ctx()),
                Decision::Ask,
                "{mode:?} {command}"
            );
            // Still look-only: plan mode lets it reach the question.
            assert!(
                is_read_only_call(&bash(command), ShellSyntax::Posix),
                "{command}"
            );
        }
        for command in inside {
            assert_eq!(
                hooks.decide(&bash(command), &ctx()),
                Decision::Allow,
                "{mode:?} {command}"
            );
        }
    }
    let (all, _) = hooks(rules("mode = \"all\""), vec![]);
    assert_eq!(
        all.decide(&bash("cat /etc/passwd"), &ctx()),
        Decision::Allow
    );

    let (hooks, asked) = hooks(rules("mode = \"configured\""), vec![]);
    let mut hooks = hooks.with_persist(Box::new(|_, _, _, _| {}));
    hooks.before_tool_call(&bash("cat /etc/hosts"), &ctx());
    let request = asked.lock().unwrap()[0].clone();
    assert_eq!(request.patterns(), vec!["cat /etc/hosts"]);
}

/// A `$` expands only before a name, a parameter, a brace, a parenthesis
/// or a bracket (and outside double quotes before a quote, `$'…'` being a
/// quote of its own); anywhere else it is a plain character, so a pattern
/// anchored with `$` is no variable whose value cannot be seen.
#[test]
fn a_dollar_that_expands_nothing_is_no_variable() {
    let words = |part: &str| shell_words(part);
    assert_eq!(
        words("rg \"foo$\" src"),
        Some(vec!["rg".into(), "foo$".into(), "src".into()])
    );
    assert_eq!(
        words("grep a$ x"),
        Some(vec!["grep".into(), "a$".into(), "x".into()])
    );
    assert_eq!(
        words("echo $ \"$\""),
        Some(vec!["echo".into(), "$".into(), "$".into()])
    );
    assert_eq!(
        words("sed 's/x$//'"),
        Some(vec!["sed".into(), "s/x$//".into()])
    );
    assert_eq!(
        words("echo \"a$'\""),
        Some(vec!["echo".into(), "a$'".into()])
    );
    for expanding in [
        "cat $HOME/x",
        "cat \"$HOME\"",
        "cat ${HOME}/x",
        "cat $1",
        "cat \"$@\"",
        "cat $_x",
        "cat $$",
        "cat $(ls)",
        "cat \"$(ls)\"",
        "cat $[1]",
        "cat $'\\x2e\\x2e/x'",
        "cat $\"x\"",
    ] {
        assert_eq!(words(expanding), None, "{expanding}");
    }

    let (hooks, _) = hooks(rules("mode = \"edit\""), vec![]);
    for command in [
        "rg \"foo$\" src",
        "rg 'foo$' src",
        "grep \"a$\" src/lib.rs",
        "grep -rn 'end$' src",
        "sed -n \"/x$/p\" src/lib.rs",
    ] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx()),
            Decision::Allow,
            "{command}"
        );
    }
    for command in ["cat $HOME/x", "cat \"${HOME}/x\"", "cat $'\\x2e\\x2e/x'"] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx()),
            Decision::Ask,
            "{command}"
        );
    }
}

/// A `for` loop over words that each name a path inside the project lets
/// its variable stand for them in a look-only read; a variable bound any
/// other way, or one the line could change, still asks.
#[test]
fn loop_variables_over_paths_in_the_project_read_inside_it() {
    let (hooks, _) = hooks(rules("mode = \"edit\""), vec![]);
    let inside = [
        "for f in *.rs src/*.toml; do wc -l $f; done",
        "for f in a b c; do cat $f; done",
        "for f in a b c; do cat ${f}; done",
        "for f in a b c; do cat \"$f\"; done",
        "for f in a b\ndo\n  head -n 3 src/$f.rs\ndone",
        "for f in src/*.rs; do grep -n TODO $f | head -5; done",
        "for d in src docs; do ls $d; done",
        "for p in foo bar; do rg $p src; done",
        "for f in a b; do for g in c d; do cat $f/$g; done; done",
        "for f in a; do wc -l < $f; done",
        "cd src && for f in *.rs; do cat $f; done",
        "for f in /proj/src/a; do cat $f; done",
        "for f in a; do cat $f; done; for f in b; do cat $f; done",
    ];
    for command in inside {
        assert_eq!(
            hooks.decide(&bash(command), &ctx()),
            Decision::Allow,
            "{command}"
        );
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let outside = [
        // A word outside the project, or one that could become one.
        "for f in ~/*; do cat $f; done",
        "for f in /etc/passwd; do cat $f; done",
        "for f in a ../b; do cat $f; done",
        "for f in .*; do cat $f; done",
        "for f in --files-from=x; do wc $f; done",
        "for f in 'a b' c; do cat $f; done",
        // A list that cannot be seen.
        "for f in $(ls ~); do cat $f; done",
        "for f in `ls ~`; do cat $f; done",
        "for f in $LIST; do cat $f; done",
        "for f; do cat $f; done",
        "while read f; do cat $f; done < list",
        "ls | while read f; do cat $f; done",
        // The value goes out, or another variable is read.
        "for f in a; do cat /etc/$f; done",
        "for f in a; do cat ../$f; done",
        "for f in a; do cat $g; done",
        "for f in a; do cat $HOME/$f; done",
        "for f in a; do cat ${f:-/etc/passwd}; done",
        "for f in a; do cat ${f}x; cat $SECRET_FILE; done",
        // Something on the line can give it another value.
        "for f in a; do f=/etc/passwd; cat $f; done",
        "for f in a; do echo ${f:=x}; cat $f; done",
        "for f in a; do read f; cat $f; done",
        "g() { f=/etc/passwd; }; for f in a; do g; cat $f; done",
        "for f in a; do cd /etc; cat $f; done",
        "for f in a; do cat $f; cd ..; done",
        "IFS=/; for f in a/b; do cat $f; done",
        // The loop is over: the variable keeps its last value unseen.
        "for f in a; do echo $f; done; cat $f",
        "cat $HOME/x",
    ];
    for command in outside {
        if home.is_empty() && command.contains('~') {
            continue;
        }
        assert_eq!(
            hooks.decide(&bash(command), &ctx()),
            Decision::Ask,
            "{command}"
        );
    }

    let values = |command: &str| -> Vec<Vec<(String, Vec<ShellWord>)>> {
        shell_parts(command, Path::new("/proj"))
            .into_iter()
            .map(|p| p.loop_values)
            .collect()
    };
    let bound = |name: &str, words: &[&str]| {
        (
            name.to_string(),
            words.iter().map(|w| ShellWord::literal(*w)).collect(),
        )
    };
    assert_eq!(
        values("for f in a 'b c'; do cat $f; done; ls"),
        vec![vec![bound("f", &["a", "b c"])], vec![]]
    );
    // A `while` inside is a loop of its own, closed by its own `done`.
    assert_eq!(
        values("for f in a; do while true; do cat $f; done; wc $f; done"),
        vec![
            vec![bound("f", &["a"])],
            vec![bound("f", &["a"])],
            vec![bound("f", &["a"])]
        ]
    );
    assert_eq!(
        values("for f in a; do while read g; do cat $f; done; done"),
        vec![vec![], vec![]]
    );
}

/// A symlink in the project leading out of it carries a look-only read
/// out too.
#[cfg(unix)]
#[test]
fn a_look_only_read_through_a_symlink_out_of_the_project_asks() {
    let outside = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "x").unwrap();
    std::fs::write(project.path().join("notes"), "x").unwrap();
    std::os::unix::fs::symlink(outside.path(), project.path().join("docs")).unwrap();
    let ctx = ToolContext::new(project.path().to_path_buf());
    let (hooks, _) = hooks(rules("mode = \"edit\""), vec![]);
    for command in ["cat docs/secret", "cd docs && cat secret", "cd docs && ls"] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx),
            Decision::Ask,
            "{command}"
        );
    }
    assert_eq!(hooks.decide(&bash("cat notes"), &ctx), Decision::Allow);
}

/// A glob in a look-only command reads what it matches, as the shell
/// expands it: a symlink out of the project among the matches asks.
#[cfg(unix)]
#[test]
fn a_glob_matching_a_symlink_out_of_the_project_asks() {
    use std::os::unix::fs::symlink;
    let outside = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(outside.path().join("secret"), "x").unwrap();
    std::fs::write(root.join("notes"), "x").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "x").unwrap();
    symlink(outside.path(), root.join("docs")).unwrap();
    symlink(outside.path().join("secret"), root.join("src/lnk")).unwrap();
    // A dotfile matches only a pattern naming its dot.
    std::fs::create_dir_all(root.join("hid")).unwrap();
    std::fs::write(root.join("hid/plain"), "x").unwrap();
    symlink(outside.path().join("secret"), root.join("hid/.out")).unwrap();
    // A match that would be taken for an option.
    std::fs::create_dir_all(root.join("opt")).unwrap();
    std::fs::write(root.join("opt/-n"), "x").unwrap();
    // More matches than are judged one by one.
    std::fs::create_dir_all(root.join("many")).unwrap();
    for i in 0..=MAX_GLOB_MATCHES {
        std::fs::write(root.join(format!("many/{i}")), "").unwrap();
    }
    let ctx = ToolContext::new(root.to_path_buf());
    let (hooks, _) = hooks(rules("mode = \"edit\""), vec![]);
    let absolute = format!("cat {}/d*/secret", root.display());
    let unknown_dir = format!("cd $X && cat {}/src/*", root.display());
    for command in [
        "cat *",
        "cat d*",
        "head */secret",
        "cat ?ocs/secret",
        "cat [d]ocs/secret",
        "cat [^n]ocs/secret",
        "cat **/secret",
        "cat src/*",
        "cat src/l?k",
        "cat \"src\"/*",
        "cd src && cat *",
        "wc -l <src/l*",
        "wc -l < src/l*",
        "for f in *; do cat $f; done",
        "for f in src/*; do cat $f; done",
        "for f in s; do cat ${f}rc/*; done",
        "cat hid/.*",
        "cd opt && cat *",
        "cat many/*",
        "for f in many/*; do cat $f; done",
        &absolute,
        &unknown_dir,
    ] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx),
            Decision::Ask,
            "{command}"
        );
    }
    for command in [
        "cat no*",
        "cat src/*.rs",
        "cat hid/*",
        "cat opt/*",
        "cat '*'",
        "cat \"d*\"",
        "cat d\\*",
        "cat *.zzz",
        "ls n?tes",
        "grep -n x src/[a]*",
        "for f in src/*.rs; do cat $f; done",
        "wc -l <n*",
        "ls many",
        "echo many/*",
        "for f in many/*; do echo $f; done",
    ] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx),
            Decision::Allow,
            "{command}"
        );
    }

    let words = |part: &str| globbing_words(part, &[]).unwrap();
    assert_eq!(
        words("cat 'a*'b* \"[x]\"? c***d [^z]")[1..],
        [
            ShellWord {
                text: "a*b*".into(),
                glob: Some("a[*]b*".into())
            },
            ShellWord {
                text: "[x]?".into(),
                glob: Some("[[]x[]]?".into())
            },
            ShellWord {
                text: "c***d".into(),
                glob: Some("c*d".into())
            },
            ShellWord {
                text: "[^z]".into(),
                glob: Some("[!z]".into())
            },
        ]
    );
    assert_eq!(words("cat \\*")[1], ShellWord::literal("*"));
}

/// A loop that changes directory reads its second turn somewhere else,
/// so its body and what follows it run in an unknown directory.
#[test]
fn a_loop_that_changes_directory_leaves_it_unknown() {
    let (hooks, _) = hooks(rules("mode = \"edit\""), vec![]);
    for command in [
        "for x in a b; do cat src/lib.rs; cd ..; done",
        "for x in a b; do cd ..; done; cat src/lib.rs",
        "while true; do ls; cd ..; done",
        "until false; do pushd ..; done; ls",
        "select x in a b; do cat f; cd sub; done",
        "for x in a; do for y in b; do cat f; done; cd ..; done",
        "pushd src; for x in a b; do popd; done; popd; cat f",
        "while cd ..; do cat f; done",
        "for x in a b; do cd ..; done; cd src && cat f",
    ] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx()),
            Decision::Ask,
            "{command}"
        );
    }
    for command in [
        // No directory change in the loop, or an absolute one after it.
        "for x in a b; do cat src/lib.rs; done; cat f",
        "for x in a b; do cd ..; done; cd /proj && cat f",
        "cd src; for f in a b; do cat $f; done",
    ] {
        assert_eq!(
            hooks.decide(&bash(command), &ctx()),
            Decision::Allow,
            "{command}"
        );
    }
    let dirs: Vec<_> = shell_parts(
        "cd src; for x in a; do ls; cd ..; done; cat f",
        Path::new("/proj"),
    )
    .into_iter()
    .map(|p| p.dir)
    .collect();
    assert_eq!(dirs, vec![Some(PathBuf::from("/proj")), None, None, None]);
}

/// The tree walks that follow symlinks can be led out of the project by
/// one inside it, so they ask wherever an outside read asks.
#[test]
fn a_tree_walk_following_symlinks_asks() {
    let (edit, _) = hooks(rules("mode = \"edit\""), vec![]);
    let (plan, _) = hooks(rules("mode = \"plan\""), vec![]);
    let (all, _) = hooks(rules("mode = \"all\""), vec![]);
    for command in [
        "rg -L foo",
        "rg --follow foo src",
        "rg -iL foo",
        "grep -R foo .",
        "grep -rnR foo src",
        "grep --dereference-recursive foo",
        "find -L . -name x",
        "find -H src",
        "find -HL .",
        "find . -follow -name x",
        "ls -L src",
        "ls -laL",
        "ls --dereference",
        "du -L",
        "du -shL src",
        "du --dereference .",
        "tree -l",
        "tree -al src",
    ] {
        assert_eq!(
            edit.decide(&bash(command), &ctx()),
            Decision::Ask,
            "{command}"
        );
        assert_eq!(
            plan.decide(&bash(command), &ctx()),
            Decision::Ask,
            "{command}"
        );
        assert_eq!(
            all.decide(&bash(command), &ctx()),
            Decision::Allow,
            "{command}"
        );
    }
    for command in [
        "rg foo",
        "grep -rn foo src",
        "find . -name x",
        "find -P .",
        "ls -la src",
        "du -sh",
        "tree -L 2",
    ] {
        assert_eq!(
            edit.decide(&bash(command), &ctx()),
            Decision::Allow,
            "{command}"
        );
    }
}

/// A backquote closes the substitution it opened, so the separators
/// after it split the line again and a removal behind it is seen.
#[test]
fn a_closing_backquote_ends_its_substitution() {
    let parsed = split_shell("echo `true`; rm -rf ~");
    assert_eq!(parsed.parts, vec!["echo `true`", "rm -rf ~"]);
    assert_eq!(parsed.substituted, vec![true, false]);
    assert_eq!(
        split_shell("a `b; c` && d `e && f`").parts,
        vec!["a `b; c`", "d `e && f`"]
    );
    assert_eq!(
        split_shell("a $(b `c; d` e; f) ; g").parts,
        vec!["a $(b `c; d` e; f)", "g"]
    );
    assert_eq!(
        split_shell("a `b $(c; d)` ; e").parts,
        vec!["a `b $(c; d)`", "e"]
    );
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        let parts = shell_parts("echo `true`; rm -rf ~", Path::new("/proj"));
        assert!(parts
            .iter()
            .any(|p| critical_removal(&p.text, Path::new("/proj"))));
    }
}

/// A lone `&` backgrounds a command and starts the next, so each side
/// is judged on its own; the `&` of a redirection splits nothing.
#[test]
fn a_background_ampersand_separates_commands() {
    let parts = |command: &str| split_shell(command).parts;
    assert_eq!(
        parts("ls & touch /tmp/pwned"),
        vec!["ls", "touch /tmp/pwned"]
    );
    assert_eq!(parts("sleep 5 &"), vec!["sleep 5"]);
    assert_eq!(parts("a&b"), vec!["a", "b"]);
    assert_eq!(parts("cargo test 2>&1"), vec!["cargo test 2>&1"]);
    assert_eq!(parts("ls >&2 && pwd"), vec!["ls >&2", "pwd"]);
    assert_eq!(parts("ls &>/dev/null"), vec!["ls &>/dev/null"]);
    assert_eq!(parts("ls &>> log"), vec!["ls &>> log"]);
    assert_eq!(parts("cat <&3"), vec!["cat <&3"]);
    assert_eq!(parts("make |& tee log"), vec!["make", "tee log"]);
    assert_eq!(parts("echo 'a & b' \\& c"), vec!["echo 'a & b' \\& c"]);

    // Each side earns its own verdict, in plan mode and under an allow
    // rule for the first side.
    let (plan, _) = hooks(rules("mode = \"plan\""), vec![]);
    assert_eq!(
        plan.decide(&bash("ls & touch /tmp/pwned"), &ctx()),
        Decision::Deny
    );
    let mut configured = rules("mode = \"configured\"");
    configured.add("shell", "cargo test*", Decision::Allow);
    let (configured, _) = hooks(configured, vec![]);
    assert_eq!(
        configured.decide(&bash("cargo test & rm -rf src"), &ctx()),
        Decision::Ask
    );
}

/// The shell's reserved words are not commands: the part is the command
/// after them, and closing words and loop headers run nothing.
#[test]
fn shell_keywords_are_not_commands() {
    let parts = |command: &str| split_shell(command).parts;
    assert_eq!(
        parts("if [ -f Cargo.toml ]; then cargo build; else echo no; fi"),
        vec!["[ -f Cargo.toml ]", "cargo build", "echo no"]
    );
    assert_eq!(
        parts("for f in *.rs; do\n  wc -l \"$f\"\ndone"),
        vec!["wc -l \"$f\""]
    );
    assert_eq!(
        parts("while ! grep -q ready log; do sleep 1; done"),
        vec!["grep -q ready log", "sleep 1"]
    );
    assert_eq!(parts("{ ls; } 2>&1"), vec!["ls"]);
    assert_eq!(parts("time cargo test"), vec!["cargo test"]);
    // A closing word that writes stays to be judged.
    assert_eq!(
        parts("for f in a; do cat $f; done > out"),
        vec!["cat $f", "done > out"]
    );
    // A substitution in a header still marks the line.
    assert!(split_shell("for f in $(rm -rf x); do echo $f; done").has_substitution);
}

/// A substitution asks about the part that carries it, not about the
/// parts beside it: a session of read commands should not be listed as
/// seven grants because one of them expanded `$(…)`.
#[test]
fn a_substitution_asks_about_its_own_part_only() {
    let mut rules = PermissionRules::default();
    rules.add("shell", "cd *", Decision::Allow);
    rules.add("shell", "ls *", Decision::Allow);
    rules.add("shell", "echo *", Decision::Allow);
    rules.add("shell", "head *", Decision::Allow);
    let (hooks, _) = hooks(rules, vec![]);

    let CommandVerdict {
        decision: verdict,
        asked,
        ..
    } = hooks.judge_command("cd x && ls && echo hi && tail -1 f", &ctx());
    assert_eq!(verdict, Decision::Allow, "no part expands");
    assert!(asked.is_empty());

    let CommandVerdict {
        decision: verdict,
        asked,
        ..
    } = hooks.judge_command("cd x && ls && echo $(date) && head -1 f", &ctx());
    assert_eq!(verdict, Decision::Ask);
    // Only the expanding part is asked about, and it has no rule to keep.
    let texts: Vec<&str> = asked.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["echo $(date)"]);
    assert!(asked[0].pattern.is_none());

    // The parts a rule covers pass even when a neighbour substitutes.
    let verdict = hooks
        .judge_command("ls 2>/dev/null && wc -l f", &ctx())
        .decision;
    assert_eq!(verdict, Decision::Allow);
    let CommandVerdict {
        decision: verdict,
        asked,
        ..
    } = hooks.judge_command("ls $(pwd)", &ctx());
    assert_eq!(verdict, Decision::Ask, "the substituting part asks");
    assert_eq!(asked.len(), 1);
}

/// A loop header that expands is still run, so it marks the command it
/// hands its words to.
#[test]
fn a_substitution_in_a_dropped_header_marks_the_command_it_feeds() {
    let (hooks, _) = hooks(PermissionRules::default(), vec![]);
    let parsed = split_shell("for f in $(rm -rf x); do echo $f; done");
    assert!(parsed.has_substitution);
    // The header is not a command, and the operators inside `$(…)` are
    // not separators either, so `rm -rf x` is no part of its own: it is
    // never offered the pattern `rm *`. What runs is the body, marked by
    // the expansion the dropped header hands it.
    assert_eq!(parsed.parts, ["echo $f"]);
    assert_eq!(parsed.substituted, [true]);
    assert_eq!(
        hooks.decide(&bash("for f in $(ls); do cat $f; done"), &ctx()),
        Decision::Ask
    );
}

/// "Allow always" is on offer only for the one command whose rule states
/// enough to trust: a bundle, a destructive command and a command that
/// runs some other program leave no rule behind.
#[test]
fn always_is_offered_only_for_a_bounded_routine_command() {
    let (hooks, asked) = hooks(
        PermissionRules::default(),
        vec![PermissionAnswer::AllowOnce; 40],
    );
    let mut hooks = hooks.with_persist(Box::new(|_, _, _, _| {}));
    let mut offered = |command: &str| -> bool {
        hooks.before_tool_call(&bash(command), &ctx());
        asked.lock().unwrap().last().unwrap().can_persist
    };

    // Bounded and reversible: on offer.
    for c in [
        "npm test",
        "cargo build --release",
        "cargo nextest run",
        "git commit -m x",
        "git checkout main",
        "touch newfile",
        "mkdir -p out",
        "git stash push -m wip",
    ] {
        assert!(offered(c), "{c} should be persistable");
    }
    // Destructive: no rule, and no session grant either.
    for c in [
        "rm -rf target",
        "sudo apt install x",
        "git push",
        "git push --force",
        "git clean -fdx",
        "git reset --hard",
        "git stash clear",
        "git branch -D old",
        "cargo publish",
        "cargo install ripgrep",
        "cargo add serde",
        "dd if=x of=/dev/disk9",
        "kubectl delete pod x",
        "docker rm x",
        "gh issue close 1",
        "chmod 777 bin",
    ] {
        assert!(!offered(c), "{c} must not be persistable");
    }
    // Running some other program: the rule would vouch for what it runs.
    for c in [
        "env rm x",
        "xargs rm",
        "timeout 5 evil",
        "sh red.sh",
        "./deploy.sh",
        "python3 -c \"import os\"",
        "make install",
        "nix run github:a/b",
    ] {
        assert!(!offered(c), "{c} runs a program the rule cannot name");
    }
    // A bundle answered as one is never a blanket rule.
    assert!(!offered("cargo build && cargo test"));
    assert!(!offered("make && make install"));
}

/// A destructive call allowed "for the session" stands for this call only:
/// nothing is remembered, so the next one asks again.
#[test]
fn a_session_answer_for_a_destructive_command_remembers_nothing() {
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let sink = persisted.clone();
    let (hooks, _) = hooks(
        PermissionRules::default(),
        vec![PermissionAnswer::AllowSession],
    );
    let mut hooks = hooks.with_persist(Box::new(move |tool, pattern, decision, scope| {
        sink.lock()
            .unwrap()
            .push((tool.to_string(), pattern.to_string(), decision, scope));
    }));
    assert_eq!(
        hooks.before_tool_call(&bash("rm -rf target"), &ctx()),
        ToolDecision::Allow
    );
    assert!(persisted.lock().unwrap().is_empty());
    assert_eq!(hooks.rules().evaluate("shell", "rm x"), None);
    assert_eq!(hooks.rules().evaluate_session("shell", "rm x"), None);
    assert_eq!(hooks.decide(&bash("rm -rf target"), &ctx()), Decision::Ask);
}

/// Denying for the session stays on offer for a destructive command: it
/// trusts nothing, so it can be remembered even when allowing cannot.
#[test]
fn a_destructive_command_can_still_be_denied_for_the_session() {
    let (hooks, asked) = hooks(
        PermissionRules::default(),
        vec![PermissionAnswer::AllowOnce, PermissionAnswer::DenySession],
    );
    let mut hooks = hooks.with_persist(Box::new(|_, _, _, _| {}));
    assert_eq!(
        hooks.before_tool_call(&bash("rm -rf target"), &ctx()),
        ToolDecision::Allow
    );
    assert!(!asked.lock().unwrap()[0].can_allow_session);
    assert!(asked.lock().unwrap()[0].can_remember());
    assert_eq!(
        hooks.before_tool_call(&bash("rm -rf target"), &ctx()),
        ToolDecision::Block {
            reason: "denied by the user for this session".into()
        }
    );
    assert_eq!(hooks.decide(&bash("rm -rf target"), &ctx()), Decision::Deny);
}

/// A whole-command `ask` rule over parts that are all look-only asks
/// with no part listed and no rule to keep, instead of panicking.
#[test]
fn a_whole_command_ask_over_look_only_parts_asks_without_parts() {
    let (hooks, asked) = hooks(
        rules(
            r#"
                mode = "configured"
                [shell]
                "*&&*" = "ask"
                "#,
        ),
        vec![PermissionAnswer::AllowAlways],
    );
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let sink = persisted.clone();
    let mut hooks = hooks.with_persist(Box::new(move |tool, pattern, _, _| {
        sink.lock()
            .unwrap()
            .push((tool.to_string(), pattern.to_string()));
    }));
    assert_eq!(
        hooks.before_tool_call(&bash("ls && pwd"), &ctx()),
        ToolDecision::Allow
    );
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].parts.is_empty());
    assert!(!asked[0].can_persist);
    assert!(persisted.lock().unwrap().is_empty());
}

#[test]
fn prompt_answers_drive_grants_and_persistence() {
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let sink = persisted.clone();
    let (hooks, asked) = hooks(
        PermissionRules {
            mode: Mode::Configured,
            ..PermissionRules::default()
        },
        vec![
            PermissionAnswer::AllowOnce,
            PermissionAnswer::AllowSession,
            PermissionAnswer::AllowAlways,
            PermissionAnswer::Deny,
        ],
    );
    let mut hooks = hooks.with_persist(Box::new(move |tool, pattern, decision, scope| {
        sink.lock()
            .unwrap()
            .push((tool.to_string(), pattern.to_string(), decision, scope));
    }));

    // 1. allow once: asked again next time
    assert_eq!(
        hooks.before_tool_call(&bash("cargo build"), &ctx()),
        ToolDecision::Allow
    );
    // 2. allow for the session: "cargo build *" is granted in memory
    assert_eq!(
        hooks.before_tool_call(&bash("cargo build"), &ctx()),
        ToolDecision::Allow
    );
    assert_eq!(
        hooks.decide(&bash("cargo build --release"), &ctx()),
        Decision::Allow
    );
    assert!(persisted.lock().unwrap().is_empty());
    // 3. allow always: persisted through the callback
    assert_eq!(
        hooks.before_tool_call(&bash("npm test"), &ctx()),
        ToolDecision::Allow
    );
    assert_eq!(
        *persisted.lock().unwrap(),
        vec![(
            "shell".to_string(),
            "npm test *".to_string(),
            Decision::Allow,
            PersistScope::Project
        )]
    );
    assert_eq!(
        hooks.rules().evaluate("shell", "npm test -- x"),
        Some(Decision::Allow)
    );
    // 4. deny
    let blocked = hooks.before_tool_call(&call("write", json!({ "path": "a" })), &ctx());
    assert!(matches!(blocked, ToolDecision::Block { reason } if reason.contains("user")));

    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 4);
    assert_eq!(asked[0].suggested_pattern, "cargo build *");
    assert_eq!(asked[3].subject, "a");
    assert_eq!(asked[3].suggested_pattern, "a");
    assert!(asked.iter().all(|request| request.can_persist));
}

#[test]
fn always_is_offered_only_in_configured_mode_and_goes_where_asked() {
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let sink = persisted.clone();
    let (hooks, asked) = hooks(
        PermissionRules::default(),
        vec![
            PermissionAnswer::AllowAlwaysGlobal,
            PermissionAnswer::DenySession,
            PermissionAnswer::AllowAlways,
        ],
    );
    let mut hooks = hooks.with_persist(Box::new(move |_, pattern, _, scope| {
        sink.lock().unwrap().push((pattern.to_string(), scope));
    }));
    hooks.before_tool_call(&bash("npm test"), &ctx());
    let refused = hooks.before_tool_call(&bash("make install"), &ctx());
    assert!(matches!(refused, ToolDecision::Block { reason } if reason.contains("session")));
    // Refused for the session: not asked again.
    assert_eq!(
        hooks.decide(&bash("make install DESTDIR=x"), &ctx()),
        Decision::Deny
    );
    assert_eq!(
        *persisted.lock().unwrap(),
        vec![("npm test *".to_string(), PersistScope::Global)]
    );
    // Outside `configured`, "always" is not on offer, and an answer that
    // says it anyway lasts for the session only.
    hooks.set_mode(Mode::Ask);
    hooks.before_tool_call(&bash("cargo build"), &ctx());
    assert!(!asked.lock().unwrap()[2].can_persist);
    assert_eq!(persisted.lock().unwrap().len(), 1);
    assert_eq!(
        hooks.decide(&bash("cargo build --release"), &ctx()),
        Decision::Allow
    );
    assert_eq!(hooks.rules().evaluate("shell", "cargo build"), None);
}

#[test]
fn rule_denials_block_without_prompting() {
    let mut rules = PermissionRules::default();
    rules.add("read", "**/.env*", Decision::Deny);
    let (mut hooks, asked) = hooks(rules, vec![]);
    let blocked = hooks.before_tool_call(
        &call("read", json!({ "path": "/proj/config/.env" })),
        &ctx(),
    );
    assert!(matches!(blocked, ToolDecision::Block { reason } if reason.contains("rules")));
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn suggested_patterns() {
    assert_eq!(
        suggested_pattern("shell", "git push origin main"),
        "git push *"
    );
    assert_eq!(suggested_pattern("shell", "ls -la"), "ls *");
    assert_eq!(suggested_pattern("shell", "git -C x status"), "git *");
    assert_eq!(
        suggested_pattern("shell", "cargo test && cargo fmt"),
        "cargo test *"
    );
    assert_eq!(suggested_pattern("edit", "src/lib.rs"), "src/lib.rs");
    assert_eq!(suggested_pattern("other", ""), "*");
    assert_eq!(
        suggested_pattern("fetch", "https://docs.rs/ratatui/latest/?x=1"),
        "https://docs.rs/*"
    );
    assert_eq!(
        suggested_pattern("fetch", "http://127.0.0.1:8080"),
        "http://127.0.0.1:8080/*"
    );
    assert_eq!(suggested_pattern("web_search", "rust tui"), "*");
}

#[test]
fn web_tools_are_judged_by_url_and_query() {
    let fetch = call("fetch", json!({ "url": " https://docs.rs/a " }));
    assert_eq!(subject_of(&fetch, &ctx()), "https://docs.rs/a");
    let search = call("web_search", json!({ "query": "rust tui", "limit": 3 }));
    assert_eq!(subject_of(&search, &ctx()), "rust tui");
    assert!(is_read_only_call(&fetch, ShellSyntax::Posix));
    assert!(is_read_only_call(&search, ShellSyntax::Posix));
}

/// A symlink in the project does not carry a file tool past its edge: the
/// subject is the file the system opens, outside the project when the
/// link points out, and the edit asks.
#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_project_is_outside_it() {
    let outside = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::write(project.path().join(".env"), "SECRET=1").unwrap();
    std::os::unix::fs::symlink(outside.path(), project.path().join("docs")).unwrap();
    std::os::unix::fs::symlink(project.path().join(".env"), project.path().join("env.txt"))
        .unwrap();
    let ctx = ToolContext::new(project.path().to_path_buf());
    let real_outside = dunce::canonicalize(outside.path()).unwrap();
    let path = |p: &str| json!({ "path": p, "content": "x" });

    let write = call("write", path("docs/.zshrc"));
    assert_eq!(
        subject_of(&write, &ctx),
        real_outside.join(".zshrc").to_string_lossy()
    );
    // `..` after the link leaves its target, as the system's lookup does.
    let up = call("write", path("docs/../escape"));
    assert_eq!(
        subject_of(&up, &ctx),
        real_outside
            .parent()
            .unwrap()
            .join("escape")
            .to_string_lossy()
    );
    // A file not yet there, inside, stays inside.
    assert_eq!(
        subject_of(&call("write", path("src/new.rs")), &ctx),
        "src/new.rs"
    );
    assert_eq!(
        subject_of(&call("write", path("new/../src/x.rs")), &ctx),
        "src/x.rs"
    );
    // A link to a guarded file is judged as that file.
    assert_eq!(subject_of(&call("read", path("env.txt")), &ctx), ".env");

    let mut rules = rules("mode = \"edit\"");
    rules.add("read", ".env", Decision::Deny);
    let (hooks, _) = hooks(rules, vec![]);
    assert_eq!(hooks.decide(&write, &ctx), Decision::Ask);
    assert_eq!(hooks.decide(&up, &ctx), Decision::Ask);
    assert_eq!(
        hooks.decide(&call("write", path("src/new.rs")), &ctx),
        Decision::Allow
    );
    assert_eq!(
        hooks.decide(&call("read", path("env.txt")), &ctx),
        Decision::Deny
    );
    for mode in [Mode::Auto, Mode::Plan] {
        hooks.set_mode(mode);
        assert_ne!(hooks.decide(&write, &ctx), Decision::Allow, "{mode:?}");
        assert_ne!(
            hooks.decide(&call("read", path("docs/secret")), &ctx),
            Decision::Allow,
            "{mode:?}"
        );
    }
}

/// An edit or a write under `.git` (its config can name a program that a
/// look-only `git status` runs) asks in every mode, whatever the rules
/// allow; a `deny` still refuses, plan mode still refuses, and no answer
/// outlasts the call.
#[test]
fn writing_under_git_asks_in_every_mode_but_all() {
    let edit = |path: &str| call("edit", json!({ "path": path }));
    for mode in [Mode::Ask, Mode::Edit, Mode::Configured, Mode::Auto] {
        let mut rules = PermissionRules {
            mode,
            ..PermissionRules::default()
        };
        rules.add("edit", "**", Decision::Allow);
        rules.add_session("edit", ".git/config", Decision::Allow);
        let (hooks, _) = hooks(rules, vec![]);
        for path in [
            ".git/config",
            ".git/hooks/pre-commit",
            "/proj/.git/config",
            ".GIT/config",
            "sub/.git",
            "/elsewhere/repo/.git/config",
        ] {
            assert_eq!(
                hooks.decide(&edit(path), &ctx()),
                Decision::Ask,
                "{mode:?} {path}"
            );
        }
        assert_eq!(
            hooks.decide(&call("write", json!({ "path": ".git/config" })), &ctx()),
            Decision::Ask
        );
        // Reading git's files, and a file merely named like it, are not
        // held back.
        assert_ne!(
            hooks.decide(&call("read", json!({ "path": ".git/config" })), &ctx()),
            Decision::Deny
        );
        if matches!(mode, Mode::Edit | Mode::Auto) {
            assert_eq!(hooks.decide(&edit(".gitignore"), &ctx()), Decision::Allow);
            assert_eq!(
                hooks.decide(&edit("src/.github/x"), &ctx()),
                Decision::Allow
            );
        }
    }
    let (plan, _) = hooks(rules("mode = \"plan\""), vec![]);
    assert_eq!(plan.decide(&edit(".git/config"), &ctx()), Decision::Deny);
    // `all` never asks, and runs any shell command anyway; its rules
    // still count.
    let mut all = rules("mode = \"all\"");
    all.add("edit", ".git/hooks/**", Decision::Ask);
    let (all, _) = hooks(all, vec![]);
    for path in [".git/config", "/elsewhere/repo/.git/config", "sub/.git"] {
        assert_eq!(all.decide(&edit(path), &ctx()), Decision::Allow, "{path}");
    }
    assert_eq!(
        all.decide(&edit(".git/hooks/pre-commit"), &ctx()),
        Decision::Ask
    );
    let mut denied = rules("mode = \"all\"");
    denied.add("edit", ".git/**", Decision::Deny);
    let (denied, _) = hooks(denied, vec![]);
    assert_eq!(denied.decide(&edit(".git/config"), &ctx()), Decision::Deny);

    // The reviewer is not asked, and no answer outlasts the call.
    let (hooks, judged, asked) = auto_hooks(
        "",
        vec![allow("fine")],
        vec![PermissionAnswer::AllowSession],
    );
    let mut hooks = hooks.with_persist(Box::new(|_, _, _, _| {}));
    assert_eq!(
        hooks.before_tool_call(&edit(".git/config"), &ctx()),
        ToolDecision::Allow
    );
    assert!(judged.lock().unwrap().is_empty());
    let request = asked.lock().unwrap()[0].clone();
    assert!(!request.can_persist && !request.can_allow_session);
    assert_eq!(hooks.decide(&edit(".git/config"), &ctx()), Decision::Ask);
}

#[test]
fn subjects_are_project_relative() {
    assert_eq!(
        subject_of(
            &call("edit", json!({ "path": "/proj/src/../a.rs" })),
            &ctx()
        ),
        "a.rs"
    );
    assert_eq!(
        subject_of(&call("edit", json!({ "path": "./b.rs" })), &ctx()),
        "b.rs"
    );
    assert_eq!(
        subject_of(&call("edit", json!({ "path": "/etc/x" })), &ctx()),
        "/etc/x"
    );
    assert_eq!(
        subject_of(&call("read", json!({ "path": "/proj" })), &ctx()),
        "."
    );
    assert_eq!(
        subject_of(&call("mcp_tool", json!({ "query": "q" })), &ctx()),
        "q"
    );
}
#[test]
fn mode_handle_switches_the_live_mode() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut hooks = PermissionHooks::new(
        PermissionRules {
            mode: Mode::Configured,
            ..PermissionRules::default()
        },
        Box::new(Scripted {
            answers: vec![],
            asked: asked.clone(),
        }),
    );
    let handle = hooks.mode_handle();
    assert_eq!(handle.get(), Mode::Configured);
    assert_eq!(
        hooks.decide(&call("write", json!({ "path": "a" })), &ctx()),
        Decision::Ask
    );

    handle.set(Mode::Edit);
    assert_eq!(hooks.mode(), Mode::Edit);
    assert_eq!(
        hooks.decide(&call("write", json!({ "path": "a" })), &ctx()),
        Decision::Allow
    );
    assert_eq!(hooks.decide(&bash("cargo build"), &ctx()), Decision::Ask);

    handle.set(Mode::All);
    assert_eq!(hooks.decide(&bash("cargo build"), &ctx()), Decision::Allow);
    assert_eq!(
        hooks.before_tool_call(&bash("cargo build"), &ctx()),
        ToolDecision::Allow
    );
    assert!(asked.lock().unwrap().is_empty());

    assert_eq!(Mode::Ask.next(), Mode::Plan);
    assert_eq!(Mode::Configured.next(), Mode::Auto);
    assert_eq!(Mode::Auto.next(), Mode::All);
    assert_eq!(Mode::All.next(), Mode::Ask);
    assert_eq!(Mode::Edit.label(), "edit");
    assert_eq!(Mode::Configured.label(), "configured");
}
#[test]
fn loading_a_skill_never_asks() {
    let hooks = PermissionHooks::new(
        PermissionRules::default(),
        Box::new(Scripted {
            answers: vec![],
            asked: Arc::new(Mutex::new(Vec::new())),
        }),
    );
    assert_eq!(
        hooks.decide(&call("skill", json!({ "name": "deploy" })), &ctx()),
        Decision::Allow
    );
}
#[test]
fn a_question_to_the_user_never_asks_and_passes_plan_mode() {
    let rules = PermissionRules {
        mode: Mode::Plan,
        ..PermissionRules::default()
    };
    let hooks = PermissionHooks::new(
        rules,
        Box::new(Scripted {
            answers: vec![],
            asked: Arc::new(Mutex::new(Vec::new())),
        }),
    );
    let question = call(
        "question",
        json!({ "questions": [{ "question": "Which?" }] }),
    );
    assert_eq!(hooks.decide(&question, &ctx()), Decision::Allow);
    assert!(is_read_only_call(&question, ShellSyntax::Posix));
}
#[test]
fn recall_never_asks_and_passes_plan_mode() {
    let recall = call("recall", json!({ "queries": ["why no tokio"] }));
    assert!(is_read_only_call(&recall, ShellSyntax::Posix));
    for mode in Mode::ALL {
        let hooks = PermissionHooks::new(
            PermissionRules {
                mode,
                ..PermissionRules::default()
            },
            Box::new(Scripted {
                answers: vec![],
                asked: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        assert_eq!(hooks.decide(&recall, &ctx()), Decision::Allow, "{mode:?}");
    }
    let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
    assert!(matches!(
        guard.before_tool_call(&recall, &ctx()),
        ToolDecision::Allow
    ));
}
/// A delegation passes plan mode's guard and needs no answer: the
/// subagent is held to plan mode in turn.
#[test]
fn plan_mode_lets_a_task_through() {
    let task = call("task", json!({ "agent": "explore", "prompt": "find it" }));
    let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
    assert!(matches!(
        guard.before_tool_call(&task, &ctx()),
        ToolDecision::Allow
    ));
    let decide = |mode| {
        PermissionHooks::new(
            PermissionRules {
                mode,
                ..PermissionRules::default()
            },
            Box::new(Scripted {
                answers: vec![],
                asked: Arc::new(Mutex::new(Vec::new())),
            }),
        )
        .decide(&task, &ctx())
    };
    assert_eq!(decide(Mode::Plan), Decision::Allow);
    assert_eq!(decide(Mode::Ask), Decision::Ask);
}

/// The plan guard and the mode read one list of the calls that ask no
/// one, so neither lets through or refuses what the other does not.
#[test]
fn the_calls_that_ask_no_one_pass_every_mode_and_the_plan_guard() {
    let calls = [
        call("skill", json!({ "name": "deploy" })),
        call("question", json!({ "questions": [] })),
        call("recall", json!({ "queries": ["why"] })),
        call("agents", json!({})),
        call("jobs", json!({})),
        call("jobs", json!({ "stop": "t1" })),
        call("suggest_command", json!({ "command": "make" })),
        call("terminals", json!({})),
        call("terminals", json!({ "action": "read", "session": "dev" })),
        call("terminals", json!({ "action": "screen", "session": "dev" })),
        call(
            "terminals",
            json!({ "action": "interrupt", "session": "dev" }),
        ),
        call("terminals", json!({ "action": "close", "session": "dev" })),
        call("debug", json!({ "action": "state" })),
    ];
    for call in &calls {
        for mode in Mode::ALL {
            let (hooks, _) = hooks(
                PermissionRules {
                    mode,
                    ..PermissionRules::default()
                },
                vec![],
            );
            assert_eq!(
                hooks.decide(call, &ctx()),
                Decision::Allow,
                "{call:?} {mode:?}"
            );
        }
        let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
        assert!(
            matches!(guard.before_tool_call(call, &ctx()), ToolDecision::Allow),
            "{call:?}"
        );
    }
}

fn typed_ctx() -> ToolContext {
    ToolContext {
        tool: crate::ToolTraits {
            typed_at_prompt: true,
            ..Default::default()
        },
        ..ctx()
    }
}

fn send(input: &str) -> ToolCall {
    call(
        "terminals",
        json!({ "action": "send", "session": "dev", "input": input, "keys": ["Enter"] }),
    )
}

/// Keys typed into a program the shell runs are read by no check: each
/// send asks, shows all it types, and no answer outlasts it — an answer
/// for the session once let every later send through, whatever it typed.
#[test]
fn typing_into_a_running_program_asks_every_time() {
    let (mut hooks, asked) = hooks(
        PermissionRules {
            mode: Mode::Configured,
            ..PermissionRules::default()
        },
        vec![PermissionAnswer::AllowSession, PermissionAnswer::Deny],
    );
    assert!(matches!(
        hooks.before_tool_call(&send("1+1"), &ctx()),
        ToolDecision::Allow
    ));
    assert!(matches!(
        hooks.before_tool_call(&send("import os; os.remove('x')"), &ctx()),
        ToolDecision::Block { .. }
    ));
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].tool, "terminals");
    assert_eq!(asked[0].subject, "send dev: 1+1 [Enter]");
    assert!(!asked[0].can_allow_session && !asked[0].can_persist);
}

/// A line typed into a shell at its prompt runs a command, and is
/// judged as the `shell` command it is: look-only commands pass, the
/// `shell` rules count, and an answer is recorded as a `shell` rule.
#[test]
fn a_line_typed_at_the_prompt_is_judged_as_a_shell_command() {
    let (mut hooks, asked) = hooks(
        rules(
            r#"
                mode = "configured"
                [shell]
                "cargo test*" = "allow"
                "#,
        ),
        vec![PermissionAnswer::AllowSession],
    );
    assert_eq!(
        hooks.decide(&send("git status"), &typed_ctx()),
        Decision::Allow
    );
    assert_eq!(
        hooks.decide(&send("cargo test -p x"), &typed_ctx()),
        Decision::Allow
    );
    assert_eq!(
        hooks.decide(&send("mkdir out"), &typed_ctx()),
        Decision::Ask
    );
    assert!(matches!(
        hooks.before_tool_call(&send("mkdir out"), &typed_ctx()),
        ToolDecision::Allow
    ));
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked[0].tool, "shell");
        assert_eq!(asked[0].subject, "mkdir out");
    }
    // The answer covers the same command run by `shell`.
    assert_eq!(hooks.decide(&bash("mkdir out"), &ctx()), Decision::Allow);

    let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
    assert!(matches!(
        guard.before_tool_call(&send("ls"), &typed_ctx()),
        ToolDecision::Allow
    ));
    assert!(matches!(
        guard.before_tool_call(&send("touch x"), &typed_ctx()),
        ToolDecision::Block { .. }
    ));
    assert!(matches!(
        guard.before_tool_call(&send("ls"), &ctx()),
        ToolDecision::Block { .. }
    ));
}

fn mcp_ctx(read_only: bool) -> ToolContext {
    ToolContext {
        tool: crate::ToolTraits {
            mcp: true,
            read_only,
            ..Default::default()
        },
        ..ctx()
    }
}

/// Plan mode leaves an MCP tool to the user: the guard lets it by, a
/// tool its server marks read-only passes, any other asks — with "allow
/// always" on offer — unless a configured rule says otherwise.
#[test]
fn plan_mode_asks_about_an_mcp_tool() {
    let ticket = call("kanban__get_ticket", json!({ "id": "42" }));
    let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
    assert!(matches!(
        guard.before_tool_call(&ticket, &mcp_ctx(false)),
        ToolDecision::Allow
    ));
    // A tool that is not MCP's is still refused by the guard.
    assert!(matches!(
        guard.before_tool_call(&ticket, &ctx()),
        ToolDecision::Block { .. }
    ));

    let plan = || PermissionRules {
        mode: Mode::Plan,
        ..PermissionRules::default()
    };
    let (hooks_, _) = hooks(plan(), vec![]);
    assert_eq!(hooks_.decide(&ticket, &mcp_ctx(true)), Decision::Allow);
    assert_eq!(hooks_.decide(&ticket, &mcp_ctx(false)), Decision::Ask);
    assert_eq!(hooks_.decide(&ticket, &ctx()), Decision::Deny);

    // `edit` trusts the mark as plan mode does, and asks without it;
    // `ask` asks either way, and its configured `allow`s stay set aside.
    let in_mode = |mode| {
        hooks(
            PermissionRules {
                mode,
                ..rules("[kanban__get_ticket]\n\"*\" = \"allow\"")
            },
            vec![],
        )
        .0
    };
    assert_eq!(
        in_mode(Mode::Edit).decide(&ticket, &mcp_ctx(true)),
        Decision::Allow
    );
    assert_eq!(
        in_mode(Mode::Edit).decide(&ticket, &mcp_ctx(false)),
        Decision::Ask
    );
    assert_eq!(
        in_mode(Mode::Ask).decide(&ticket, &mcp_ctx(true)),
        Decision::Ask
    );

    // The configured rules count in full for an MCP tool in plan mode,
    // the `allow`s too; a `deny` beats the server's read-only claim.
    let configured = |text: &str| PermissionRules {
        mode: Mode::Plan,
        ..rules(text)
    };
    let (hooks_, _) = hooks(
        configured("[kanban__get_ticket]\n\"*\" = \"allow\""),
        vec![],
    );
    assert_eq!(hooks_.decide(&ticket, &mcp_ctx(false)), Decision::Allow);
    let (hooks_, _) = hooks(configured("[kanban__get_ticket]\n\"*\" = \"deny\""), vec![]);
    assert_eq!(hooks_.decide(&ticket, &mcp_ctx(true)), Decision::Deny);

    // Asked in plan mode, the user may answer for good.
    let (hooks_, asked) = hooks(plan(), vec![PermissionAnswer::AllowOnce]);
    let mut hooks_ = hooks_.with_persist(Box::new(|_, _, _, _| {}));
    assert!(matches!(
        hooks_.before_tool_call(&ticket, &mcp_ctx(false)),
        ToolDecision::Allow
    ));
    assert!(asked.lock().unwrap()[0].can_persist);
}

#[test]
fn plan_mode_refuses_every_change_and_lets_reads_through() {
    let mode = ModeHandle::new(Mode::Plan);
    let mut guard = PlanGuard::new(mode.clone());
    let blocked = |guard: &mut PlanGuard, call: &ToolCall| {
        matches!(
            guard.before_tool_call(call, &ctx()),
            ToolDecision::Block { reason } if reason.starts_with(&Refusals::default().plan_mode)
        )
    };
    assert!(!blocked(
        &mut guard,
        &call("read", json!({ "path": "src/main.rs" }))
    ));
    assert!(!blocked(
        &mut guard,
        &call("skill", json!({ "name": "deploy" }))
    ));
    assert!(!blocked(
        &mut guard,
        &call("shell", json!({ "command": "git status && rg TODO src" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("shell", json!({ "command": "cat $(ls)" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("shell", json!({ "command": "ls > out" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("shell", json!({ "command": "git status; cargo build" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("shell", json!({ "command": "" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("edit", json!({ "path": "src/main.rs" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("write", json!({ "path": "new.rs" }))
    ));
    assert!(blocked(
        &mut guard,
        &call("fs__search", json!({ "query": "x" }))
    ));
    // A suggestion runs nothing itself: its card comes up without
    // `[Run]`. A message to another agent, not in plan mode, is refused.
    assert!(!blocked(
        &mut guard,
        &call("suggest_command", json!({ "command": "cargo build" }))
    ));
    assert!(blocked(
        &mut guard,
        &call(
            "send_message",
            json!({ "to": 2, "kind": "info", "text": "edit it" })
        )
    ));
    // Reading the debug session is look-only; running the program is not.
    assert!(!blocked(
        &mut guard,
        &call("debug", json!({ "action": "state" }))
    ));
    assert!(blocked(
        &mut guard,
        &call(
            "debug",
            json!({ "action": "start", "command": "debug-app" })
        )
    ));

    // Any other mode: the guard steps aside, the rules decide.
    mode.set(Mode::All);
    assert!(!blocked(
        &mut guard,
        &call("edit", json!({ "path": "src/main.rs" }))
    ));
}
/// Replays verdicts and records the calls it was asked about.
struct ScriptedReviewer {
    verdicts: Vec<Verdict>,
    judged: Arc<Mutex<Vec<String>>>,
}

impl Classifier for ScriptedReviewer {
    fn classify(&mut self, call: &ToolCall, ctx: &ToolContext) -> Verdict {
        self.judged.lock().unwrap().push(subject_of(call, ctx));
        if self.verdicts.is_empty() {
            Verdict::Unavailable {
                reason: "no script".into(),
            }
        } else {
            self.verdicts.remove(0)
        }
    }
}

/// Hooks in `auto` mode, the subjects the reviewer judged, and the
/// requests the user was asked.
type AutoHooks = (
    PermissionHooks,
    Arc<Mutex<Vec<String>>>,
    Arc<Mutex<Vec<PermissionRequest>>>,
);

fn auto_hooks(
    rules_text: &str,
    verdicts: Vec<Verdict>,
    answers: Vec<PermissionAnswer>,
) -> AutoHooks {
    let mut rules = rules(rules_text);
    rules.mode = Mode::Auto;
    let (hooks, asked) = hooks(rules, answers);
    let judged = Arc::new(Mutex::new(Vec::new()));
    let hooks = hooks.with_classifier(Box::new(ScriptedReviewer {
        verdicts,
        judged: judged.clone(),
    }));
    (hooks, judged, asked)
}

fn allow(reason: &str) -> Verdict {
    Verdict::Allow {
        reason: reason.into(),
    }
}

fn block(reason: &str) -> Verdict {
    Verdict::Block {
        reason: reason.into(),
    }
}

#[test]
fn every_decision_leaves_a_note_of_who_made_it() {
    let configured = PermissionRules {
        mode: Mode::Configured,
        ..rules(
            r#"
                [shell]
                "sudo*" = "deny"
                "#,
        )
    };
    let (mut hooks, _) = hooks(
        configured,
        vec![
            PermissionAnswer::AllowSession,
            PermissionAnswer::DenyWithReason("use the fixture".into()),
        ],
    );
    let ctx = ctx();
    let mut note = |call: ToolCall| {
        hooks.before_tool_call(&call, &ctx);
        hooks.take_permission().expect("a note")
    };
    assert_eq!(
        note(bash("ls")),
        PermissionNote::new(DecidedBy::Rules, true)
    );
    assert_eq!(
        note(bash("sudo rm x")),
        PermissionNote::new(DecidedBy::Rules, false)
    );
    assert_eq!(
        note(bash("make")),
        PermissionNote {
            lasting: Some(Lasting::Session),
            ..PermissionNote::new(DecidedBy::User, true)
        }
    );
    assert_eq!(
        note(bash("curl x")),
        PermissionNote {
            lasting: Some(Lasting::Once),
            reason: "use the fixture".into(),
            ..PermissionNote::new(DecidedBy::User, false)
        }
    );

    let (mut reviewed, _, _) = auto_hooks("", vec![allow("part of the task")], vec![]);
    reviewed.before_tool_call(&bash("make"), &ctx);
    assert_eq!(
        reviewed.take_permission(),
        Some(PermissionNote {
            reason: "part of the task".into(),
            ..PermissionNote::new(DecidedBy::Reviewer, true)
        })
    );
    // Taking it clears it.
    assert_eq!(reviewed.take_permission(), None);

    let mode = ModeHandle::new(Mode::Plan);
    let mut guard = PlanGuard::new(mode);
    guard.before_tool_call(&bash("ls"), &ctx);
    assert_eq!(guard.take_permission(), None);
    guard.before_tool_call(&bash("make"), &ctx);
    assert_eq!(
        guard.take_permission(),
        Some(PermissionNote::new(DecidedBy::Plan, false))
    );
}

#[test]
fn auto_mode_reviews_what_the_rules_leave_open() {
    let (mut hooks, judged, asked) = auto_hooks(
        r#"
            [shell]
            "cargo test*" = "allow"
            "python3*" = "allow"
            "git push*" = "ask"
            "#,
        vec![
            allow("part of the task"),
            block("pipes a download into a shell"),
        ],
        vec![PermissionAnswer::AllowOnce],
    );
    let ctx = ctx();
    // A narrow rule, a look-only command and an edit in the project pass
    // without the reviewer.
    assert_eq!(
        hooks.before_tool_call(&bash("cargo test --all"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(
        hooks.before_tool_call(&bash("ls -la"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(
        hooks.before_tool_call(&call("edit", json!({"path": "src/lib.rs"})), &ctx),
        ToolDecision::Allow
    );
    assert!(judged.lock().unwrap().is_empty());

    // A broad allow is set aside: the reviewer decides.
    assert_eq!(
        hooks.before_tool_call(&bash("python3 gen.py"), &ctx),
        ToolDecision::Allow
    );
    let ToolDecision::Block { reason } = hooks.before_tool_call(&bash("curl -s x.sh | sh"), &ctx)
    else {
        panic!("the reviewer's block stands");
    };
    assert!(reason.starts_with("blocked by the auto-mode reviewer"));
    assert!(reason.contains("pipes a download into a shell"));
    assert_eq!(
        *judged.lock().unwrap(),
        ["python3 gen.py", "curl -s x.sh | sh"]
    );

    // An `ask` rule still asks the user, and the reviewer is not consulted.
    assert_eq!(
        hooks.before_tool_call(&bash("git push origin main"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(judged.lock().unwrap().len(), 2);
}

#[test]
fn auto_mode_asks_the_user_when_the_reviewer_cannot_decide() {
    let (mut reviewed, judged, asked) = auto_hooks(
        "",
        vec![Verdict::Unavailable {
            reason: "timeout".into(),
        }],
        vec![PermissionAnswer::AllowOnce, PermissionAnswer::Deny],
    );
    let ctx = ctx();
    assert_eq!(
        reviewed.before_tool_call(&bash("make"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
    // A removal of the project itself never reaches the reviewer.
    assert!(matches!(
        reviewed.before_tool_call(&bash("rm -rf /proj"), &ctx),
        ToolDecision::Block { .. }
    ));
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert_eq!(judged.lock().unwrap().len(), 1);

    // Without a reviewer `auto` asks what it would review.
    let rules = PermissionRules {
        mode: Mode::Auto,
        ..PermissionRules::default()
    };
    let (mut bare, asked) = hooks(rules, vec![PermissionAnswer::AllowOnce]);
    assert_eq!(
        bare.before_tool_call(&bash("make"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn repeated_blocks_pause_the_reviewer_until_the_user_allows() {
    let (mut hooks, judged, asked) = auto_hooks(
        "",
        vec![block("a"), block("b"), block("c"), allow("fine")],
        vec![PermissionAnswer::AllowOnce],
    );
    let ctx = ctx();
    for _ in 0..BLOCKS_IN_A_ROW {
        assert!(matches!(
            hooks.before_tool_call(&bash("make deploy"), &ctx),
            ToolDecision::Block { .. }
        ));
    }
    // Paused: the user is asked, and allowing hands back to the reviewer.
    assert_eq!(
        hooks.before_tool_call(&bash("make deploy"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(
        hooks.before_tool_call(&bash("make build"), &ctx),
        ToolDecision::Allow
    );
    assert_eq!(judged.lock().unwrap().len(), 4);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn broad_allows_are_those_that_run_any_program() {
    for (tool, pattern) in [
        ("shell", "*"),
        ("shell", "python3*"),
        ("shell", "python *"),
        ("shell", "npm run *"),
        ("shell", "env *"),
        ("shell", "sh -c*"),
        ("shell", "make*"),
        ("task", "reviewer"),
        ("mcp__db__query", "*"),
    ] {
        assert!(broad_allow(tool, pattern), "{tool} {pattern}");
    }
    for (tool, pattern) in [
        ("shell", "cargo test*"),
        ("shell", "git status*"),
        ("shell", "npm run build"),
        ("shell", "python3 --version"),
        ("edit", "src/**"),
    ] {
        assert!(!broad_allow(tool, pattern), "{tool} {pattern}");
    }
}

#[test]
fn critical_removals_are_the_root_the_home_and_the_project() {
    let cwd = Path::new("/proj/sub");
    let home = std::env::var("HOME").unwrap_or_default();
    for command in [
        "rm -rf /",
        "rm -rf /*",
        "rm -rf ~",
        "rm -rf ~/",
        "rm -rf $HOME",
        "rm -rf .",
        "rm -rf ./*",
        "rm -rf *",
        "rm -r ..",
        "/bin/rm -rf /proj",
        "rmdir /proj/sub",
    ] {
        assert!(critical_removal(command, cwd), "{command}");
    }
    assert!(critical_removal(&format!("rm -rf {home}"), cwd) || home.is_empty());
    for command in [
        "rm -rf target",
        "rm -rf ./build/*",
        "rm -f /tmp/x",
        "rm -rf ~/.cache/termide",
        "ls /",
    ] {
        assert!(!critical_removal(command, cwd), "{command}");
    }
}

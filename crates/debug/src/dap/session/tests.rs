//! A session driven by a scripted adapter: what it sends is read back from
//! the writer, what the adapter answers is put in its inbox.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::*;
use crate::dap::adapter::Console;
use crate::dap::transport::read_message;

#[derive(Clone, Default)]
struct Wire(Arc<Mutex<Vec<u8>>>);

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Harness {
    session: Session,
    wire: Wire,
    read: usize,
    tx: Sender<Incoming>,
    adapter_seq: i64,
}

impl Harness {
    fn new(breakpoints: Vec<Breakpoint>) -> Self {
        let (tx, rx) = mpsc::channel();
        let spec = LaunchSpec {
            adapter: Adapter::LldbDap,
            debugger_settings: Default::default(),
            adapter_args: Vec::new(),
            program: "target/debug/app".into(),
            args: vec!["--flag".into()],
            cwd: "/p".into(),
            build: None,
            env: Vec::new(),
            label: "app".into(),
            console: Console::Panel,
            tty: None,
        };
        let mut session = Session::new(spec, breakpoints, rx, tx.clone());
        let wire = Wire::default();
        session.connect(Box::new(wire.clone()));
        Self {
            session,
            wire,
            read: 0,
            tx,
            adapter_seq: 1000,
        }
    }

    /// The requests sent since the last call.
    fn sent(&mut self) -> Vec<Value> {
        let bytes = self.wire.0.lock().unwrap().clone();
        let mut reader = io::BufReader::new(&bytes[self.read..]);
        let mut out = Vec::new();
        while let Some(message) = read_message(&mut reader).unwrap() {
            out.push(message);
        }
        self.read = bytes.len();
        out
    }

    fn commands(&mut self) -> Vec<String> {
        self.sent()
            .iter()
            .map(|m| m["command"].as_str().unwrap().to_string())
            .collect()
    }

    fn last_seq_of(&self, command: &str) -> i64 {
        let bytes = self.wire.0.lock().unwrap().clone();
        let mut reader = io::BufReader::new(bytes.as_slice());
        let mut seq = None;
        while let Some(m) = read_message(&mut reader).unwrap() {
            if m["command"] == command && m["type"] == "request" {
                seq = m["seq"].as_i64();
            }
        }
        seq.unwrap_or_else(|| panic!("no {command} was sent"))
    }

    fn respond(&mut self, command: &str, body: Value) {
        let request_seq = self.last_seq_of(command);
        self.adapter_seq += 1;
        let message = json!({
            "seq": self.adapter_seq,
            "type": "response",
            "request_seq": request_seq,
            "command": command,
            "success": true,
            "body": body,
        });
        self.tx.send(Incoming::Message(message)).unwrap();
        self.session.poll();
    }

    fn event(&mut self, event: &str, body: Value) {
        self.adapter_seq += 1;
        let message = json!({
            "seq": self.adapter_seq,
            "type": "event",
            "event": event,
            "body": body,
        });
        self.tx.send(Incoming::Message(message)).unwrap();
        self.session.poll();
    }

    /// Run the session up to a stop at `main.rs:4` with locals fetched.
    fn stopped_at_breakpoint() -> Self {
        let mut h = Harness::new(vec![bp("/p/src/main.rs", 4)]);
        h.respond(
            "initialize",
            json!({ "supportsConfigurationDoneRequest": true }),
        );
        h.event("initialized", json!({}));
        h.respond("launch", json!({}));
        h.respond(
            "threads",
            json!({ "threads": [{ "id": 1, "name": "main" }] }),
        );
        h.event("stopped", json!({ "reason": "breakpoint", "threadId": 1 }));
        h.respond(
            "stackTrace",
            json!({ "stackFrames": [
                { "id": 10, "name": "app::main", "line": 4, "column": 5,
                  "source": { "path": "/p/src/main.rs" } },
                { "id": 11, "name": "std::rt::lang_start", "line": 1, "column": 1 }
            ]}),
        );
        h.respond(
            "scopes",
            json!({ "scopes": [
                { "name": "Locals", "variablesReference": 5, "expensive": false },
                { "name": "Registers", "variablesReference": 6, "expensive": true }
            ]}),
        );
        h.respond(
            "variables",
            json!({ "variables": [
                { "name": "a", "value": "41", "type": "i32", "variablesReference": 0 },
                { "name": "v", "value": "size=2", "type": "Vec<i32>", "variablesReference": 7 }
            ]}),
        );
        h.sent();
        h
    }
}

fn bp(path: &str, line: usize) -> Breakpoint {
    Breakpoint {
        path: path.into(),
        line,
    }
}

#[test]
fn a_launch_sets_the_breakpoints_and_stops_with_frames_and_locals() {
    let mut h = Harness::new(vec![bp("/p/src/main.rs", 4), bp("/p/tool.py", 1)]);
    let init = h.sent();
    assert_eq!(init[0]["command"], "initialize");
    assert_eq!(init[0]["arguments"]["linesStartAt1"], true);

    h.respond(
        "initialize",
        json!({ "supportsConfigurationDoneRequest": true }),
    );
    let launch = h.sent();
    assert_eq!(launch[0]["command"], "launch");
    assert_eq!(launch[0]["arguments"]["program"], "target/debug/app");
    assert_eq!(launch[0]["arguments"]["args"], json!(["--flag"]));

    h.event("initialized", json!({}));
    let config = h.sent();
    let commands: Vec<&str> = config
        .iter()
        .map(|m| m["command"].as_str().unwrap())
        .collect();
    // The Python file is not lldb's.
    assert_eq!(commands, ["setBreakpoints", "configurationDone", "threads"]);
    assert_eq!(config[0]["arguments"]["source"]["path"], "/p/src/main.rs");
    assert_eq!(
        config[0]["arguments"]["breakpoints"],
        json!([{ "line": 4 }])
    );

    h.respond("launch", json!({}));
    assert_eq!(h.session.status, Status::Running);

    let h = Harness::stopped_at_breakpoint();
    assert!(matches!(&h.session.status, Status::Stopped { reason, .. } if reason == "breakpoint"));
    assert_eq!(h.session.frames.len(), 2);
    assert_eq!(
        h.session.execution_point(),
        Some((PathBuf::from("/p/src/main.rs"), 4))
    );
    let locals = &h.session.scopes[0];
    assert!(locals.expanded, "the first cheap scope opens by itself");
    assert!(
        !h.session.scopes[1].expanded,
        "an expensive one stays closed"
    );
    let names: Vec<&str> = locals
        .children
        .as_ref()
        .unwrap()
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, ["a", "v"]);
}

#[test]
fn a_variable_opens_on_demand_and_closes_again() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session.toggle_variable(&[0, 1]);
    let sent = h.sent();
    assert_eq!(sent[0]["command"], "variables");
    assert_eq!(sent[0]["arguments"]["variablesReference"], 7);
    h.respond(
        "variables",
        json!({ "variables": [{ "name": "[0]", "value": "1", "variablesReference": 0 }] }),
    );
    let v = &h.session.scopes[0].children.as_ref().unwrap()[1];
    assert!(v.expanded);
    assert_eq!(v.children.as_ref().unwrap()[0].name, "[0]");

    // Closing and reopening needs no new request.
    h.session.toggle_variable(&[0, 1]);
    h.session.toggle_variable(&[0, 1]);
    assert!(h.sent().is_empty());
}

#[test]
fn a_step_resumes_and_a_late_answer_is_dropped() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session.step_over();
    let sent = h.sent();
    assert_eq!(sent[0]["command"], "next");
    assert_eq!(sent[0]["arguments"]["threadId"], 1);
    assert_eq!(h.session.status, Status::Running);
    assert_eq!(h.session.execution_point(), None);

    // The stack of the stop before the step arrives after it: ignored.
    h.respond(
        "stackTrace",
        json!({ "stackFrames": [{ "id": 99, "name": "stale", "line": 1 }] }),
    );
    assert!(h.session.frames.is_empty());
}

#[test]
fn another_frame_shows_its_own_variables() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session.select_frame(1);
    let sent = h.sent();
    assert_eq!(sent[0]["command"], "scopes");
    assert_eq!(sent[0]["arguments"]["frameId"], 11);
    // That frame has no source, so there is no line to show.
    assert_eq!(h.session.execution_point(), None);
}

#[test]
fn changed_breakpoints_go_to_the_adapter_file_by_file() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session
        .update_breakpoints(&[bp("/p/src/main.rs", 4), bp("/p/src/lib.rs", 9)]);
    let sent = h.sent();
    assert_eq!(sent.len(), 1, "main.rs did not change");
    assert_eq!(sent[0]["arguments"]["source"]["path"], "/p/src/lib.rs");

    h.session.update_breakpoints(&[bp("/p/src/lib.rs", 9)]);
    let sent = h.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["arguments"]["source"]["path"], "/p/src/main.rs");
    assert_eq!(sent[0]["arguments"]["breakpoints"], json!([]));
}

#[test]
fn output_is_split_into_lines_across_events() {
    let mut h = Harness::stopped_at_breakpoint();
    h.event("output", json!({ "category": "stdout", "output": "hel" }));
    h.event(
        "output",
        json!({ "category": "stdout", "output": "lo\nworld\n" }),
    );
    h.event(
        "output",
        json!({ "category": "telemetry", "output": "x\n" }),
    );
    let lines: Vec<&str> = h
        .session
        .output
        .iter()
        .filter(|l| l.kind == OutputKind::Stdout)
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(lines, ["hello", "world"]);
}

#[test]
fn an_evaluation_goes_to_the_output() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session.evaluate("a + 1");
    let sent = h.sent();
    assert_eq!(sent[0]["arguments"]["frameId"], 10);
    h.respond("evaluate", json!({ "result": "42" }));
    let tail: Vec<(OutputKind, &str)> = h
        .session
        .output
        .iter()
        .rev()
        .take(2)
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert_eq!(
        tail,
        [(OutputKind::Result, "42"), (OutputKind::Input, "a + 1")]
    );
}

#[test]
fn the_exit_code_ends_the_session() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session.continue_();
    h.event("exited", json!({ "exitCode": 3 }));
    h.event("terminated", json!({}));
    assert_eq!(h.session.status, Status::Exited { code: Some(3) });
    assert!(!h.session.is_active());
    assert!(h.commands().contains(&"disconnect".to_string()));
}

#[test]
fn a_failed_launch_says_why() {
    let mut h = Harness::new(Vec::new());
    h.respond("initialize", json!({}));
    let request_seq = h.last_seq_of("launch");
    h.tx.send(Incoming::Message(json!({
        "seq": 5000, "type": "response", "request_seq": request_seq,
        "command": "launch", "success": false,
        "message": "error: executable doesn't exist"
    })))
    .unwrap();
    h.session.poll();
    assert_eq!(
        h.session.status,
        Status::Failed("error: executable doesn't exist".into())
    );
}

#[test]
fn an_adapter_that_hangs_up_while_starting_says_why() {
    // Delve on Android cannot read /proc/net/tcp, takes termide for another
    // user and hangs up; its reason may come through after the hang-up.
    let mut h = Harness::new(Vec::new());
    h.tx.send(Incoming::Log(
        "cannot check remote address: open /proc/net/tcp: permission denied".into(),
    ))
    .unwrap();
    h.tx.send(Incoming::Closed).unwrap();
    h.tx.send(Incoming::Log(
        "closing connection from different user (127.0.0.1:54426)".into(),
    ))
    .unwrap();
    h.session.poll();
    assert_eq!(
        h.session.status,
        Status::Failed(
            "the debug adapter closed the connection: \
             closing connection from different user (127.0.0.1:54426)"
                .into()
        )
    );
    let console: Vec<&str> = h
        .session
        .output
        .iter()
        .filter(|l| l.kind == OutputKind::Console)
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(console.len(), 2, "both lines are shown: {console:?}");
}

#[test]
fn an_adapter_that_hangs_up_after_the_launch_ends_the_session() {
    let mut h = Harness::stopped_at_breakpoint();
    h.tx.send(Incoming::Closed).unwrap();
    h.session.poll();
    assert_eq!(h.session.status, Status::Ended);
}

#[test]
fn a_failed_build_names_its_last_line() {
    let mut h = Harness::new(Vec::new());
    h.session.status = Status::Building;
    for line in [
        "   Compiling app",
        "error[E0425]: cannot find value `x`",
        "",
    ] {
        h.tx.send(Incoming::BuildLine(line.into())).unwrap();
    }
    h.tx.send(Incoming::BuildDone(false)).unwrap();
    h.session.poll();
    assert_eq!(
        h.session.status,
        Status::Failed("the build failed: error[E0425]: cannot find value `x`".into())
    );
}

#[test]
fn a_reverse_request_is_declined() {
    let mut h = Harness::new(Vec::new());
    h.sent();
    h.tx.send(Incoming::Message(json!({
        "seq": 7, "type": "request", "command": "runInTerminal", "arguments": {}
    })))
    .unwrap();
    h.session.poll();
    let sent = h.sent();
    assert_eq!(sent[0]["type"], "response");
    assert_eq!(sent[0]["request_seq"], 7);
    assert_eq!(sent[0]["success"], false);
}

#[test]
fn delve_says_where_it_listens() {
    assert_eq!(
        listen_address("DAP server listening at: 127.0.0.1:38697"),
        Some("127.0.0.1:38697".into())
    );
    assert_eq!(listen_address("Type 'help' for list of commands."), None);
}

#[test]
fn a_tcp_adapter_is_reached_at_the_address_it_printed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (tx, rx) = mpsc::channel();
    let spec = LaunchSpec {
        adapter: Adapter::Delve,
        debugger_settings: Default::default(),
        adapter_args: Vec::new(),
        program: ".".into(),
        args: Vec::new(),
        cwd: "/p".into(),
        build: None,
        env: Vec::new(),
        label: "go".into(),
        console: Console::Panel,
        tty: None,
    };
    let mut session = Session::new(spec, Vec::new(), rx, tx.clone());
    tx.send(Incoming::Listening(address)).unwrap();
    session.poll();

    let (stream, _) = listener.accept().unwrap();
    let mut reader = io::BufReader::new(stream.try_clone().unwrap());
    let init = read_message(&mut reader).unwrap().unwrap();
    assert_eq!(init["command"], "initialize");
    assert_eq!(init["arguments"]["adapterID"], "go");

    // Its answer comes back over the same socket.
    let mut writer = stream;
    writer
        .write_all(&crate::dap::transport::encode(&json!({
            "seq": 1, "type": "response", "request_seq": init["seq"],
            "command": "initialize", "success": true, "body": {}
        })))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !session
        .pending
        .values()
        .any(|p| matches!(p, Pending::Launch))
    {
        assert!(std::time::Instant::now() < deadline, "no launch was sent");
        session.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let launch = read_message(&mut reader).unwrap().unwrap();
    assert_eq!(launch["command"], "launch");
    assert_eq!(launch["arguments"]["mode"], "debug");
    assert_eq!(launch["arguments"]["program"], ".");
}

#[test]
fn the_session_reads_as_text_for_an_agent() {
    let mut h = Harness::stopped_at_breakpoint();
    assert!(!h.session.is_busy(), "stack and locals are in");
    h.event(
        "output",
        json!({ "category": "stdout", "output": "hello\n" }),
    );
    let text = h.session.describe(10);
    assert!(text.contains("stopped: breakpoint"), "{text}");
    assert!(text.contains("At: /p/src/main.rs:4"), "{text}");
    assert!(text.contains("*#0 app::main  /p/src/main.rs:4"), "{text}");
    assert!(text.contains("a: i32 = 41"), "{text}");
    assert!(text.contains("Registers: (not fetched)"), "{text}");
    assert!(text.contains("  hello"), "{text}");

    h.session.step_over();
    assert!(h.session.is_busy());
    h.event("stopped", json!({ "reason": "step", "threadId": 1 }));
    assert!(h.session.is_busy(), "the stack is still to come");
}

#[test]
fn an_evaluation_is_counted_with_its_result() {
    let mut h = Harness::stopped_at_breakpoint();
    h.session.evaluate("a");
    h.respond("evaluate", json!({ "result": "41" }));
    assert_eq!(h.session.evaluations, 1);
    assert_eq!(h.session.last_evaluation, Some(Ok("41".into())));
}

#[test]
fn a_program_asked_for_in_a_terminal_is_started_by_the_app() {
    let (tx, rx) = mpsc::channel();
    let spec = LaunchSpec {
        adapter: Adapter::Debugpy,
        debugger_settings: Default::default(),
        adapter_args: Vec::new(),
        program: "t.py".into(),
        args: Vec::new(),
        cwd: "/p".into(),
        build: None,
        env: Vec::new(),
        label: "t.py".into(),
        console: Console::Terminal,
        tty: None,
    };
    let mut session = Session::new(spec, Vec::new(), rx, tx.clone());
    let wire = Wire::default();
    session.connect(Box::new(wire.clone()));
    let mut h = Harness {
        session,
        wire,
        read: 0,
        tx,
        adapter_seq: 1000,
    };
    let init = h.sent();
    assert_eq!(init[0]["arguments"]["supportsRunInTerminalRequest"], true);
    h.respond("initialize", json!({}));
    assert_eq!(h.sent()[0]["arguments"]["console"], "integratedTerminal");

    h.tx.send(Incoming::Message(json!({
        "seq": 9, "type": "request", "command": "runInTerminal",
        "arguments": {
            "kind": "integrated", "title": "Python Debug Console", "cwd": "/p",
            "args": ["python", "launcher", "--", "t.py"], "env": { "A": "1", "B": null }
        }
    })))
    .unwrap();
    h.session.poll();
    let request = h
        .session
        .take_terminal_request()
        .expect("asked for a terminal");
    assert_eq!(request.args, ["python", "launcher", "--", "t.py"]);
    assert_eq!(request.env, [("A".to_string(), "1".to_string())]);
    assert_eq!(request.cwd, Some(PathBuf::from("/p")));
    assert!(h.session.take_terminal_request().is_none(), "taken once");

    h.session.answer_terminal(&request, Ok(4242));
    let sent = h.sent();
    assert_eq!(sent[0]["request_seq"], 9);
    assert_eq!(sent[0]["success"], true);
    assert_eq!(sent[0]["body"]["processId"], 4242);
}

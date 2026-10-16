//! A session driven by a scripted adapter: what it sends is read back from
//! the writer, what the adapter answers is put in its inbox.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::*;
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
            program: "target/debug/app".into(),
            args: vec!["--flag".into()],
            cwd: "/p".into(),
            build: None,
            env: Vec::new(),
            label: "app".into(),
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

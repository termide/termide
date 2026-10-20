//! The Python library and examples under `examples/plugins`: each example
//! carries the same copy of the library, and an example speaks the protocol
//! end to end, asking termide back while it answers.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use termide_plugins::rpc::{Incoming, Peer};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

#[test]
fn every_example_carries_the_library_as_it_is() {
    let library = std::fs::read_to_string(examples().join("sdk/termide_plugin.py")).unwrap();
    for example in ["trim-on-save", "todo-lint", "word-count"] {
        let copy =
            std::fs::read_to_string(examples().join(example).join("termide_plugin.py")).unwrap();
        assert!(
            copy == library,
            "{example}/termide_plugin.py differs from sdk/termide_plugin.py: copy it again"
        );
        assert!(examples().join(example).join("plugin.toml").is_file());
    }
}

#[test]
fn trim_on_save_reads_the_buffer_back_and_answers_with_edits() {
    let Ok(python) = which_python() else {
        eprintln!("no python3: skipped");
        return;
    };
    // `-B`: importing the library must leave no `__pycache__` in the tree.
    let mut child = Command::new(python)
        .arg("-B")
        .arg(examples().join("trim-on-save/run.py"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();

    // termide's side: it answers the plugin's editor/read with a buffer.
    let slot: Arc<Mutex<Option<Arc<Peer>>>> = Arc::new(Mutex::new(None));
    let answering = Arc::clone(&slot);
    let peer = Arc::new(Peer::new(
        stdout,
        stdin,
        move |incoming| {
            if let Incoming::Request { id, method, .. } = incoming {
                assert_eq!(method, "editor/read");
                let peer = answering.lock().unwrap().clone().unwrap();
                peer.respond(
                    &id,
                    Ok(json!({ "text": "a  \nb\nc\u{1F600}\t\n", "version": 3 })),
                )
                .unwrap();
            }
        },
        || {},
    ));
    *slot.lock().unwrap() = Some(Arc::clone(&peer));

    let second = Duration::from_secs(10);
    let init = peer
        .request(
            "initialize",
            json!({ "plugin": { "name": "trim" } }),
            second,
            &|| false,
        )
        .unwrap();
    assert_eq!(init["subscriptions"], json!(["file/will_save"]));
    let answer: Value = peer
        .request(
            "file/will_save",
            json!({ "path": "/p/a.txt", "version": 3 }),
            second,
            &|| false,
        )
        .unwrap();
    // The emoji is two UTF-16 units: the tab after it starts at column 3.
    assert_eq!(
        answer["edits"],
        json!([
            { "line": 0, "col": 1, "end_line": 0, "end_col": 3, "text": "" },
            { "line": 2, "col": 3, "end_line": 2, "end_col": 4, "text": "" },
        ])
    );
    peer.notify("exit", json!({})).unwrap();
    let _ = child.wait();
}

fn which_python() -> Result<&'static str, ()> {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|_| "python3")
        .map_err(|_| ())
}

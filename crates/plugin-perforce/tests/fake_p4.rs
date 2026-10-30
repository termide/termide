//! The plugin end to end against a fake `p4`, termide's side played by a
//! peer on a pair of pipes.

// The fake `p4` is a shell script.
#![cfg(unix)]

use std::io::pipe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use termide_plugins::rpc::{Incoming, Peer};

/// A `p4` that knows one workspace and one file in it, `a.c`: at #3 of 5,
/// opened by someone else, read-only until `p4 edit` opens it.
const FAKE_P4: &str = r#"#!/bin/sh
root="$FAKE_ROOT"
case "$*" in
"-ztag info")
    printf '... clientName ws\n... clientRoot %s\n\n' "$root" ;;
"-ztag fstat $root/a.c")
    printf '... depotFile //depot/a.c\n... clientFile %s/a.c\n... headRev 5\n... haveRev 3\n' "$root"
    printf '... otherOpen0 bob@other\n... otherAction0 edit\n... otherOpen 1\n'
    if [ -f "$root/.opened" ]; then printf '... action edit\n... change default\n'; fi ;;
"-ztag fstat -Ro $root/...")
    if [ -f "$root/.opened" ]; then
        printf '... depotFile //depot/a.c\n... clientFile %s/a.c\n... action edit\n... change default\n' "$root"
    else
        echo "$root/... - file(s) not opened on this client." >&2
    fi ;;
"-ztag changes -s pending -c ws")
    ;;
"print -q $root/a.c#3")
    printf 'int main;\n' ;;
"edit $root/a.c")
    touch "$root/.opened"; chmod u+w "$root/a.c"
    echo "//depot/a.c#3 - opened for edit" ;;
"diff -du $root/a.c")
    printf '==== //depot/a.c#3 - %s/a.c ====\n' "$root"
    printf -- '--- //depot/a.c\t2026/10/01 10:00:00\n+++ %s/a.c\t2026/10/02 10:00:00\n' "$root"
    printf '@@ -1 +1 @@\n-int main;\n+int main(void);\n' ;;
"revert $root/a.c")
    rm -f "$root/.opened"; chmod u-w "$root/a.c"
    echo "//depot/a.c#3 - was edit, reverted" ;;
*)
    echo "$* - no such file(s)." >&2 ;;
esac
"#;

/// The file's state in the status bar, nothing outside the workspace, and
/// `Ctrl+E` taken over: on the read-only file `p4 edit`, asked about first
/// when someone else has the file open; back off on the opened file, a
/// choice between reverting it and only viewing it. The
/// workspace root is known to p4 through a symbolic link, as `/tmp` is on
/// macOS, while termide gives real paths: the plugin has to write them as
/// p4 does.
#[test]
fn shows_the_state_and_opens_for_edit() {
    let tmp = tempfile::tempdir().unwrap();
    // Unix only, so no `\\?\` paths: the plugin compares real paths.
    #[allow(clippy::disallowed_methods)]
    let base = tmp.path().canonicalize().unwrap();
    let root = base.join("ws");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("a.c");
    std::fs::write(&file, "int main;\n").unwrap();
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&file, permissions).unwrap();
    let spelled = base.join("link-ws");
    std::os::unix::fs::symlink(&root, &spelled).unwrap();
    // The script reads its root from the environment: a wrapper sets it,
    // so the test process's own environment is left alone.
    let script = base.join("p4-script");
    std::fs::write(&script, FAKE_P4).unwrap();
    let fake = base.join("p4");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nFAKE_ROOT='{}' exec /bin/sh '{}' \"$@\"\n",
            spelled.display(),
            script.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let (plugin_in, termide_out) = pipe().unwrap();
    let (termide_in, plugin_out) = pipe().unwrap();
    let program: PathBuf = fake;
    let served = std::thread::spawn(move || {
        termide_plugin_perforce::serve(plugin_in, plugin_out, program);
    });

    // termide's side: the active editor shows `a.c`, every question is
    // answered yes, and the status bar texts are kept.
    let statuses: Arc<Mutex<Vec<String>>> = Arc::default();
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let marks: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let panels: Arc<Mutex<Vec<Value>>> = Arc::default();
    let opened_panels = Arc::clone(&panels);
    // What each `window/choose` is answered, in turn.
    let choices: Arc<Mutex<Vec<Value>>> = Arc::default();
    let picks = Arc::clone(&choices);
    let slot: Arc<Mutex<Option<Arc<Peer>>>> = Arc::new(Mutex::new(None));
    let (answering, seen, questions, marked) = (
        Arc::clone(&slot),
        Arc::clone(&statuses),
        Arc::clone(&asked),
        Arc::clone(&marks),
    );
    let active = file.to_string_lossy().into_owned();
    let peer = Arc::new(Peer::new(
        termide_in,
        termide_out,
        move |incoming| {
            let Incoming::Request { id, method, params } = incoming else {
                return;
            };
            let result = match method.as_str() {
                "status/set" => {
                    seen.lock()
                        .unwrap()
                        .push(params["text"].as_str().unwrap().to_string());
                    Value::Null
                }
                "editor/active" => json!({ "path": active }),
                "files/status" | "editor/base" => {
                    marked.lock().unwrap().push((method.clone(), params));
                    Value::Null
                }
                "window/confirm" => {
                    questions
                        .lock()
                        .unwrap()
                        .push(params["message"].as_str().unwrap().to_string());
                    json!(true)
                }
                "window/choose" => {
                    let first = &params["items"][0];
                    if params["title"] == "Stop editing a.c" {
                        assert_eq!(first, "Only stop editing: keep it opened for edit");
                    } else {
                        assert_eq!(params["title"], "Stop editing a.c: it has unsaved changes");
                        assert_eq!(first, "Save, then stop editing: keep it opened for edit");
                    }
                    picks.lock().unwrap().remove(0)
                }
                "panel/open" => {
                    opened_panels.lock().unwrap().push(params);
                    json!({ "panel": 1 })
                }
                "window/notify" => panic!("unexpected notice: {params}"),
                other => panic!("unexpected request {other}"),
            };
            let peer = answering.lock().unwrap().clone().unwrap();
            peer.respond(&id, Ok(result)).unwrap();
        },
        || {},
    ));
    *slot.lock().unwrap() = Some(Arc::clone(&peer));

    let wait = Duration::from_secs(10);
    let init = peer
        .request(
            "initialize",
            json!({
                "plugin": { "name": "perforce" },
                "project": { "root": root },
            }),
            wait,
            &|| false,
        )
        .unwrap();
    let commands: Vec<&str> = init["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(commands, ["edit", "revert", "diff", "opened", "sync"]);
    // The opened files stand in for git's status panel (Alt+G); no command
    // takes a key of its own.
    assert_eq!(init["commands"][3]["replaces"], "open_git_status");
    assert!(init["commands"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c.get("key").is_none()));
    peer.notify("initialized", json!({})).unwrap();

    let status_count = |n: usize| {
        for _ in 0..200 {
            if statuses.lock().unwrap().len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("no status after {:?}", statuses.lock().unwrap());
    };

    peer.notify("file/opened", json!({ "path": file })).unwrap();
    status_count(1);
    assert_eq!(
        statuses.lock().unwrap()[0],
        "p4: #3/5 (out of date) · also opened by bob@other"
    );

    // Outside the workspace there is nothing to say.
    peer.notify("editor/focused", json!({ "path": base.join("p4") }))
        .unwrap();
    status_count(2);
    assert_eq!(statuses.lock().unwrap()[1], "");

    // A file outside the workspace is not the plugin's to make writable.
    let outside = peer
        .request(
            "file/will_edit",
            json!({ "path": base.join("p4") }),
            wait,
            &|| false,
        )
        .unwrap();
    assert_eq!(outside, json!({ "handled": false }));

    let answer = peer
        .request("file/will_edit", json!({ "path": file }), wait, &|| false)
        .unwrap();
    assert_eq!(answer, json!({ "handled": true }));
    status_count(3);
    assert_eq!(
        statuses.lock().unwrap()[2],
        "p4: edit #3/5 (out of date) · also opened by bob@other"
    );
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "asked about bob's copy first"
    );
    assert!(!std::fs::metadata(&file).unwrap().permissions().readonly());

    // The editor got the depot's text to mark changes against, and the
    // file manager the opened file — under the project root as termide
    // spells it, not through p4's symbolic link.
    let marks = marks.lock().unwrap().clone();
    assert_eq!(
        marks[0],
        (
            "editor/base".to_string(),
            json!({ "path": file, "text": "int main;\n" })
        )
    );
    assert_eq!(
        marks[1],
        (
            "files/status".to_string(),
            json!({ "items": [{ "path": file, "status": "modified" }] })
        )
    );
    assert_eq!(marks.len(), 2, "the base is not sent again: {marks:?}");

    // Opened already: p4 would not make it writable, termide decides.
    let again = peer
        .request("file/will_edit", json!({ "path": file }), wait, &|| false)
        .unwrap();
    assert_eq!(again, json!({ "handled": false }));

    // The opened files, as a list: a group by changelist with its submit,
    // a row by file with the keys git status gives the same actions.
    peer.request("command/execute", json!({ "id": "opened" }), wait, &|| {
        false
    })
    .unwrap();
    let panel = panels.lock().unwrap().remove(0);
    assert_eq!(panel["title"], "Opened files");
    assert!(panel.get("markdown").is_none());
    let group = &panel["list"]["groups"][0];
    assert_eq!(group["label"], "Default changelist");
    assert_eq!(group["actions"][0]["action"], "submit-default");
    let row = &group["items"][0];
    assert_eq!(row["label"], "a.c");
    assert_eq!(
        (row["detail"].as_str(), row["status"].as_str()),
        (Some("edit"), Some("modified"))
    );
    let keys: Vec<&str> = row["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["edit", "view", "revert"]);
    assert_eq!(panel["list"]["refresh"], "refresh");

    // The file's changes, as a unified diff named by its place in the
    // project rather than by p4's depot and local paths.
    peer.request("command/execute", json!({ "id": "diff" }), wait, &|| false)
        .unwrap();
    let panel = panels.lock().unwrap().remove(0);
    assert_eq!(panel["title"], "diff a.c");
    assert_eq!(
        panel["diff"],
        "--- a/a.c\n+++ b/a.c\n@@ -1 +1 @@\n-int main;\n+int main(void);\n"
    );

    // Turning editing off on the opened file asks what to do with it.
    let stop = |pick: Value, modified: bool| {
        choices.lock().unwrap().push(pick);
        peer.request(
            "file/will_edit",
            json!({ "path": file, "edit": false, "modified": modified }),
            wait,
            &|| false,
        )
        .unwrap()
    };
    // Dismissed: editing stays on.
    assert_eq!(stop(Value::Null, false), json!({ "handled": true }));
    // Only viewing: termide turns editing off, the file stays opened.
    assert_eq!(stop(json!(0), false), json!({ "handled": false }));
    // Unsaved work is saved or dropped by termide; the file stays opened.
    assert_eq!(stop(json!(0), true), json!({ "buffer": "save" }));
    assert_eq!(stop(json!(1), true), json!({ "buffer": "discard" }));
    assert!(root.join(".opened").exists());
    // Reverted: read-only again, and the buffer takes the depot's text.
    assert_eq!(stop(json!(1), false), json!({ "buffer": "discard" }));
    assert!(!root.join(".opened").exists());
    assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
    assert!(choices.lock().unwrap().is_empty());
    // Not opened any more: nothing to ask.
    let viewing = peer
        .request(
            "file/will_edit",
            json!({ "path": file, "edit": false }),
            wait,
            &|| false,
        )
        .unwrap();
    assert_eq!(viewing, json!({ "handled": false }));

    peer.notify("exit", json!({})).unwrap();
    served.join().unwrap();
}

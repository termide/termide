//! A real plugin process: a shell script that speaks just enough of the
//! protocol, found and started the way termide does it.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use termide_plugins::{discover, Host, HostEvent, Launch, ProcessLauncher, Timing};

/// Answers `initialize` (request 1) and `shutdown` (request 2), records the
/// directory it runs in and every line it is sent.
const SCRIPT: &str = r#"#!/bin/sh
pwd > "$OUT/cwd"
sleep 300 &
echo $! > "$OUT/child"
read -r line
echo "$line" >> "$OUT/received"
echo '{"jsonrpc":"2.0","id":1,"result":{"subscriptions":["file/saved"],"commands":[{"id":"hi","title":"Hi"}]}}'
while read -r line; do
  echo "$line" >> "$OUT/received"
  case "$line" in
    *'"shutdown"'*) echo '{"jsonrpc":"2.0","id":2,"result":null}' ;;
    *'"exit"'*) exit 0 ;;
  esac
done
"#;

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
// Unix only: no verbatim `\\?\` paths to guard against.
#[allow(clippy::disallowed_methods)]
fn a_script_plugin_runs_in_its_directory_and_stops_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let plugins = root.path().join("plugins");
    let dir = plugins.join("echo");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("run.sh");
    std::fs::write(&script, SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "command = \"./run.sh\"\n[env]\nOUT = \"{}\"\n",
            out.display()
        ),
    )
    .unwrap();

    let (installed, problems) = discover(&plugins);
    assert!(problems.is_empty(), "{problems:?}");
    let plugin = &installed[0];
    let launch = Launch {
        name: plugin.name.clone(),
        dir: plugin.dir.clone(),
        program: plugin.program().unwrap(),
        args: plugin.manifest.args.clone(),
        env: plugin.manifest.env.clone(),
        settings: json!({ "level": 2 }),
    };
    let project = root.path().join("project");
    let mut host = Host::new(Arc::new(ProcessLauncher), Timing::default());
    host.set_project(&project, vec![launch]);

    let deadline = Instant::now() + Duration::from_secs(5);
    let contributions = loop {
        assert!(Instant::now() < deadline, "the plugin never became ready");
        if let Some(HostEvent::Ready { contributions, .. }) = host
            .poll()
            .into_iter()
            .find(|e| matches!(e, HostEvent::Ready { .. }))
        {
            break contributions;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(contributions.commands[0].title, "Hi");

    let cwd = std::fs::read_to_string(out.join("cwd")).unwrap();
    assert_eq!(
        Path::new(cwd.trim()).canonicalize().unwrap(),
        dir.canonicalize().unwrap()
    );
    let received = || std::fs::read_to_string(out.join("received")).unwrap_or_default();
    assert!(received().contains("\"level\":2"), "{}", received());

    host.emit(&project, "file/saved", &json!({ "path": "/a.rs" }));
    wait_until("file/saved", || received().contains("file/saved"));

    host.shutdown();
    assert!(received().contains("\"shutdown\""), "{}", received());
    assert!(received().contains("\"exit\""), "{}", received());

    // What the plugin started went with it.
    let child = std::fs::read_to_string(out.join("child")).unwrap();
    let alive = || {
        std::process::Command::new("kill")
            .args(["-0", child.trim()])
            .status()
            .is_ok_and(|status| status.success())
    };
    wait_until("the plugin's child to end", || !alive());
}

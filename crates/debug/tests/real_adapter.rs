//! Sessions against real debug adapters. Ignored by default: they need the
//! adapter installed and, for lldb, the right to debug processes. Run with
//! `cargo test -p termide-debug --test real_adapter -- --ignored --nocapture`.

use std::path::Path;
use std::time::{Duration, Instant};

use termide_debug::dap::{Adapter, LaunchSpec, Session, Status};
use termide_debug::Breakpoint;

fn wait_until(session: &mut Session, what: &str, done: impl Fn(&Session) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(session) {
        session.poll();
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; status {:?}, output {:#?}",
            session.status,
            session.output
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run(adapter: Adapter, dir: &Path, program: &str, source: &Path, line: usize) {
    let mut session = Session::start(
        LaunchSpec {
            adapter,
            program: program.into(),
            args: Vec::new(),
            cwd: dir.to_path_buf(),
            build: None,
            env: Vec::new(),
            label: "smoke".into(),
        },
        vec![Breakpoint {
            path: source.to_path_buf(),
            line,
        }],
    );
    wait_until(&mut session, "the stop", |s| {
        s.execution_point().is_some() || !s.is_active()
    });
    println!("stopped: {:?}", session.status);
    println!("frames: {:#?}", session.frames);
    assert_eq!(
        session.execution_point().map(|(_, l)| l),
        Some(line),
        "{:?}",
        session.status
    );
    wait_until(&mut session, "the locals", |s| {
        s.scopes.first().is_some_and(|s| s.children.is_some())
    });
    println!("locals: {:#?}", session.scopes[0].children);
    session.step_over();
    wait_until(&mut session, "the step", |s| {
        s.execution_point().is_some_and(|(_, l)| l != line)
    });
    session.continue_();
    wait_until(&mut session, "the end", |s| !s.is_active());
    println!("ended: {:?}\noutput: {:#?}", session.status, session.output);
    assert!(matches!(
        session.status,
        Status::Exited { .. } | Status::Ended
    ));
}

#[test]
#[ignore]
fn lldb_dap_stops_steps_and_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dunce_like(dir.path());
    let source = dir.join("m.c");
    std::fs::write(
        &source,
        "#include <stdio.h>\nint main(void) {\n  int a = 41;\n  a = a + 1;\n  printf(\"%d\\n\", a);\n  return 0;\n}\n",
    )
    .unwrap();
    let ok = std::process::Command::new("cc")
        .args(["-g", "-O0", "m.c", "-o", "m"])
        .current_dir(&dir)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    run(Adapter::LldbDap, &dir, "./m", &source, 4);
}

#[test]
#[ignore]
fn debugpy_stops_steps_and_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dunce_like(dir.path());
    let source = dir.join("t.py");
    std::fs::write(&source, "a = 41\na = a + 1\nprint(a)\n").unwrap();
    run(
        Adapter::Debugpy,
        &dir,
        &source.to_string_lossy(),
        &source,
        2,
    );
}

/// The temporary directory by its real name (`/private/var` on macOS), as
/// the debuggee reports its files.
fn dunce_like(path: &Path) -> std::path::PathBuf {
    dunce::canonicalize(path).unwrap()
}

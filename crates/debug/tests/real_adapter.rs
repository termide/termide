//! Sessions against real debug adapters. Ignored by default: they need the
//! adapter installed and, for lldb, the right to debug processes. Run with
//! `cargo test -p termide-debug --test real_adapter -- --ignored --nocapture`.

use std::path::Path;
use std::time::{Duration, Instant};

use termide_debug::dap::{Adapter, Console, LaunchSpec, Session, Status};
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
            debugger_settings: Default::default(),
            adapter_args: Vec::new(),
            program: program.into(),
            args: Vec::new(),
            cwd: dir.to_path_buf(),
            build: None,
            env: Vec::new(),
            label: "smoke".into(),
            console: Console::Panel,
            tty: None,
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
    let ok = termide_debug::detach(&mut std::process::Command::new("cc"))
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

#[test]
#[ignore]
fn delve_stops_steps_and_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dunce_like(dir.path());
    let source = dir.join("main.go");
    std::fs::write(
        &source,
        "package main\n\nimport \"fmt\"\n\nfunc main() {\n\ta := 41\n\ta = a + 1\n\tfmt.Println(a)\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("go.mod"), "module smoke\n\ngo 1.21\n").unwrap();
    run(Adapter::Delve, &dir, ".", &source, 7);
}

/// A program that reads the keyboard, run by lldb-dap with its input and
/// output on a terminal device: it waits for a line there, gets it, and
/// stops at the breakpoint after.
#[cfg(unix)]
#[test]
#[ignore]
fn lldb_dap_runs_the_program_on_a_terminal() {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;

    let dir = tempfile::tempdir().unwrap();
    let dir = dunce_like(dir.path());
    let source = dir.join("m.c");
    std::fs::write(
        &source,
        "#include <stdio.h>\nint main(void) {\n  char name[64] = {0};\n  printf(\"name? \"); fflush(stdout);\n  if (!fgets(name, sizeof name, stdin)) return 1;\n  int len = 0;\n  while (name[len] && name[len] != '\\n') len++;\n  printf(\"hello %.*s (%d)\\n\", len, name, len);\n  return 0;\n}\n",
    )
    .unwrap();
    assert!(termide_debug::detach(&mut std::process::Command::new("cc"))
        .args(["-g", "-O0", "m.c", "-o", "m"])
        .current_dir(&dir)
        .status()
        .unwrap()
        .success());

    let (mut master, mut slave_fd) = (0, 0);
    // SAFETY: openpty fills the two descriptors; no name, termios or size.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0);
    // SAFETY: a descriptor openpty just returned.
    let tty = unsafe { std::ffi::CStr::from_ptr(libc::ttyname(slave_fd)) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the master descriptor is ours alone from here on.
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let mut reader = master.try_clone().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut session = Session::start(
        LaunchSpec {
            adapter: Adapter::LldbDap,
            debugger_settings: Default::default(),
            adapter_args: Vec::new(),
            program: "./m".into(),
            args: Vec::new(),
            cwd: dir.clone(),
            build: None,
            env: Vec::new(),
            label: "m".into(),
            console: Console::Terminal,
            tty: Some(tty.into()),
        },
        vec![Breakpoint {
            path: source.clone(),
            line: 8,
        }],
    );
    let mut screen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !String::from_utf8_lossy(&screen).contains("name? ") {
        session.poll();
        while let Ok(bytes) = rx.try_recv() {
            screen.extend(bytes);
        }
        assert!(Instant::now() < deadline, "no prompt: {:?}", session.output);
        std::thread::sleep(Duration::from_millis(20));
    }
    master.write_all(b"world\n").unwrap();
    wait_until(&mut session, "the stop", |s| s.execution_point().is_some());
    assert_eq!(session.execution_point().map(|(_, l)| l), Some(8));
    session.continue_();
    wait_until(&mut session, "the end", |s| !s.is_active());
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(bytes) = rx.try_recv() {
        screen.extend(bytes);
    }
    let screen = String::from_utf8_lossy(&screen);
    assert!(screen.contains("hello world (5)"), "{screen:?}");
}

/// debugpy asks for the program to be started in a terminal; the test does
/// what the app does — starts it on a PTY and answers with its pid — and
/// the program reads its line from there.
#[cfg(unix)]
#[test]
#[ignore]
fn debugpy_runs_the_program_in_a_terminal() {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::process::Stdio;

    let dir = tempfile::tempdir().unwrap();
    let dir = dunce_like(dir.path());
    let source = dir.join("t.py");
    std::fs::write(
        &source,
        "name = input(\"name? \")\ngreeting = \"hello \" + name\nprint(greeting)\n",
    )
    .unwrap();
    let mut session = Session::start(
        LaunchSpec {
            adapter: Adapter::Debugpy,
            debugger_settings: Default::default(),
            adapter_args: Vec::new(),
            program: source.to_string_lossy().into_owned(),
            args: Vec::new(),
            cwd: dir.clone(),
            build: None,
            env: Vec::new(),
            label: "t.py".into(),
            console: Console::Terminal,
            tty: None,
        },
        vec![Breakpoint {
            path: source.clone(),
            line: 3,
        }],
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let request = loop {
        session.poll();
        if let Some(request) = session.take_terminal_request() {
            break request;
        }
        assert!(
            Instant::now() < deadline,
            "no terminal asked for: {:?}",
            session.status
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty fills the two descriptors; no name, termios or size.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0);
    // SAFETY: descriptors openpty just returned, each owned once below.
    let (stdin, stdout, stderr) = unsafe {
        (
            Stdio::from_raw_fd(slave),
            Stdio::from_raw_fd(libc::dup(slave)),
            Stdio::from_raw_fd(libc::dup(slave)),
        )
    };
    let mut command = std::process::Command::new(&request.args[0]);
    // As a terminal panel starts it: a session of its own, with the PTY as
    // its controlling terminal, never the one the tests run in.
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid and ioctl are async-signal-safe; stdin is already
        // the PTY's slave when the closure runs.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
    }
    let mut child = command
        .args(&request.args[1..])
        .current_dir(request.cwd.clone().unwrap_or(dir.clone()))
        .envs(request.env.iter().cloned())
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    session.answer_terminal(&request, Ok(child.id()));

    // SAFETY: the master descriptor is ours alone from here on.
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let mut reader = master.try_clone().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut screen = Vec::new();
    while !String::from_utf8_lossy(&screen).contains("name? ") {
        session.poll();
        while let Ok(bytes) = rx.try_recv() {
            screen.extend(bytes);
        }
        assert!(Instant::now() < deadline, "no prompt: {:?}", session.output);
        std::thread::sleep(Duration::from_millis(20));
    }
    master.write_all(b"world\n").unwrap();
    wait_until(&mut session, "the stop", |s| s.execution_point().is_some());
    assert_eq!(session.execution_point().map(|(_, l)| l), Some(3));
    session.continue_();
    wait_until(&mut session, "the end", |s| !s.is_active());
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(bytes) = rx.try_recv() {
        screen.extend(bytes);
    }
    assert!(
        String::from_utf8_lossy(&screen).contains("hello world"),
        "{:?}",
        String::from_utf8_lossy(&screen)
    );
    let _ = child.wait();
}

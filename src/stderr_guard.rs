//! Keep stray stderr output off the screen while the UI owns the terminal,
//! and show it in the journal instead.
//!
//! Some writers to file descriptor 2 are outside termide's control: macOS
//! frameworks log there on their own (reading file URLs off the pasteboard
//! prints `sandbox_extension_consume failed: 1 (Operation not permitted)`),
//! and so do C libraries and children that inherit stderr. Under raw mode on
//! the alternate screen each such line lands on top of the drawn frame and
//! stays there until the cells under it are redrawn.
//!
//! [`divert`] points fd 2 at a pipe; a thread reads it line by line and logs
//! each line, so it reaches the journal. The logger must be up by then, or
//! the lines are dropped. [`restore`] puts the original stderr back; it runs
//! on normal exit and from the panic hook before the panic message is
//! printed, so that message still reaches the user.

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::os::fd::FromRawFd;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// Duplicate of the original stderr while it is diverted, `-1` otherwise.
    static SAVED: AtomicI32 = AtomicI32::new(-1);

    pub fn divert() {
        if SAVED.load(Ordering::SeqCst) >= 0 {
            return;
        }
        let mut fds = [-1; 2];
        // SAFETY: plain fd syscalls on descriptors owned here; every failure
        // path closes what it opened and leaves fd 2 as it was.
        let reader = unsafe {
            if libc::pipe(fds.as_mut_ptr()) < 0 {
                return;
            }
            let [read_end, write_end] = fds;
            libc::fcntl(read_end, libc::F_SETFD, libc::FD_CLOEXEC);
            let saved = libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3);
            if saved < 0 || libc::dup2(write_end, libc::STDERR_FILENO) < 0 {
                if saved >= 0 {
                    libc::close(saved);
                }
                libc::close(read_end);
                libc::close(write_end);
                return;
            }
            libc::close(write_end);
            SAVED.store(saved, Ordering::SeqCst);
            File::from_raw_fd(read_end)
        };

        // Ends at EOF, once `restore` has closed the last write end in this
        // process (a child that inherited fd 2 may keep it open longer).
        let spawned = std::thread::Builder::new()
            .name("stderr-to-journal".into())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let text = String::from_utf8_lossy(&line);
                            let text = text.trim_end();
                            if !text.is_empty() {
                                log::info!("stderr: {text}");
                            }
                        }
                    }
                }
            });
        if spawned.is_err() {
            // Nobody would drain the pipe: writers would block once it fills.
            restore();
        }
    }

    pub fn restore() {
        let saved = SAVED.swap(-1, Ordering::SeqCst);
        if saved < 0 {
            return;
        }
        // SAFETY: `saved` is the duplicate made by `divert` and owned here.
        unsafe {
            libc::dup2(saved, libc::STDERR_FILENO);
            libc::close(saved);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            LINES.lock().unwrap().push(record.args().to_string());
        }
        fn flush(&self) {}
    }

    #[test]
    fn stderr_lines_reach_the_log() {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Info);

        super::divert();
        let msg = b"x_extension_consume failed: 1\n\nsecond line\r\n";
        // SAFETY: writes a byte buffer to fd 2.
        unsafe { libc::write(libc::STDERR_FILENO, msg.as_ptr().cast(), msg.len()) };
        super::restore();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let lines = LINES.lock().unwrap().clone();
            let ours: Vec<_> = lines.iter().filter(|l| l.starts_with("stderr: ")).collect();
            if ours.len() >= 2 {
                assert_eq!(ours[0], "stderr: x_extension_consume failed: 1");
                assert_eq!(ours[1], "stderr: second line");
                break;
            }
            assert!(Instant::now() < deadline, "got {lines:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn divert() {}
    pub fn restore() {}
}

/// Send stderr to the journal. Idempotent. Call once the logger is up.
pub fn divert() {
    imp::divert();
}

/// Give stderr back its original target. Idempotent; a no-op when not diverted.
pub fn restore() {
    imp::restore();
}

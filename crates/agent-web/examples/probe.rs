//! Live check of the web tools on a scratch browser profile:
//!
//! ```text
//! cargo run -p termide-agent-web --example probe -- search <query> [engine]
//! cargo run -p termide-agent-web --example probe -- fetch <url> [http]
//! ```
//!
//! `PROBE_DISPLAY=headless|minimized|visible` picks the browser display;
//! `PROBE_BACKEND=http` searches without the browser.

use termide_agent_core::{CancelToken, SEED_ENGINES};
use termide_agent_web::{Backend, Display, Engine, Web, WebConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, subject) = match args.as_slice() {
        [command, subject, ..] => (command.as_str(), subject.as_str()),
        _ => {
            eprintln!("usage: probe search <query> [engine] | probe fetch <url> [http]");
            std::process::exit(2);
        }
    };
    let extra = args.get(2).map(String::as_str);
    let engine_name = if command == "search" {
        extra.unwrap_or("duckduckgo")
    } else {
        "duckduckgo"
    };
    let engine = SEED_ENGINES
        .iter()
        .find(|(name, _)| *name == engine_name)
        .map(|(_, text)| Engine::from_toml(text).expect("seed parses"));
    let backend =
        if extra == Some("http") || std::env::var("PROBE_BACKEND").as_deref() == Ok("http") {
            Backend::Http
        } else {
            Backend::Auto
        };
    if command == "js" {
        // Raw page access: open the URL and print the value of a script.
        let chrome = termide_agent_web::find_chrome().expect("no browser found");
        let display = Display::parse(&std::env::var("PROBE_DISPLAY").unwrap_or_default());
        let profile = std::env::temp_dir().join("termide-probe-profile");
        let browser =
            termide_agent_web::Browser::launch(&chrome, &profile, display).expect("launch");
        let cancel = CancelToken::new();
        // A second URL after `::` opens after the first, to see the tab reused.
        let (first, second) = subject.split_once("::").unwrap_or((subject, ""));
        if !second.is_empty() {
            drop(browser.open(first, &cancel).expect("open"));
        }
        let target = if second.is_empty() { first } else { second };
        let page = browser.open(target, &cancel).expect("open");
        println!(
            "{}",
            page.eval(extra.unwrap_or("document.title"), &cancel)
                .expect("eval")
        );
        drop(page);
        browser.close();
        return;
    }
    let web = Web::new(WebConfig {
        backend,
        engine,
        chrome_path: None,
        display: Display::parse(&std::env::var("PROBE_DISPLAY").unwrap_or_default()),
        profile: std::env::temp_dir().join("termide-probe-profile"),
    });
    let cancel = CancelToken::new();
    let started = std::time::Instant::now();
    match command {
        "search" => {
            let mut on_wait = |message: String| eprintln!("[wait] {message}");
            match web.search(subject, 10, &cancel, &mut on_wait) {
                Ok(results) => {
                    for (n, result) in results.iter().enumerate() {
                        println!(
                            "{}. {}\n   {}\n   {}",
                            n + 1,
                            result.title,
                            result.url,
                            result.snippet
                        );
                    }
                }
                Err(error) => println!("error: {error}"),
            }
        }
        _ => match web.fetch(subject, &cancel) {
            Ok(page) => {
                println!("URL: {}\nTitle: {}\n", page.url, page.title);
                let lines: Vec<&str> = page.content.lines().collect();
                for line in lines.iter().take(60) {
                    println!("{line}");
                }
                println!("[{} lines, {} bytes]", lines.len(), page.content.len());
            }
            Err(error) => println!("error: {error}"),
        },
    }
    eprintln!("took {:?}", started.elapsed());
}

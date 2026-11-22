//! Print the commit graph of the repository in the current directory, for
//! eyeballing the renderer against a real history:
//!
//! ```text
//! cargo run -p termide-git --example graph -- [count]
//! ```
//!
//! `count` is how many commits of all branches to draw (30 by default).

use termide_git::graph::{parse_parents_log, render_graph};

fn main() {
    let count = std::env::args().nth(1).unwrap_or_else(|| "30".to_string());
    let out = std::process::Command::new("git")
        .args(["log", "--format=%h\t%p", "-n", &count, "--branches"])
        .output()
        .expect("git log");
    if !out.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr).trim());
        std::process::exit(1);
    }
    let commits = parse_parents_log(&String::from_utf8_lossy(&out.stdout));
    for row in render_graph(&commits) {
        match row.commit {
            Some(i) => println!("{:<6}{}", row.graph, commits[i].hash),
            None => println!("{}", row.graph),
        }
    }
}

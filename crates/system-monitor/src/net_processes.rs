//! Network process tracking — platform-specific implementations.

use super::NetworkProcessInfo;

pub fn collect() -> Vec<NetworkProcessInfo> {
    platform::collect()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::NetworkProcessInfo;
    use std::collections::{BTreeSet, HashMap};
    use std::fs;

    pub fn collect() -> Vec<NetworkProcessInfo> {
        // Map socket inode -> (is_listen, local_port)
        let mut inode_map: HashMap<u64, (bool, u16)> = HashMap::new();
        parse_tcp_file("/proc/net/tcp", &mut inode_map);
        parse_tcp_file("/proc/net/tcp6", &mut inode_map);

        if inode_map.is_empty() {
            return Vec::new();
        }

        // Map process name -> (listening ports, connection count)
        let mut by_name: HashMap<String, (BTreeSet<u16>, usize)> = HashMap::new();
        let Ok(proc_dir) = fs::read_dir("/proc") else {
            return Vec::new();
        };

        for entry in proc_dir.flatten() {
            let fname = entry.file_name();
            let pid_str = fname.to_string_lossy();
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(fd_dir) = fs::read_dir(format!("/proc/{}/fd", pid_str)) else {
                continue;
            };

            let mut matched: Vec<u64> = Vec::new();
            for fd in fd_dir.flatten() {
                let Ok(target) = fs::read_link(fd.path()) else {
                    continue;
                };
                let t = target.to_string_lossy();
                if let Some(s) = t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
                    if let Ok(inode) = s.parse::<u64>() {
                        matched.push(inode);
                    }
                }
            }
            if matched.is_empty() {
                continue;
            }

            let comm_path = format!("/proc/{}/comm", pid_str);
            let Ok(comm) = fs::read_to_string(&comm_path) else {
                continue;
            };
            let name = comm.trim().to_string();
            if name.is_empty() {
                continue;
            }

            let entry = by_name.entry(name).or_insert_with(|| (BTreeSet::new(), 0));
            for inode in &matched {
                if let Some((is_listen, port)) = inode_map.get(inode) {
                    if *is_listen {
                        if *port > 0 {
                            entry.0.insert(*port);
                        }
                    } else {
                        entry.1 += 1;
                    }
                }
            }
        }

        by_name
            .into_iter()
            .filter(|(_, (ports, conns))| !ports.is_empty() || *conns > 0)
            .map(|(name, (ports, connections))| NetworkProcessInfo {
                name,
                listening_ports: ports.into_iter().collect(),
                connections,
            })
            .collect()
    }

    fn parse_tcp_file(path: &str, inode_map: &mut HashMap<u64, (bool, u16)>) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        for line in content.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 {
                continue;
            }
            let state = u8::from_str_radix(cols[3], 16).unwrap_or(0);
            let is_listen = state == 0x0A;
            let is_established = state == 0x01;
            if !is_listen && !is_established {
                continue;
            }
            let inode: u64 = cols[9].parse().unwrap_or(0);
            if inode == 0 {
                continue;
            }
            let port = cols[1]
                .split(':')
                .nth(1)
                .and_then(|p| u16::from_str_radix(p, 16).ok())
                .unwrap_or(0);
            inode_map.entry(inode).or_insert((is_listen, port));
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::NetworkProcessInfo;
    use std::collections::{BTreeSet, HashMap};
    use std::process::Command;

    pub fn collect() -> Vec<NetworkProcessInfo> {
        let Ok(output) = Command::new("lsof")
            .args(["-i", "TCP", "-n", "-P"])
            .output()
        else {
            return Vec::new();
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        // COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME [STATE]
        let mut by_name: HashMap<String, (BTreeSet<u16>, usize)> = HashMap::new();

        for line in stdout.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 9 {
                continue;
            }
            let cmd = cols[0];
            let name_col = cols[8];
            let state = cols.get(9).copied().unwrap_or("");
            let entry = by_name
                .entry(cmd.to_string())
                .or_insert_with(|| (BTreeSet::new(), 0));
            if state == "(LISTEN)" {
                if let Some(port_str) = name_col.rsplit(':').next() {
                    if let Ok(port) = port_str.parse::<u16>() {
                        entry.0.insert(port);
                    }
                }
            } else if state == "(ESTABLISHED)" {
                entry.1 += 1;
            }
        }

        by_name
            .into_iter()
            .filter(|(_, (ports, conns))| !ports.is_empty() || *conns > 0)
            .map(|(name, (ports, connections))| NetworkProcessInfo {
                name,
                listening_ports: ports.into_iter().collect(),
                connections,
            })
            .collect()
    }
}

#[cfg(windows)]
mod platform {
    use super::NetworkProcessInfo;
    use std::collections::{BTreeSet, HashMap};
    use std::process::Command;

    pub fn collect() -> Vec<NetworkProcessInfo> {
        // Build PID -> process name map from tasklist
        let mut pid_to_name: HashMap<u32, String> = HashMap::new();
        if let Ok(out) = Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let fields: Vec<&str> = line.splitn(6, ',').collect();
                if fields.len() < 2 {
                    continue;
                }
                let name = fields[0].trim_matches('"').trim_end_matches(".exe");
                let pid_str = fields[1].trim_matches('"');
                if let Ok(pid) = pid_str.parse::<u32>() {
                    pid_to_name.insert(pid, name.to_string());
                }
            }
        }

        let Ok(output) = Command::new("netstat").args(["-ano"]).output() else {
            return Vec::new();
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut by_name: HashMap<String, (BTreeSet<u16>, usize)> = HashMap::new();

        for line in stdout.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // Proto Local Foreign State PID
            if cols.len() < 5 || !cols[0].eq_ignore_ascii_case("TCP") {
                continue;
            }
            let state = cols[3];
            let pid: u32 = cols[4].parse().unwrap_or(0);
            let name = pid_to_name
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| format!("pid:{}", pid));

            let entry = by_name.entry(name).or_insert_with(|| (BTreeSet::new(), 0));
            if state.eq_ignore_ascii_case("LISTENING") {
                if let Some(port_str) = cols[1].rsplit(':').next() {
                    if let Ok(port) = port_str.parse::<u16>() {
                        entry.0.insert(port);
                    }
                }
            } else if state.eq_ignore_ascii_case("ESTABLISHED") {
                entry.1 += 1;
            }
        }

        by_name
            .into_iter()
            .filter(|(_, (ports, conns))| !ports.is_empty() || *conns > 0)
            .map(|(name, (ports, connections))| NetworkProcessInfo {
                name,
                listening_ports: ports.into_iter().collect(),
                connections,
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::NetworkProcessInfo;
    pub fn collect() -> Vec<NetworkProcessInfo> {
        Vec::new()
    }
}

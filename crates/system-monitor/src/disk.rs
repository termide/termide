//! Disk space queries and device-name resolution.

use crate::DiskSpaceInfo;
#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::path::Path;

/// Resolve dm-X device to physical partition.
/// e.g., /dev/dm-0 -> /dev/nvme0n1p2
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn resolve_dm_device(device: &str) -> Option<String> {
    // Extract dm number (e.g., "dm-0" from "/dev/dm-0")
    let dm_name = device.strip_prefix("/dev/")?;
    if !dm_name.starts_with("dm-") {
        return None;
    }

    // Read /sys/block/dm-X/slaves/ to find physical partition
    let slaves_path = format!("/sys/block/{}/slaves", dm_name);
    let slaves_dir = std::fs::read_dir(&slaves_path).ok()?;

    // Get first slave (physical partition)
    for entry in slaves_dir.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            return Some(format!("/dev/{}", name));
        }
    }

    None
}

/// One entry of the system mount table: a backing device, its filesystem
/// type and where it is mounted.
#[cfg(unix)]
pub(crate) struct MountEntry {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    /// macOS `MNT_DONTBROWSE`: the system marks the mount as not meant to be
    /// shown to the user (`/System/Volumes/*`, `/Volumes/Recovery`, …).
    pub nobrowse: bool,
}

/// Read the system mount table.
///
/// Linux exposes it as text in `/proc/mounts`; macOS has no `/proc` and
/// answers the same question through `getmntinfo(3)`. Everything downstream —
/// device resolution for a path and the all-devices listing — works off this
/// one list, so the platform difference is confined here.
#[cfg(target_os = "linux")]
fn read_mounts() -> Vec<MountEntry> {
    let Ok(content) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };

    content
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some(MountEntry {
                device: unescape_mount_field(parts.next()?),
                mount_point: unescape_mount_field(parts.next()?),
                fs_type: parts.next().unwrap_or_default().to_string(),
                nobrowse: false,
            })
        })
        .collect()
}

/// Undo the octal escapes `/proc/mounts` writes for whitespace and
/// backslashes in its fields (`\040` for a space, `\134` for `\`), so a
/// mount point such as `/media/me/USB DISK` comes out as a usable path.
#[cfg(target_os = "linux")]
fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            if digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                let value = digits
                    .iter()
                    .fold(0u32, |acc, d| acc * 8 + u32::from(d - b'0'));
                if let Ok(byte) = u8::try_from(value) {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| field.to_string())
}

/// Filesystem types of network and user-space mounts worth offering as a
/// place to go even though they have no `/dev/` device behind them.
#[cfg(unix)]
const BROWSABLE_VIRTUAL_FS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "afpfs",
    "webdav",
    "davfs",
    "9p",
    "drvfs",
    "virtiofs",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.davfs",
    "osxfuse",
    "macfuse",
];

/// Mount points under these prefixes are system plumbing (pseudo filesystems,
/// boot partitions, container layers, macOS system volumes), not places a user
/// browses to. `/run/media` is where udisks mounts removable media and is let
/// through by `is_browsable_mount`.
#[cfg(unix)]
const HIDDEN_MOUNT_PREFIXES: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/boot",
    "/efi",
    "/snap",
    "/var/lib",
    "/var/snap",
    "/nix",
    "/usr/lib",
    "/System",
    "/private",
];

/// Whether a mount-table entry is a place worth listing next to bookmarks:
/// the root, a real disk or partition, removable media or a network share.
#[cfg(unix)]
fn is_browsable_mount(entry: &MountEntry) -> bool {
    let mount_point = entry.mount_point.as_str();
    if mount_point == "/" {
        return true;
    }
    if entry.nobrowse {
        return false;
    }
    let under = |prefix: &str| {
        mount_point == prefix
            || mount_point
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    if HIDDEN_MOUNT_PREFIXES.iter().any(|p| under(p)) && !under("/run/media") {
        return false;
    }
    let device = entry.device.as_str();
    let real_device = device.starts_with("/dev/")
        && !device.starts_with("/dev/loop")
        && !device.starts_with("/dev/ram");
    real_device || BROWSABLE_VIRTUAL_FS.contains(&entry.fs_type.as_str())
}

/// Mount points a user may want to jump to: the root, mounted disks and
/// partitions, removable media and network shares, sorted and deduplicated.
///
/// Windows drives are listed by `termide_vfs::drive_roots`, so this returns
/// nothing there.
#[cfg(unix)]
pub fn browsable_mount_points() -> Vec<std::path::PathBuf> {
    let mut points: Vec<std::path::PathBuf> = read_mounts()
        .iter()
        .filter(|entry| is_browsable_mount(entry))
        .map(|entry| std::path::PathBuf::from(&entry.mount_point))
        .collect();
    points.sort();
    points.dedup();
    points
}

/// Windows drives are listed by `termide_vfs::drive_roots`.
#[cfg(not(unix))]
pub fn browsable_mount_points() -> Vec<std::path::PathBuf> {
    Vec::new()
}

/// `getmntinfo` returns a pointer into per-process static storage that the
/// next call overwrites, so callers are serialized and copy out under the lock.
#[cfg(target_os = "macos")]
static MNT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "macos")]
fn read_mounts() -> Vec<MountEntry> {
    let _guard = MNT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut buf: *mut libc::statfs = std::ptr::null_mut();

    // SAFETY: `getmntinfo` points `buf` at a kernel-filled array of `count`
    // `statfs` structs and returns that count (0 on failure). The storage is
    // owned by libc and must not be freed; it stays valid until the next call,
    // which `MNT_LOCK` prevents while the slice below is alive.
    let count = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if count <= 0 || buf.is_null() {
        return Vec::new();
    }

    // SAFETY: `count` entries were just reported as written to `buf`.
    let entries = unsafe { std::slice::from_raw_parts(buf, count as usize) };

    entries
        .iter()
        .filter_map(|fs| {
            Some(MountEntry {
                device: c_chars_to_string(&fs.f_mntfromname)?,
                mount_point: c_chars_to_string(&fs.f_mntonname)?,
                fs_type: c_chars_to_string(&fs.f_fstypename).unwrap_or_default(),
                nobrowse: fs.f_flags & libc::MNT_DONTBROWSE as u32 != 0,
            })
        })
        .collect()
}

/// No known way to enumerate mounts on this platform; callers degrade to an
/// empty table rather than reporting wrong devices.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn read_mounts() -> Vec<MountEntry> {
    Vec::new()
}

/// Decode a fixed-size, NUL-padded C string field.
#[cfg(target_os = "macos")]
fn c_chars_to_string(field: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Whole-disk device behind an APFS volume BSD name.
///
/// macOS puts several volumes (`/`, `/System/Volumes/Data`, `Recovery`, …)
/// inside one APFS container. They share a single space pool and `statvfs`
/// reports identical numbers for every one of them, so the container device is
/// what those numbers actually describe: `/dev/disk3s1s1` -> `/dev/disk3`.
///
/// Returns `None` for names that carry no volume suffix (already a whole disk)
/// and for anything that is not a `disk<N>…` BSD name, so Linux partition
/// names (`/dev/nvme0n1p2`) stay untouched.
#[cfg(unix)]
fn apfs_container_device(device: &str) -> Option<&str> {
    let name_start = device.rfind('/')? + 1;
    let name = &device[name_start..];
    let after_prefix = name.strip_prefix("disk")?;
    let digits = after_prefix.bytes().take_while(u8::is_ascii_digit).count();
    // No digits, or nothing but digits: not a volume slice.
    if digits == 0 || digits == after_prefix.len() {
        return None;
    }
    Some(&device[..name_start + "disk".len() + digits])
}

/// Space-pool identity for a mount: the APFS container for `apfs` volumes,
/// the device itself otherwise. Two mounts sharing a key report the same free
/// and total space and should be listed once.
#[cfg(unix)]
fn space_pool(device: &str, fs_type: &str) -> String {
    if fs_type == "apfs" {
        if let Some(container) = apfs_container_device(device) {
            return container.to_string();
        }
    }
    device.to_string()
}

/// Resolve a mount-table device: follow symlinks (e.g.
/// `/dev/disk/by-uuid/...` -> `/dev/nvme0n1p2`) and map device-mapper targets
/// to their physical partition.
#[cfg(unix)]
fn resolve_device_path(device: &str) -> String {
    let resolved = dunce::canonicalize(device)
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| device.to_string());

    if resolved.contains("/dm-") {
        resolve_dm_device(&resolved).unwrap_or(resolved)
    } else {
        resolved
    }
}

/// Space pool of a mount table entry, with the device fully resolved.
#[cfg(unix)]
fn pool_for_entry(entry: &MountEntry) -> String {
    space_pool(&resolve_device_path(&entry.device), &entry.fs_type)
}

/// Backing device of a mount point, collapsed to its APFS container, together
/// with the filesystem type.
#[cfg(unix)]
pub(crate) struct MountInfo {
    pub device: String,
    pub fs_type: Option<String>,
}

/// Resolve the mount backing a path by longest matching mount point.
///
/// Devices are collapsed to their APFS container so the reported device matches
/// the free/total space `statvfs` returns: every volume of a container shares
/// one space pool and reports identical numbers.
#[cfg(unix)]
pub(crate) fn resolve_mount_for_path(path: &Path) -> Option<MountInfo> {
    // Canonicalized once: it does not change across mount entries, and the
    // syscall used to run per entry.
    let canonical_path = dunce::canonicalize(path).ok()?;
    let mounts = read_mounts();

    let mut best_match: Option<(usize, usize)> = None; // (mount index, mount path len)

    for (idx, entry) in mounts.iter().enumerate() {
        // Check if this mount point is a prefix of our path
        if let Ok(canonical_mount) = dunce::canonicalize(&entry.mount_point) {
            if canonical_path.starts_with(&canonical_mount) {
                let mount_len = canonical_mount.as_os_str().len();
                // Keep track of the longest matching mount point
                if best_match.as_ref().is_none_or(|(_, len)| mount_len > *len) {
                    best_match = Some((idx, mount_len));
                }
            }
        }
    }

    let entry = &mounts[best_match?.0];

    Some(MountInfo {
        device: pool_for_entry(entry),
        fs_type: (!entry.fs_type.is_empty()).then(|| entry.fs_type.clone()),
    })
}

/// Query a path with `statvfs` and return `(available, total)` in bytes.
///
/// `f_blocks` and `f_bavail` count *fundamental* blocks, so they must be
/// scaled by `f_frsize`. `f_bsize` is the optimal I/O size and can be much
/// larger: on macOS/APFS it is 1 MiB while `f_frsize` is 4 KiB, so scaling by
/// `f_bsize` inflates every figure 256x (a 1.8 TB volume reports 464 TB).
/// Some platforms leave `f_frsize` unset, hence the `f_bsize` fallback.
///
/// The `statvfs` field types differ between platforms (`u64` on 64-bit Linux,
/// `u32` block counts on macOS), so the `as u64` casts are needed on some and
/// redundant on others.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub(crate) fn statvfs_bytes(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path_cstr = CString::new(path.as_os_str().as_bytes()).ok()?;

    // SAFETY: `statvfs` is POSIX; it fills the zero-initialized struct and
    // returns 0 on success, at which point the fields are valid. `path_cstr`
    // is a valid NUL-terminated path built above.
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path_cstr.as_ptr(), &mut stat) != 0 {
            return None;
        }
        stat
    };

    statvfs_bytes_of(&stat)
}

/// Scale the block counts of an already-taken `statvfs` snapshot to bytes.
///
/// Split from [`statvfs_bytes`] so the scaling can be tested against a single
/// snapshot: two `statvfs` calls cannot be compared byte for byte, because
/// every other writer on the volume moves `f_bavail` between them.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn statvfs_bytes_of(stat: &libc::statvfs) -> Option<(u64, u64)> {
    let block_size = {
        let frsize = stat.f_frsize as u64;
        if frsize == 0 {
            stat.f_bsize as u64
        } else {
            frsize
        }
    };
    if block_size == 0 {
        return None;
    }

    let available = (stat.f_bavail as u64).checked_mul(block_size)?;
    let total = (stat.f_blocks as u64).checked_mul(block_size)?;

    Some((available, total))
}

/// Get disk space information for a given path.
///
/// Returns `DiskSpaceInfo` with device name, filesystem type, available and
/// total space.
#[cfg(unix)]
pub fn get_disk_space_info(path: &Path) -> Option<DiskSpaceInfo> {
    // Get device name and filesystem type for this path
    let mount = resolve_mount_for_path(path);

    let (available, total) = statvfs_bytes(path)?;

    Some(DiskSpaceInfo {
        device: mount.as_ref().map(|m| m.device.clone()),
        fs_type: mount.and_then(|m| m.fs_type),
        available,
        total,
    })
}

/// Get disk space information for all real mounted devices.
///
/// Reads the system mount table, filters for real devices (`/dev/`), and calls
/// `statvfs` once per *space pool*: every APFS volume of one container reports
/// the container's identical free/total, so those volumes collapse into a single
/// row labelled with the container device. The representative mount of a pool is
/// the root-most (shortest) mount point, ties broken lexicographically.
#[cfg(unix)]
pub fn get_all_disk_space_info() -> Vec<DiskSpaceInfo> {
    // pool device -> (representative mount point, fs type)
    let mut seen_pools: HashMap<String, (String, Option<String>)> = HashMap::new();

    for entry in read_mounts() {
        let device = entry.device.as_str();

        // Only real devices. Filters out Linux's proc/sysfs/cgroup entries and
        // macOS's devfs, `map auto_home` and network shares alike.
        if !device.starts_with("/dev/") {
            continue;
        }

        // Skip pseudo-devices
        if device.starts_with("/dev/loop") || device.starts_with("/dev/ram") {
            continue;
        }

        let pool = pool_for_entry(&entry);
        let fs_type = (!entry.fs_type.is_empty()).then(|| entry.fs_type.clone());

        // Keep the root-most mount point per pool: it is the mount whose
        // numbers the user cares about, and statvfs of any volume in the pool
        // returns the same figures anyway. Ties break lexicographically so the
        // choice does not depend on mount-table order.
        let replace = match seen_pools.get(&pool) {
            None => true,
            Some((mount_point, _)) => match entry.mount_point.len().cmp(&mount_point.len()) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => entry.mount_point < *mount_point,
                std::cmp::Ordering::Greater => false,
            },
        };
        if replace {
            seen_pools.insert(pool, (entry.mount_point.clone(), fs_type));
        }
    }

    let mut result = Vec::new();
    for (device, (mount_point, fs_type)) in &seen_pools {
        if let Some((available, total)) = statvfs_bytes(Path::new(mount_point)) {
            result.push(DiskSpaceInfo {
                device: Some(device.clone()),
                fs_type: fs_type.clone(),
                available,
                total,
            });
        }
    }

    // Sort by device name for consistent ordering
    result.sort_by(|a, b| a.device.cmp(&b.device));
    result
}

#[cfg(windows)]
pub fn get_disk_space_info(path: &std::path::Path) -> Option<DiskSpaceInfo> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let root = path.components().next()?;
    let root_str = format!("{}\\", root.as_os_str().to_string_lossy());

    let wide_path: Vec<u16> = OsStr::new(&root_str)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut free_bytes_available: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut _total_free_bytes: u64 = 0;

    let success = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide_path.as_ptr(),
            &mut free_bytes_available,
            &mut total_bytes,
            &mut _total_free_bytes,
        )
    };

    if success != 0 {
        Some(DiskSpaceInfo {
            device: Some(root_str.trim_end_matches('\\').to_string()),
            fs_type: None,
            available: free_bytes_available,
            total: total_bytes,
        })
    } else {
        None
    }
}

#[cfg(windows)]
pub fn get_all_disk_space_info() -> Vec<DiskSpaceInfo> {
    // Query drives A-Z using GetDiskFreeSpaceExW
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let mut result = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:\\", letter as char);
        let wide_path: Vec<u16> = OsStr::new(&drive)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut free_bytes_available: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut _total_free_bytes: u64 = 0;

        let success = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide_path.as_ptr(),
                &mut free_bytes_available,
                &mut total_bytes,
                &mut _total_free_bytes,
            )
        };

        if success != 0 && total_bytes > 0 {
            result.push(DiskSpaceInfo {
                device: Some(format!("{}:", letter as char)),
                fs_type: None,
                available: free_bytes_available,
                total: total_bytes,
            });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_get_all_disk_space_info() {
        let disks = get_all_disk_space_info();
        // Should find at least one real disk on any Linux system
        assert!(!disks.is_empty());
        for disk in &disks {
            assert!(disk.device.is_some());
            assert!(disk.total > 0);
            // No virtual filesystems
            let dev = disk.device.as_ref().unwrap();
            assert!(dev.starts_with("/dev/"));
            assert!(!dev.starts_with("/dev/loop"));
        }
    }

    /// `statvfs` figures stay physically plausible on a live volume.
    ///
    /// The block-size scaling itself is pinned exactly by
    /// [`test_statvfs_bytes_scales_by_frsize`] and
    /// [`test_statvfs_bytes_falls_back_to_bsize`], which need no volume.
    /// This one only checks the shape of the answer, because it cannot check
    /// the numbers: comparing against a second `statvfs` call races with every
    /// other writer on the volume. Measured on macOS under load, `f_bavail`
    /// moved up to 3072 blocks (12 MiB) between two back-to-back calls, so no
    /// tolerance small enough to mean anything would have held.
    ///
    /// The plausibility bound still catches the original bug: scaling by
    /// `f_bsize` (1 MiB) instead of `f_frsize` (4 KiB) inflates a 606 GiB
    /// volume to 151 TiB, far past the 64 TiB ceiling below.
    #[cfg(unix)]
    #[test]
    fn test_statvfs_bytes_is_plausible_on_a_live_volume() {
        let path = std::path::Path::new("/");
        let (available, total) = statvfs_bytes(path).expect("statvfs on /");

        assert!(
            total < 64 * 1024u64.pow(4),
            "total {total} bytes is beyond any plausible single volume"
        );
        assert!(
            available > 0 && available < total,
            "available {available} outside 0..{total}"
        );
    }

    /// The scaling itself, pinned on a synthetic snapshot so it holds on every
    /// platform and cannot race a live volume.
    ///
    /// The original bug was scaling by `f_bsize` where `f_frsize` was the
    /// fundamental size. On macOS those differ by 256x, but on a platform
    /// where they are equal the mistake is invisible — so a test that only
    /// reads a real volume does not always catch it. Here the two fields are
    /// deliberately different, on every platform.
    #[cfg(unix)]
    // Mirrors the casts in `statvfs_bytes_of`, which explains why they stay.
    #[allow(clippy::unnecessary_cast)]
    #[test]
    fn test_statvfs_bytes_scales_by_frsize() {
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        stat.f_bsize = 1024 * 1024; // optimal I/O size, deliberately larger
        stat.f_frsize = 4096; // fundamental block size
        stat.f_blocks = 1_000_000;
        stat.f_bavail = 250_000;

        let (available, total) = statvfs_bytes_of(&stat).expect("scaling");
        assert_eq!(total, 1_000_000 * 4096);
        assert_eq!(available, 250_000 * 4096);
        assert_ne!(
            total,
            stat.f_blocks as u64 * stat.f_bsize as u64,
            "total was scaled by f_bsize, which inflates the volume"
        );
    }

    /// A snapshot taken off a real volume scales exactly, with no second call
    /// to race against: the counts and the bytes come from the same one.
    #[cfg(unix)]
    // Mirrors the casts in `statvfs_bytes_of`, which explains why they stay.
    #[allow(clippy::unnecessary_cast)]
    #[test]
    fn test_statvfs_bytes_of_a_live_snapshot_is_exact() {
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let c_path = std::ffi::CString::new("/").unwrap();
        assert_eq!(unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) }, 0);

        let (available, total) = statvfs_bytes_of(&stat).expect("scaling");
        let block = {
            let frsize = stat.f_frsize as u64;
            if frsize == 0 {
                stat.f_bsize as u64
            } else {
                frsize
            }
        };

        assert_eq!(total, stat.f_blocks as u64 * block);
        assert_eq!(available, stat.f_bavail as u64 * block);
    }

    /// Some platforms leave `f_frsize` unset; the counts then scale by
    /// `f_bsize`. When neither is set there is nothing to scale by.
    #[cfg(unix)]
    #[test]
    fn test_statvfs_bytes_falls_back_to_bsize() {
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        stat.f_bsize = 8192;
        stat.f_frsize = 0;
        stat.f_blocks = 100;
        stat.f_bavail = 50;

        assert_eq!(statvfs_bytes_of(&stat), Some((50 * 8192, 100 * 8192)));

        stat.f_bsize = 0;
        assert_eq!(statvfs_bytes_of(&stat), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_statvfs_bytes_missing_path() {
        assert!(statvfs_bytes(std::path::Path::new("/nonexistent-volume-path")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_apfs_container_device_strips_volume_suffix() {
        assert_eq!(apfs_container_device("/dev/disk3s1s1"), Some("/dev/disk3"));
        assert_eq!(apfs_container_device("/dev/disk3s5"), Some("/dev/disk3"));
        assert_eq!(
            apfs_container_device("/dev/disk12s2s1"),
            Some("/dev/disk12")
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_apfs_container_device_leaves_non_volumes_alone() {
        // Whole disks and Linux-style partition names must not be rewritten.
        assert_eq!(apfs_container_device("/dev/disk3"), None);
        assert_eq!(apfs_container_device("/dev/nvme0n1p2"), None);
        assert_eq!(apfs_container_device("/dev/sda1"), None);
        assert_eq!(apfs_container_device("disk3s5"), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_space_pool_collapses_apfs_only() {
        assert_eq!(space_pool("/dev/disk3s5", "apfs"), "/dev/disk3");
        // Non-APFS volumes are separate filesystems with their own numbers.
        assert_eq!(space_pool("/dev/disk4s2", "hfs"), "/dev/disk4s2");
        assert_eq!(space_pool("/dev/nvme0n1p2", "ext4"), "/dev/nvme0n1p2");
    }

    /// Volumes of one APFS container share a single space pool, so the listing
    /// must report each pool once.
    #[cfg(unix)]
    #[test]
    fn test_all_disk_rows_are_unique_per_space_pool() {
        let disks = get_all_disk_space_info();
        let mut seen = std::collections::HashSet::new();
        for disk in &disks {
            let dev = disk.device.clone().unwrap_or_default();
            assert!(
                seen.insert(dev.clone()),
                "duplicate row for space pool {dev}"
            );
        }
        // Every row is a pool that is actually in the mount table.
        let pools: std::collections::HashSet<String> = read_mounts()
            .iter()
            .filter(|e| e.device.starts_with("/dev/"))
            .map(pool_for_entry)
            .collect();
        for dev in seen {
            assert!(pools.contains(&dev), "{dev} is not a mounted pool");
        }
    }

    #[cfg(unix)]
    fn mount(device: &str, mount_point: &str, fs_type: &str) -> MountEntry {
        MountEntry {
            device: device.into(),
            mount_point: mount_point.into(),
            fs_type: fs_type.into(),
            nobrowse: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn browsable_mounts_keep_disks_media_and_shares() {
        for entry in [
            mount("/dev/nvme0n1p2", "/", "ext4"),
            mount("/dev/nvme0n1p3", "/home", "ext4"),
            mount("/dev/sdb1", "/run/media/me/USB", "vfat"),
            mount("/dev/sdc1", "/media/me/Backup", "exfat"),
            mount("/dev/disk4s1", "/Volumes/Photos", "apfs"),
            mount("//me@nas/share", "/Volumes/share", "smbfs"),
            mount("nas:/export", "/mnt/nas", "nfs4"),
            mount("me@host:/srv", "/home/me/remote", "fuse.sshfs"),
            mount("C:\\", "/mnt/c", "drvfs"),
        ] {
            assert!(is_browsable_mount(&entry), "{} hidden", entry.mount_point);
        }
    }

    #[cfg(unix)]
    #[test]
    fn browsable_mounts_skip_system_plumbing() {
        for entry in [
            mount("proc", "/proc", "proc"),
            mount("tmpfs", "/tmp", "tmpfs"),
            mount("/dev/nvme0n1p1", "/boot/efi", "vfat"),
            mount("/dev/loop3", "/snap/core/1", "squashfs"),
            mount("/dev/nvme0n1p2", "/nix/store", "ext4"),
            mount("/dev/sda1", "/run/user/1000/doc", "fuse.portal"),
            mount("/dev/disk3s5", "/System/Volumes/Data", "apfs"),
            mount("devfs", "/dev", "devfs"),
            mount("map auto_home", "/System/Volumes/Data/home", "autofs"),
            mount("overlay", "/var/lib/docker/overlay2/x/merged", "overlay"),
            mount("drivers", "/usr/lib/wsl/drivers", "drvfs"),
        ] {
            assert!(!is_browsable_mount(&entry), "{} listed", entry.mount_point);
        }
        let recovery = MountEntry {
            nobrowse: true,
            ..mount("/dev/disk3s3", "/Volumes/Recovery", "apfs")
        };
        assert!(!is_browsable_mount(&recovery));
    }

    #[cfg(unix)]
    #[test]
    fn hidden_prefix_matches_whole_components_only() {
        // `/runner` is not under `/run`, `/system-backup` is not `/System`.
        assert!(is_browsable_mount(&mount("/dev/sdd1", "/runner", "ext4")));
        assert!(is_browsable_mount(&mount("/dev/sdd2", "/devdata", "ext4")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_mounts_escapes_are_decoded() {
        assert_eq!(
            unescape_mount_field("/media/me/USB\\040DISK"),
            "/media/me/USB DISK"
        );
        assert_eq!(unescape_mount_field("/a\\011b\\134c"), "/a\tb\\c");
        assert_eq!(unescape_mount_field("/plain"), "/plain");
        // A lone or short backslash sequence is kept literally.
        assert_eq!(unescape_mount_field("/x\\04"), "/x\\04");
    }

    #[test]
    fn browsable_mount_points_include_root_on_unix() {
        let points = browsable_mount_points();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(
                points.iter().any(|p| p == std::path::Path::new("/")),
                "{points:?}"
            );
        }
    }
}

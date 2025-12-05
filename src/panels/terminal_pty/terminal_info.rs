use crate::panels::file_manager::DiskSpaceInfo;

/// Terminal information for status bar
pub struct TerminalInfo {
    pub user_host: String,                 // user@host
    pub cwd: String,                       // current directory
    pub disk_space: Option<DiskSpaceInfo>, // disk information
}

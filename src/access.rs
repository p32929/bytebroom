//! Full Disk Access: macOS cannot ask for it with a popup, only send people to the settings pane.
//! Without it, macOS asks separately for each protected folder the scan touches. So we check up
//! front and, if it is missing, explain it once before scanning.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

const SETTINGS_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// The system privacy database: it exists on every Mac and only a process with Full Disk Access
/// may open it. Opening it triggers no prompt. Anything but "permission denied" counts as
/// granted, so an unexpected error never nags.
pub fn has_full_disk_access() -> bool {
    match std::fs::File::open("/Library/Application Support/com.apple.TCC/TCC.db") {
        Err(e) => e.kind() != ErrorKind::PermissionDenied,
        Ok(_) => true,
    }
}

pub fn open_settings() {
    let _ = Command::new("open").arg(SETTINGS_PANE).spawn();
}

/// The .app this program runs from, if it was started as an app and not with `cargo run`.
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(PathBuf::from)
}

pub fn can_relaunch() -> bool {
    bundle().is_some()
}

/// Opens a fresh copy once this one has quit (macOS applies a new permission only to new processes).
pub fn relaunch() {
    if let Some(app) = bundle() {
        let _ = Command::new("sh")
            .arg("-c")
            .arg(r#"sleep 1; open -n "$0""#)
            .arg(app)
            .spawn();
    }
}

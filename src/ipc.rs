//! IPC: write workspace state for waybar's custom module.
//!
//! Creates a FIFO at $XDG_RUNTIME_DIR/notion-river-workspaces.
//! Waybar reads it as a streaming exec module for instant updates.
//! Also writes to a regular file as fallback for scripts.

use std::io::Write;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::PathBuf;

use crate::workspace::WorkspaceManager;

fn fifo_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river-workspaces")
}

/// State for the IPC writer.
#[derive(Debug)]
pub struct IpcState {
    fifo: PathBuf,
    fifo_fd: Option<std::os::fd::OwnedFd>,
    last_json: String,
}

impl IpcState {
    pub fn new() -> Self {
        let fifo = fifo_path();

        // Remove existing file/fifo and create fresh FIFO
        let _ = std::fs::remove_file(&fifo);
        let c_path =
            std::ffi::CString::new(fifo.to_str().unwrap_or("/tmp/notion-river-workspaces"))
                .unwrap();
        unsafe {
            libc::mkfifo(c_path.as_ptr(), 0o644);
        }

        Self {
            fifo,
            fifo_fd: None,
            last_json: String::new(),
        }
    }

    /// Write workspace state. Only writes if state changed.
    pub fn update(&mut self, workspaces: &WorkspaceManager) {
        let json = workspace_json(workspaces);
        if json == self.last_json {
            return;
        }
        self.last_json = json.clone();

        // Try to open the FIFO non-blocking (fails if no reader — that's fine)
        if self.fifo_fd.is_none() {
            let c_path = std::ffi::CString::new(self.fifo.to_str().unwrap_or("")).unwrap();
            let fd = unsafe {
                libc::open(
                    c_path.as_ptr(),
                    libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd >= 0 {
                self.fifo_fd = Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
            }
        }

        // Write to FIFO if open
        if let Some(ref fd) = self.fifo_fd {
            let line = format!("{json}\n");
            let ret = unsafe {
                libc::write(
                    fd.as_raw_fd(),
                    line.as_ptr() as *const libc::c_void,
                    line.len(),
                )
            };
            if ret < 0 {
                // Reader disconnected — close and retry next time
                self.fifo_fd = None;
            }
        }
    }
}

impl Drop for IpcState {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.fifo);
    }
}

/// Generate waybar JSON for workspace state.
/// Groups workspaces by output with a │ separator between monitors.
pub fn workspace_json(workspaces: &WorkspaceManager) -> String {
    // Group workspaces by their preferred output
    let mut output_groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    for ws in &workspaces.workspaces {
        let is_focused = ws.id == workspaces.focused_workspace;
        let is_visible = ws.active_output.is_some();
        let has_windows = ws
            .root
            .all_frame_ids()
            .iter()
            .any(|fid| ws.root.find_frame(*fid).is_some_and(|f| !f.is_empty()));

        let marker = if is_focused {
            "▶"
        } else if is_visible {
            "●"
        } else if has_windows {
            "○"
        } else {
            "·"
        };

        let output_name = ws
            .preferred_output
            .as_deref()
            .unwrap_or("none")
            .to_string();
        output_groups
            .entry(output_name)
            .or_default()
            .push(format!("{marker} {}", ws.name));
    }

    let text = output_groups
        .values()
        .map(|group| group.join("  "))
        .collect::<Vec<_>>()
        .join("  │  ");

    let focused_name = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .map(|ws| ws.name.as_str())
        .unwrap_or("");

    format!(
        r#"{{"text": "{text}", "tooltip": "Focused: {focused_name}", "class": "workspaces"}}"#
    )
}

    let text = parts.join("  ");
    let focused_name = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .map(|ws| ws.name.as_str())
        .unwrap_or("");

    format!(r#"{{"text": "{text}", "tooltip": "Focused: {focused_name}", "class": "workspaces"}}"#)
}

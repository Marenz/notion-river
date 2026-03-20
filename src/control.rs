use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::ipc::Subscriber;

use serde::Serialize;

fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river.sock")
}

#[derive(Debug, Clone)]
pub enum ControlRequest {
    FocusWindow(u64),
    SwitchWorkspace(String),
    SetFixedDimensions(String, Option<(i32, i32)>),
    Bind {
        app_id: String,
        workspace: String,
        frame_index: usize,
        dimensions: Option<(i32, i32)>,
    },
    Unbind(String),
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WindowInfo {
    pub id: u64,
    pub workspace: String,
    pub frame_id: u64,
    pub title: String,
    pub app_id: String,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WorkspaceInfo {
    pub name: String,
    pub output: Option<String>,
    pub focused: bool,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Snapshot {
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
}

#[derive(Debug, Clone)]
pub struct ControlState {
    pub path: PathBuf,
    pub pending: Arc<Mutex<Vec<ControlRequest>>>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    /// Write end of a self-pipe. Written to when a command arrives,
    /// read by the main loop to trigger manage_dirty.
    pub notify_fd: std::os::fd::RawFd,
}

impl ControlState {
    pub fn new(subscribers: Arc<Mutex<Vec<Subscriber>>>) -> Self {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);

        let pending = Arc::new(Mutex::new(Vec::new()));
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));

        // Self-pipe for signaling the main event loop
        let mut fds = [0i32; 2];
        unsafe { libc::pipe(fds.as_mut_ptr()) };
        let read_fd = fds[0];
        let write_fd = fds[1];
        // Make read end non-blocking
        unsafe { libc::fcntl(read_fd, libc::F_SETFL, libc::O_NONBLOCK) };

        let pending_thread = Arc::clone(&pending);
        let snapshot_thread = Arc::clone(&snapshot);
        let subscribers_thread = Arc::clone(&subscribers);
        let path_thread = path.clone();

        std::thread::spawn(move || {
            let listener = match UnixListener::bind(&path_thread) {
                Ok(l) => l,
                Err(e) => {
                    log::error!(
                        "Failed to bind control socket {}: {e}",
                        path_thread.display()
                    );
                    return;
                }
            };

            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Control socket accept failed: {e}");
                        continue;
                    }
                };
                handle_client(
                    stream,
                    &pending_thread,
                    &snapshot_thread,
                    &subscribers_thread,
                );
                // Signal the main loop to trigger a manage cycle
                unsafe { libc::write(write_fd, b"x".as_ptr() as _, 1) };
            }
        });

        Self {
            path,
            pending,
            snapshot,
            notify_fd: read_fd,
        }
    }

    /// Drain the notification pipe (call after checking for pending requests).
    pub fn drain_notify(&self) {
        let mut buf = [0u8; 64];
        unsafe { libc::read(self.notify_fd, buf.as_mut_ptr() as _, buf.len()) };
    }

    pub fn take_pending(&self) -> Vec<ControlRequest> {
        let mut guard = self.pending.lock().expect("control pending poisoned");
        std::mem::take(&mut *guard)
    }

    pub fn update_snapshot(&self, snapshot: Snapshot) {
        let mut guard = self.snapshot.lock().expect("control snapshot poisoned");
        *guard = snapshot;
    }
}

impl Drop for ControlState {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn handle_client(
    stream: UnixStream,
    pending: &Arc<Mutex<Vec<ControlRequest>>>,
    snapshot: &Arc<Mutex<Snapshot>>,
    subscribers: &Arc<Mutex<Vec<Subscriber>>>,
) {
    // Try line-based read first (for subscribe-workspaces), fall back to
    // read-to-EOF for legacy clients that shutdown(Write) before we read.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(100)));

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    match reader.read_line(&mut buf) {
        Ok(0) => {
            let _ = reader.get_mut().write_all(b"ERR empty\n");
            return;
        }
        Ok(_) => {} // Got a line
        Err(_) => {
            // Timeout or error — legacy client that shuts down write end.
            let _ = reader.read_to_string(&mut buf);
        }
    }

    let line = buf.trim().to_string();
    if line.is_empty() {
        let _ = reader.get_mut().write_all(b"ERR empty\n");
        return;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("");

    // Handle subscribe commands: keep connection open for streaming.
    if cmd == "subscribe-workspaces" || cmd == "subscribe-workspace" {
        let mut stream = reader.into_inner();
        let _ = stream.set_read_timeout(None);

        let kind = if cmd == "subscribe-workspace" {
            let ws_name = parts.collect::<Vec<_>>().join(" ");
            if ws_name.is_empty() {
                let _ = stream.write_all(b"ERR missing workspace name\n");
                return;
            }
            // Send initial state for this workspace.
            let snap = snapshot.lock().expect("control snapshot poisoned").clone();
            let initial = single_ws_json_from_snapshot(&snap, &ws_name);
            if stream.write_all(format!("{initial}\n").as_bytes()).is_err() {
                return;
            }
            crate::ipc::SubscriptionKind::SingleWorkspace(ws_name)
        } else {
            // Send current full state.
            let ipc_path = std::env::var("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
                .join("notion-river-workspaces");
            if let Ok(current) = std::fs::read_to_string(&ipc_path)
                && stream.write_all(current.as_bytes()).is_err()
            {
                return;
            }
            crate::ipc::SubscriptionKind::AllWorkspaces
        };

        subscribers.lock().unwrap().push(Subscriber {
            stream,
            kind,
            last_json: String::new(),
        });
        log::info!("New workspace subscriber connected");
        return;
    }

    // All other commands: extract stream for response writing.
    let mut stream = reader.into_inner();
    let _ = stream.set_read_timeout(None);

    match cmd {
        "list-windows" => {
            let snap = snapshot.lock().expect("control snapshot poisoned").clone();
            match serde_json::to_string(&snap.windows) {
                Ok(json) => {
                    let _ = stream.write_all(json.as_bytes());
                    let _ = stream.write_all(b"\n");
                }
                Err(_) => {
                    let _ = stream.write_all(b"ERR serialize\n");
                }
            }
        }
        "list-workspaces" => {
            let snap = snapshot.lock().expect("control snapshot poisoned").clone();
            match serde_json::to_string(&snap.workspaces) {
                Ok(json) => {
                    let _ = stream.write_all(json.as_bytes());
                    let _ = stream.write_all(b"\n");
                }
                Err(_) => {
                    let _ = stream.write_all(b"ERR serialize\n");
                }
            }
        }
        "focus-window" => {
            let Some(id_str) = parts.next() else {
                let _ = stream.write_all(b"ERR missing id\n");
                return;
            };
            match id_str.parse::<u64>() {
                Ok(id) => {
                    pending
                        .lock()
                        .expect("control pending poisoned")
                        .push(ControlRequest::FocusWindow(id));
                    let _ = stream.write_all(b"OK\n");
                }
                Err(_) => {
                    let _ = stream.write_all(b"ERR bad id\n");
                }
            }
        }
        "switch-workspace" => {
            let name = parts.collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                let _ = stream.write_all(b"ERR missing name\n");
                return;
            }
            pending
                .lock()
                .expect("control pending poisoned")
                .push(ControlRequest::SwitchWorkspace(name));
            let _ = stream.write_all(b"OK\n");
        }
        "bind" => {
            // Usage: bind <app_id> <workspace> <frame_index> [WxH]
            let Some(app_id) = parts.next() else {
                let _ =
                    stream.write_all(b"ERR usage: bind <app_id> <workspace> <frame_index> [WxH]\n");
                return;
            };
            let Some(workspace) = parts.next() else {
                let _ = stream.write_all(b"ERR missing workspace\n");
                return;
            };
            let Some(frame_str) = parts.next() else {
                let _ = stream.write_all(b"ERR missing frame_index\n");
                return;
            };
            let frame_index = match frame_str.parse::<usize>() {
                Ok(i) => i,
                Err(_) => {
                    let _ = stream.write_all(b"ERR bad frame_index\n");
                    return;
                }
            };
            let dimensions = parts.next().and_then(|d| {
                let p: Vec<&str> = d.split('x').collect();
                if p.len() == 2 {
                    Some((p[0].parse::<i32>().ok()?, p[1].parse::<i32>().ok()?))
                } else {
                    None
                }
            });
            pending
                .lock()
                .expect("control pending poisoned")
                .push(ControlRequest::Bind {
                    app_id: app_id.to_string(),
                    workspace: workspace.to_string(),
                    frame_index,
                    dimensions,
                });
            let _ = stream.write_all(b"OK\n");
        }
        "unbind" => {
            let Some(app_id) = parts.next() else {
                let _ = stream.write_all(b"ERR missing app_id\n");
                return;
            };
            pending
                .lock()
                .expect("control pending poisoned")
                .push(ControlRequest::Unbind(app_id.to_string()));
            let _ = stream.write_all(b"OK\n");
        }
        "set-fixed-dimensions" => {
            // Usage: set-fixed-dimensions <app_id> <width>x<height>
            // Or:    set-fixed-dimensions <app_id> clear
            let Some(app_id) = parts.next() else {
                let _ = stream.write_all(b"ERR missing app_id\n");
                return;
            };
            let Some(dims_str) = parts.next() else {
                let _ = stream.write_all(b"ERR missing dimensions (WxH or 'clear')\n");
                return;
            };
            let dims = if dims_str == "clear" {
                None
            } else {
                let parts: Vec<&str> = dims_str.split('x').collect();
                if parts.len() != 2 {
                    let _ = stream.write_all(b"ERR bad format, use WxH\n");
                    return;
                }
                match (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
                    (Ok(w), Ok(h)) => Some((w, h)),
                    _ => {
                        let _ = stream.write_all(b"ERR bad dimensions\n");
                        return;
                    }
                }
            };
            pending
                .lock()
                .expect("control pending poisoned")
                .push(ControlRequest::SetFixedDimensions(app_id.to_string(), dims));
            let _ = stream.write_all(b"OK\n");
        }
        _ => {
            let _ = stream.write_all(b"ERR unknown\n");
        }
    }
}

/// Generate waybar JSON for a single workspace from the control snapshot.
/// Used for initial state when a subscriber connects (before the main thread runs).
fn single_ws_json_from_snapshot(snap: &Snapshot, name: &str) -> String {
    for ws in &snap.workspaces {
        if ws.name != name {
            continue;
        }
        let cls = if ws.focused {
            "focused"
        } else if ws.visible {
            "visible"
        } else {
            "hidden"
        };
        let output = ws.output.as_deref().unwrap_or("?");
        return format!(
            r#"{{"text": "{name}", "tooltip": "{name} on {output}", "class": "{cls}"}}"#
        );
    }
    format!(r#"{{"text": "{name}", "class": "empty"}}"#)
}

pub fn build_snapshot(wm: &crate::wm::WindowManager) -> Snapshot {
    let mut windows = Vec::new();
    let focused_ws = wm.workspaces.focused_workspace;
    let focused_frame = wm.workspaces.workspaces[focused_ws.0].focused_frame;

    for ws in &wm.workspaces.workspaces {
        let ws_name = ws.name.clone();
        for frame_id in ws.root.all_frame_ids() {
            if let Some(frame) = ws.root.find_frame(frame_id) {
                for win in &frame.windows {
                    windows.push(WindowInfo {
                        id: win.window_id,
                        workspace: ws_name.clone(),
                        frame_id: frame_id.0,
                        title: win.title.clone(),
                        app_id: win.app_id.clone(),
                        focused: ws.id == focused_ws
                            && frame_id == focused_frame
                            && frame
                                .active_window()
                                .is_some_and(|w| w.window_id == win.window_id),
                    });
                }
            }
        }
    }

    let workspaces = wm
        .workspaces
        .workspaces
        .iter()
        .map(|ws| WorkspaceInfo {
            name: ws.name.clone(),
            output: ws
                .active_output
                .and_then(|oid| wm.workspaces.output(oid))
                .and_then(|o| o.name.clone()),
            focused: ws.id == focused_ws,
            visible: ws.active_output.is_some(),
        })
        .collect();

    Snapshot {
        windows,
        workspaces,
    }
}

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
}

impl ControlState {
    pub fn new() -> Self {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);

        let pending = Arc::new(Mutex::new(Vec::new()));
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));

        let pending_thread = Arc::clone(&pending);
        let snapshot_thread = Arc::clone(&snapshot);
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
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Control socket accept failed: {e}");
                        continue;
                    }
                };
                handle_client(&mut stream, &pending_thread, &snapshot_thread);
            }
        });

        Self {
            path,
            pending,
            snapshot,
        }
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
    stream: &mut UnixStream,
    pending: &Arc<Mutex<Vec<ControlRequest>>>,
    snapshot: &Arc<Mutex<Snapshot>>,
) {
    let mut buf = String::new();
    if stream.read_to_string(&mut buf).is_err() {
        let _ = stream.write_all(b"ERR read\n");
        return;
    }
    let line = buf.trim();
    if line.is_empty() {
        let _ = stream.write_all(b"ERR empty\n");
        return;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("");
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

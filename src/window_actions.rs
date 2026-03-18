//! Action execution for the window manager.
//!
//! Contains `perform_action`, `perform_split`, `perform_unsplit`,
//! `find_cross_monitor_target`, and command spawning.

use crate::actions::Action;
use crate::layout::{FrameId, Orientation};
use crate::protocol::river_window_manager_v1::RiverWindowManagerV1;
use crate::wm::{InputMode, WindowManager};
use crate::workspace::OutputId;

impl WindowManager {
    pub(crate) fn perform_action(
        &mut self,
        action: Action,
        wm_proxy: &RiverWindowManagerV1,
        river_outputs: &std::collections::HashMap<
            u64,
            crate::protocol::river_output_v1::RiverOutputV1,
        >,
    ) {
        if !matches!(action, Action::None) {
            log::info!("Action: {action:?}");
        }
        match action {
            Action::None => {}

            Action::Close => {
                // Close window if frame has one; if frame is empty, unsplit it
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window() {
                        let window_id = win_ref.window_id;
                        if let Some(win) = self.windows.iter().find(|w| w.id == window_id) {
                            win.proxy.close();
                        }
                    } else {
                        // Frame is empty — remove it (unsplit)
                        self.perform_unsplit();
                    }
                }
            }

            Action::ToggleFullscreen => {
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window() {
                        let wid = win_ref.window_id;
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                            if win.fullscreen {
                                win.proxy.exit_fullscreen();
                                win.proxy.inform_not_fullscreen();
                                win.fullscreen = false;
                                log::info!("Exiting fullscreen for window {wid}");
                            } else {
                                // Find the output proxy for the workspace's output
                                let output_proxy =
                                    ws.active_output.and_then(|oid| river_outputs.get(&oid.0));
                                if let Some(output) = output_proxy {
                                    win.proxy.fullscreen(output);
                                    win.proxy.inform_fullscreen();
                                    win.fullscreen = true;
                                    log::info!("Entering fullscreen for window {wid}");
                                }
                            }
                        }
                    }
                }
            }

            Action::ToggleFloat => {
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window() {
                        let wid = win_ref.window_id;
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                            win.floating = !win.floating;
                        }
                    }
                }
            }

            Action::FocusDirection(dir) => {
                let ws_idx = self.workspaces.focused_workspace.0;
                let ws = &self.workspaces.workspaces[ws_idx];
                let frame_id = ws.focused_frame;
                let gap = self.config.general.gap as i32;

                if let Some(output_id) = ws.active_output {
                    if let Some(output) = self.workspaces.output(output_id) {
                        let area = output.usable_rect();
                        if let Some(neighbor) = ws.root.find_neighbor(frame_id, dir, area, gap) {
                            // Neighbor within same workspace
                            log::info!("FocusDirection {dir:?}: {frame_id:?} -> {neighbor:?}");
                            let ws_mut = &mut self.workspaces.workspaces[ws_idx];
                            ws_mut.focused_frame = neighbor;
                        } else {
                            // No neighbor in this workspace — try adjacent monitor
                            let frame_rect = ws
                                .root
                                .calculate_layout(area, gap)
                                .into_iter()
                                .find(|(id, _)| *id == frame_id)
                                .map(|(_, r)| r);

                            if let Some(src_rect) = frame_rect {
                                let target =
                                    self.find_cross_monitor_target(output_id, dir, src_rect, gap);
                                if let Some((target_ws_id, target_frame_id)) = target {
                                    log::info!(
                                        "FocusDirection {dir:?}: cross-monitor to ws {} frame {target_frame_id:?}",
                                        self.workspaces.workspaces[target_ws_id.0].name
                                    );
                                    self.workspaces.workspaces[target_ws_id.0].focused_frame =
                                        target_frame_id;
                                    self.workspaces.focused_workspace = target_ws_id;
                                } else {
                                    log::info!(
                                        "FocusDirection {dir:?}: no neighbor or adjacent monitor"
                                    );
                                }
                            }
                        }
                    }
                }
            }

            Action::FocusNextTab => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                    frame.next_tab();
                }
            }

            Action::FocusPrevTab => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                    frame.prev_tab();
                }
            }

            Action::FocusParent => {
                // TODO: implement focus parent for nested container navigation
            }

            Action::MoveDirection(dir) => {
                let ws_idx = self.workspaces.focused_workspace.0;
                let frame_id = self.workspaces.workspaces[ws_idx].focused_frame;
                let gap = self.config.general.gap as i32;

                // Try same-workspace neighbor first
                let same_ws_neighbor = {
                    let ws = &self.workspaces.workspaces[ws_idx];
                    ws.active_output.and_then(|oid| {
                        self.workspaces.output(oid).and_then(|output| {
                            let area = output.usable_rect();
                            ws.root.find_neighbor(frame_id, dir, area, gap)
                        })
                    })
                };

                if let Some(target_frame_id) = same_ws_neighbor {
                    // Move within same workspace
                    let ws = &mut self.workspaces.workspaces[ws_idx];
                    if let Some(frame) = ws.root.find_frame(frame_id) {
                        if let Some(win_ref) = frame.active_window().cloned() {
                            let wid = win_ref.window_id;
                            if let Some(src) = ws.root.find_frame_mut(frame_id) {
                                src.remove_window(wid);
                            }
                            if let Some(dst) = ws.root.find_frame_mut(target_frame_id) {
                                dst.add_window(win_ref);
                            }
                            if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                                win.frame_id = Some(target_frame_id);
                            }
                            self.workspaces.workspaces[ws_idx].focused_frame = target_frame_id;
                        }
                    }
                } else {
                    // Try cross-monitor move
                    let cross = {
                        let ws = &self.workspaces.workspaces[ws_idx];
                        let output_id = match ws.active_output {
                            Some(oid) => oid,
                            None => return,
                        };
                        let output = match self.workspaces.output(output_id) {
                            Some(o) => o,
                            None => return,
                        };
                        let area = output.usable_rect();
                        let frame_rect = ws
                            .root
                            .calculate_layout(area, gap)
                            .into_iter()
                            .find(|(id, _)| *id == frame_id)
                            .map(|(_, r)| r);
                        frame_rect.and_then(|src_rect| {
                            self.find_cross_monitor_target(output_id, dir, src_rect, gap)
                        })
                    };

                    if let Some((target_ws_id, target_frame_id)) = cross {
                        // Get the window ref from source
                        let win_ref = self.workspaces.workspaces[ws_idx]
                            .root
                            .find_frame(frame_id)
                            .and_then(|f| f.active_window().cloned());

                        if let Some(win_ref) = win_ref {
                            let wid = win_ref.window_id;
                            // Remove from source
                            if let Some(src) = self.workspaces.workspaces[ws_idx]
                                .root
                                .find_frame_mut(frame_id)
                            {
                                src.remove_window(wid);
                            }
                            // Add to target on other monitor
                            if let Some(dst) = self.workspaces.workspaces[target_ws_id.0]
                                .root
                                .find_frame_mut(target_frame_id)
                            {
                                dst.add_window(win_ref);
                            }
                            if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                                win.frame_id = Some(target_frame_id);
                            }
                            // Focus follows the window
                            self.workspaces.workspaces[target_ws_id.0].focused_frame =
                                target_frame_id;
                            self.workspaces.focused_workspace = target_ws_id;
                            log::info!(
                                "Cross-monitor move to ws '{}' frame {:?}",
                                self.workspaces.workspaces[target_ws_id.0].name,
                                target_frame_id
                            );
                        }
                    }
                }
            }

            Action::MoveToWorkspace(name) => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;

                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window().cloned() {
                        let wid = win_ref.window_id;
                        // Remove from current frame
                        if let Some(src) = ws.root.find_frame_mut(frame_id) {
                            src.remove_window(wid);
                        }
                        // Find target workspace and add to its focused frame
                        if let Some(target_ws) = self.workspaces.workspace_by_name_mut(&name) {
                            let target_frame = target_ws.focused_frame;
                            if let Some(dst) = target_ws.root.find_frame_mut(target_frame) {
                                dst.add_window(win_ref);
                            }
                            if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                                win.frame_id = Some(target_frame);
                            }
                        }
                    }
                }
            }

            Action::SplitHorizontal => {
                self.perform_split(Orientation::Horizontal);
            }

            Action::SplitVertical => {
                self.perform_split(Orientation::Vertical);
            }

            Action::Unsplit => {
                self.perform_unsplit();
            }

            Action::ToggleSplit => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if ws.root.toggle_orientation(frame_id) {
                    log::info!("Toggled split orientation for frame {frame_id:?}");
                }
            }

            Action::SwitchWorkspace(name) => {
                self.workspaces.switch_workspace(&name);
            }

            Action::EnterResizeMode => {
                self.mode = InputMode::Resize;
                log::info!("Entering resize mode");
            }

            Action::ExitResizeMode => {
                self.mode = InputMode::Normal;
                log::info!("Exiting resize mode");
            }

            Action::Resize(dir) => {
                let delta = 0.05; // 5% per resize step
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                ws.root.resize_frame(frame_id, dir, delta);
            }

            Action::SpawnTerminal => {
                let cmd = self.config.commands.terminal.clone();
                spawn_command(&[&cmd]);
            }

            Action::SpawnLauncher => {
                let args: Vec<String> = self.config.commands.launcher.clone();
                let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                spawn_command(&refs);
            }

            Action::Spawn(args) => {
                let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                spawn_command(&refs);
            }

            Action::Exit => {
                wm_proxy.exit_session();
            }

            Action::Restart => {
                log::info!("Restarting WM — saving state");
                crate::state::save_state(&self.workspaces, &self.windows);
                std::process::exit(0);
            }

            Action::ReloadConfig => {
                self.config = crate::config::Config::load();
                log::info!("Configuration reloaded");
                // TODO: re-parse bindings and re-register with seats
            }
        }
    }

    /// Find the best frame on an adjacent monitor when focus crosses a monitor boundary.
    /// Returns (WorkspaceId, FrameId) of the target, matching the source position.
    pub(crate) fn find_cross_monitor_target(
        &self,
        current_output: OutputId,
        dir: crate::layout::Direction,
        src_rect: crate::layout::Rect,
        gap: i32,
    ) -> Option<(crate::workspace::WorkspaceId, FrameId)> {
        use crate::layout::Direction;

        let cur = self.workspaces.output(current_output)?;
        let cur_rect = cur.usable_rect();

        // Find the adjacent output in the given direction
        let target_output = self.workspaces.outputs.iter().find(|o| {
            if o.id == current_output || o.removed {
                return false;
            }
            let r = o.usable_rect();
            match dir {
                Direction::Right => {
                    r.x >= cur_rect.x + cur_rect.width - gap
                        && crate::layout::vertical_overlap(cur_rect, r) > 0
                }
                Direction::Left => {
                    r.x + r.width <= cur_rect.x + gap
                        && crate::layout::vertical_overlap(cur_rect, r) > 0
                }
                Direction::Down => {
                    r.y >= cur_rect.y + cur_rect.height - gap
                        && crate::layout::horizontal_overlap(cur_rect, r) > 0
                }
                Direction::Up => {
                    r.y + r.height <= cur_rect.y + gap
                        && crate::layout::horizontal_overlap(cur_rect, r) > 0
                }
            }
        })?;

        let target_oid = target_output.id;
        let target_area = target_output.usable_rect();

        // Find the visible workspace on that output
        let target_ws = self
            .workspaces
            .workspaces
            .iter()
            .find(|ws| ws.active_output == Some(target_oid))?;

        // Calculate frame layouts on the target workspace
        let layouts = target_ws.root.calculate_layout(target_area, gap);

        // Find the frame on the entry edge that best matches our vertical/horizontal position
        let src_center_x = src_rect.x + src_rect.width / 2;
        let src_center_y = src_rect.y + src_rect.height / 2;

        let best_frame = match dir {
            Direction::Right => {
                // Entering from the left edge of target — find leftmost frame matching Y
                layouts
                    .iter()
                    .filter(|(_, r)| r.x <= target_area.x + gap)
                    .min_by_key(|(_, r)| {
                        let frame_center_y = r.y + r.height / 2;
                        (frame_center_y - src_center_y).abs()
                    })
            }
            Direction::Left => {
                // Entering from the right edge — find rightmost frame matching Y
                layouts
                    .iter()
                    .filter(|(_, r)| r.x + r.width >= target_area.x + target_area.width - gap)
                    .min_by_key(|(_, r)| {
                        let frame_center_y = r.y + r.height / 2;
                        (frame_center_y - src_center_y).abs()
                    })
            }
            Direction::Down => {
                // Entering from the top — find topmost frame matching X
                layouts
                    .iter()
                    .filter(|(_, r)| r.y <= target_area.y + gap)
                    .min_by_key(|(_, r)| {
                        let frame_center_x = r.x + r.width / 2;
                        (frame_center_x - src_center_x).abs()
                    })
            }
            Direction::Up => {
                // Entering from the bottom — find bottommost frame matching X
                layouts
                    .iter()
                    .filter(|(_, r)| r.y + r.height >= target_area.y + target_area.height - gap)
                    .min_by_key(|(_, r)| {
                        let frame_center_x = r.x + r.width / 2;
                        (frame_center_x - src_center_x).abs()
                    })
            }
        };

        best_frame.map(|(fid, _)| (target_ws.id, *fid))
    }

    pub(crate) fn perform_split(&mut self, orientation: Orientation) {
        let ratio = self.config.general.default_split_ratio;
        let ws_idx = self.workspaces.focused_workspace.0;
        let frame_id = self.workspaces.workspaces[ws_idx].focused_frame;

        // Check if the frame has multiple windows — if so, move active window to new frame
        let active_win = self.workspaces.workspaces[ws_idx]
            .root
            .find_frame(frame_id)
            .and_then(|f| {
                if f.windows.len() > 1 {
                    f.active_window().cloned()
                } else {
                    None
                }
            });

        if let Some(new_id) =
            self.workspaces.workspaces[ws_idx]
                .root
                .split_frame(frame_id, orientation, ratio)
        {
            let dir = if orientation == Orientation::Horizontal {
                "horizontally"
            } else {
                "vertically"
            };
            log::info!("Split frame {frame_id:?} {dir}, new frame {new_id:?}");

            // Move active window to the new frame
            if let Some(win_ref) = active_win {
                let wid = win_ref.window_id;
                if let Some(src) = self.workspaces.workspaces[ws_idx]
                    .root
                    .find_frame_mut(frame_id)
                {
                    src.remove_window(wid);
                }
                if let Some(dst) = self.workspaces.workspaces[ws_idx]
                    .root
                    .find_frame_mut(new_id)
                {
                    dst.add_window(win_ref);
                }
                if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                    win.frame_id = Some(new_id);
                }
                // Focus follows the window to the new frame
                self.workspaces.workspaces[ws_idx].focused_frame = new_id;
            }
        }
    }

    pub(crate) fn perform_unsplit(&mut self) {
        let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
        let frame_id = ws.focused_frame;

        // Only unsplit if frame is empty
        if let Some(frame) = ws.root.find_frame(frame_id) {
            if !frame.is_empty() {
                log::info!("Cannot unsplit non-empty frame");
                return;
            }
        }

        // Get all frame IDs before removal to find a new focus target
        let all_ids = ws.root.all_frame_ids();
        if all_ids.len() <= 1 {
            log::info!("Cannot unsplit the last frame");
            return;
        }

        if ws.root.remove_frame(frame_id) {
            // Focus the first remaining frame
            ws.focused_frame = ws.root.first_frame_id();
            log::info!("Removed frame {frame_id:?}");
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn spawn_command(args: &[&str]) {
    if args.is_empty() {
        return;
    }
    match std::process::Command::new(args[0]).args(&args[1..]).spawn() {
        Ok(_) => log::info!("Spawned: {}", args.join(" ")),
        Err(e) => log::error!("Failed to spawn {}: {e}", args[0]),
    }
}

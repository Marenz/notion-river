//! Focus-follows-mouse logic, extracted for testability.
//!
//! This module contains the pure logic for determining which frame should
//! be focused based on pointer position and hovered window state.

use crate::layout::FrameId;
use crate::workspace::{WorkspaceId, WorkspaceManager};

/// Input for focus-follows-mouse decision.
pub struct FocusInput {
    /// Window ID the pointer is currently hovering (from PointerEnter).
    pub hovered_window_id: Option<u64>,
    /// Absolute pointer position (from pointer_position event).
    pub pointer_x: i32,
    pub pointer_y: i32,
}

/// Result of focus-follows-mouse decision.
#[derive(Debug, PartialEq, Eq)]
pub struct FocusResult {
    pub workspace: WorkspaceId,
    pub frame: FrameId,
}

/// Determine which frame should be focused based on pointer state.
///
/// Returns Some(FocusResult) if focus should change, None if it should stay.
pub fn compute_focus(
    input: &FocusInput,
    workspaces: &WorkspaceManager,
    gap: i32,
    margin: i32,
) -> Option<FocusResult> {
    // Method 1: If pointer is hovering a window, focus that window's frame
    if let Some(hovered_id) = input.hovered_window_id {
        for ws in &workspaces.workspaces {
            if ws.active_output.is_none() {
                continue;
            }
            if let Some(frame_id) = ws.root.find_frame_with_window(hovered_id) {
                if ws.focused_frame != frame_id || workspaces.focused_workspace != ws.id {
                    return Some(FocusResult {
                        workspace: ws.id,
                        frame: frame_id,
                    });
                }
                return None; // already focused
            }
        }
    }

    // Method 2: Use pointer_position coordinates for any frame (including empty)
    let (px, py) = (input.pointer_x, input.pointer_y);
    if px == 0 && py == 0 {
        return None; // no position yet
    }

    for ws in &workspaces.workspaces {
        let output = match ws.active_output.and_then(|oid| workspaces.output(oid)) {
            Some(o) => o,
            None => continue,
        };
        let area = output.usable_rect();
        let layouts = ws.root.calculate_layout(area, gap);

        if let Some((frame_id, _)) = layouts.iter().find(|(_, rect)| {
            px >= rect.x + margin
                && px < rect.x + rect.width - margin
                && py >= rect.y + margin
                && py < rect.y + rect.height - margin
        }) {
            if ws.focused_frame != *frame_id || workspaces.focused_workspace != ws.id {
                return Some(FocusResult {
                    workspace: ws.id,
                    frame: *frame_id,
                });
            }
            return None; // already focused
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkspaceConfig;
    use crate::layout::WindowRef;
    use crate::workspace::{Output, OutputId};

    fn make_workspace_manager(
        configs: &[WorkspaceConfig],
        outputs: Vec<(OutputId, &str, i32, i32, i32, i32)>,
    ) -> WorkspaceManager {
        let mut wm = WorkspaceManager::new(configs, 0.5);
        for (oid, name, x, y, w, h) in outputs {
            let mut output = Output::new(oid);
            output.name = Some(name.to_string());
            output.x = x;
            output.y = y;
            output.width = w;
            output.height = h;
            wm.add_output(output);
        }
        // Reassign based on names
        wm.reassign_outputs();
        wm
    }

    #[test]
    fn test_focus_follows_mouse_between_frames_same_output() {
        let configs = vec![WorkspaceConfig {
            name: "main".to_string(),
            output: Some("HDMI-A-1".to_string()),
            initial_layout: Some("hsplit".to_string()),
        }];
        let mut wm =
            make_workspace_manager(&configs, vec![(OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080)]);

        // Add a window to frame 1 (left)
        let frame_ids = wm.workspaces[0].root.all_frame_ids();
        let frame1 = frame_ids[0];
        let frame2 = frame_ids[1];

        wm.workspaces[0]
            .root
            .find_frame_mut(frame1)
            .unwrap()
            .add_window(WindowRef {
                window_id: 100,
                app_id: "foot".to_string(),
                title: "term".to_string(),
            });

        // Add a window to frame 2 (right)
        wm.workspaces[0]
            .root
            .find_frame_mut(frame2)
            .unwrap()
            .add_window(WindowRef {
                window_id: 200,
                app_id: "foot".to_string(),
                title: "term2".to_string(),
            });

        // Focus starts on frame1
        wm.workspaces[0].focused_frame = frame1;

        // Pointer hovers window 200 (in frame2)
        let input = FocusInput {
            hovered_window_id: Some(200),
            pointer_x: 1500,
            pointer_y: 540,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(
            result.is_some(),
            "Focus should change when hovering window in different frame"
        );
        let result = result.unwrap();
        assert_eq!(result.frame, frame2);
    }

    #[test]
    fn test_focus_stays_when_hovering_same_frame() {
        let configs = vec![WorkspaceConfig {
            name: "main".to_string(),
            output: Some("HDMI-A-1".to_string()),
            initial_layout: Some("hsplit".to_string()),
        }];
        let mut wm =
            make_workspace_manager(&configs, vec![(OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080)]);

        let frame_ids = wm.workspaces[0].root.all_frame_ids();
        let frame1 = frame_ids[0];

        wm.workspaces[0]
            .root
            .find_frame_mut(frame1)
            .unwrap()
            .add_window(WindowRef {
                window_id: 100,
                app_id: "foot".to_string(),
                title: "term".to_string(),
            });

        wm.workspaces[0].focused_frame = frame1;

        let input = FocusInput {
            hovered_window_id: Some(100),
            pointer_x: 400,
            pointer_y: 540,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(
            result.is_none(),
            "Focus should not change when hovering same frame"
        );
    }

    #[test]
    fn test_focus_follows_pointer_position_into_empty_frame() {
        let configs = vec![WorkspaceConfig {
            name: "main".to_string(),
            output: Some("HDMI-A-1".to_string()),
            initial_layout: Some("hsplit".to_string()),
        }];
        let mut wm =
            make_workspace_manager(&configs, vec![(OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080)]);

        let frame_ids = wm.workspaces[0].root.all_frame_ids();
        let frame1 = frame_ids[0];
        let frame2 = frame_ids[1];

        // Only frame1 has a window, frame2 is empty
        wm.workspaces[0]
            .root
            .find_frame_mut(frame1)
            .unwrap()
            .add_window(WindowRef {
                window_id: 100,
                app_id: "foot".to_string(),
                title: "term".to_string(),
            });

        wm.workspaces[0].focused_frame = frame1;

        // No hovered window (empty frame), but pointer is in right half
        let input = FocusInput {
            hovered_window_id: None,
            pointer_x: 1500,
            pointer_y: 540,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(
            result.is_some(),
            "Focus should follow pointer into empty frame"
        );
        assert_eq!(result.unwrap().frame, frame2);
    }

    #[test]
    fn test_focus_follows_across_monitors() {
        let configs = vec![
            WorkspaceConfig {
                name: "main".to_string(),
                output: Some("HDMI-A-1".to_string()),
                initial_layout: Some("hsplit".to_string()),
            },
            WorkspaceConfig {
                name: "social".to_string(),
                output: Some("DP-1".to_string()),
                initial_layout: None,
            },
        ];
        let mut wm = make_workspace_manager(
            &configs,
            vec![
                (OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080),
                (OutputId(2), "DP-1", 1920, 0, 1080, 1920),
            ],
        );

        let ws0_frame = wm.workspaces[0].root.first_frame_id();
        wm.workspaces[0].focused_frame = ws0_frame;
        wm.focused_workspace = WorkspaceId(0);

        // Pointer moves to second monitor
        let input = FocusInput {
            hovered_window_id: None,
            pointer_x: 2400,
            pointer_y: 960,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(
            result.is_some(),
            "Focus should follow pointer to second monitor"
        );
        let result = result.unwrap();
        assert_eq!(result.workspace, WorkspaceId(1));
    }

    #[test]
    fn test_focus_ignores_pointer_at_origin() {
        let configs = vec![WorkspaceConfig {
            name: "main".to_string(),
            output: Some("HDMI-A-1".to_string()),
            initial_layout: Some("hsplit".to_string()),
        }];
        let wm =
            make_workspace_manager(&configs, vec![(OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080)]);

        let input = FocusInput {
            hovered_window_id: None,
            pointer_x: 0,
            pointer_y: 0,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(result.is_none(), "Should ignore pointer at origin (0,0)");
    }

    #[test]
    fn test_focus_respects_margin() {
        let configs = vec![WorkspaceConfig {
            name: "main".to_string(),
            output: Some("HDMI-A-1".to_string()),
            initial_layout: Some("hsplit".to_string()),
        }];
        let mut wm =
            make_workspace_manager(&configs, vec![(OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080)]);

        let frame_ids = wm.workspaces[0].root.all_frame_ids();
        let frame1 = frame_ids[0];
        wm.workspaces[0].focused_frame = frame1;

        // Pointer right at the edge of frame2 (within margin)
        // hsplit with gap=4: frame1 is 0..958, gap 958..962, frame2 is 962..1920
        // With margin=6, frame2 starts accepting at 962+6=968
        let input = FocusInput {
            hovered_window_id: None,
            pointer_x: 963, // inside frame2 rect but within margin
            pointer_y: 540,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(
            result.is_none(),
            "Should not change focus when pointer is within margin of frame edge"
        );
    }

    #[test]
    fn test_hovered_window_takes_priority_over_position() {
        let configs = vec![WorkspaceConfig {
            name: "main".to_string(),
            output: Some("HDMI-A-1".to_string()),
            initial_layout: Some("hsplit".to_string()),
        }];
        let mut wm =
            make_workspace_manager(&configs, vec![(OutputId(1), "HDMI-A-1", 0, 0, 1920, 1080)]);

        let frame_ids = wm.workspaces[0].root.all_frame_ids();
        let frame1 = frame_ids[0];
        let frame2 = frame_ids[1];

        wm.workspaces[0]
            .root
            .find_frame_mut(frame1)
            .unwrap()
            .add_window(WindowRef {
                window_id: 100,
                app_id: "foot".to_string(),
                title: "term".to_string(),
            });

        wm.workspaces[0]
            .root
            .find_frame_mut(frame2)
            .unwrap()
            .add_window(WindowRef {
                window_id: 200,
                app_id: "foot".to_string(),
                title: "term2".to_string(),
            });

        wm.workspaces[0].focused_frame = frame1;

        // Hovered window is in frame2, but pointer position is in frame1 area
        // (stale pointer_position — PointerEnter should win)
        let input = FocusInput {
            hovered_window_id: Some(200),
            pointer_x: 400, // left half = frame1
            pointer_y: 540,
        };

        let result = compute_focus(&input, &wm, 4, 6);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().frame,
            frame2,
            "Hovered window should take priority over pointer position"
        );
    }

    // ── WindowManager-level integration tests ────────────────────────────

    mod wm_integration {
        use super::*;
        use crate::config::Config;
        use crate::wm::WindowManager;

        fn make_test_wm() -> WindowManager {
            let mut config = Config::default();
            config.general.focus_follows_mouse = true;
            config.general.gap = 4;
            config.general.border_width = 2;
            config.workspaces = vec![
                crate::config::WorkspaceConfig {
                    name: "main".to_string(),
                    output: Some("HDMI-A-1".to_string()),
                    initial_layout: Some("hsplit".to_string()),
                },
                crate::config::WorkspaceConfig {
                    name: "social".to_string(),
                    output: Some("DP-1".to_string()),
                    initial_layout: None,
                },
            ];
            let mut wm = WindowManager::new(config);

            // Add outputs
            let mut output1 = crate::workspace::Output::new(OutputId(1));
            output1.name = Some("HDMI-A-1".to_string());
            output1.x = 0;
            output1.y = 0;
            output1.width = 1920;
            output1.height = 1080;
            wm.workspaces.add_output(output1);

            let mut output2 = crate::workspace::Output::new(OutputId(2));
            output2.name = Some("DP-1".to_string());
            output2.x = 1920;
            output2.y = 0;
            output2.width = 1080;
            output2.height = 1920;
            wm.workspaces.add_output(output2);

            wm.workspaces.reassign_outputs();
            wm
        }

        #[test]
        fn test_wm_focus_follows_hovered_window() {
            let mut wm = make_test_wm();

            let frame_ids = wm.workspaces.workspaces[0].root.all_frame_ids();
            let frame1 = frame_ids[0];
            let frame2 = frame_ids[1];

            // Add windows to both frames
            wm.workspaces.workspaces[0]
                .root
                .find_frame_mut(frame1)
                .unwrap()
                .add_window(WindowRef {
                    window_id: 100,
                    app_id: "foot".into(),
                    title: "term1".into(),
                });
            wm.workspaces.workspaces[0]
                .root
                .find_frame_mut(frame2)
                .unwrap()
                .add_window(WindowRef {
                    window_id: 200,
                    app_id: "foot".into(),
                    title: "term2".into(),
                });

            wm.workspaces.workspaces[0].focused_frame = frame1;
            wm.workspaces.focused_workspace = WorkspaceId(0);

            // Simulate PointerEnter on window 200 (frame2)
            let inputs = vec![FocusInput {
                hovered_window_id: Some(200),
                pointer_x: 1500,
                pointer_y: 540,
            }];

            wm.apply_focus_follows_mouse(&inputs);

            assert_eq!(
                wm.workspaces.workspaces[0].focused_frame, frame2,
                "Focus should have moved to frame2"
            );
        }

        #[test]
        fn test_wm_focus_follows_pointer_to_empty_frame() {
            let mut wm = make_test_wm();

            let frame_ids = wm.workspaces.workspaces[0].root.all_frame_ids();
            let frame1 = frame_ids[0];
            let frame2 = frame_ids[1];

            // Only frame1 has a window
            wm.workspaces.workspaces[0]
                .root
                .find_frame_mut(frame1)
                .unwrap()
                .add_window(WindowRef {
                    window_id: 100,
                    app_id: "foot".into(),
                    title: "term1".into(),
                });

            wm.workspaces.workspaces[0].focused_frame = frame1;

            // Pointer moves to right half (frame2, empty), no hovered window
            let inputs = vec![FocusInput {
                hovered_window_id: None,
                pointer_x: 1500,
                pointer_y: 540,
            }];

            wm.apply_focus_follows_mouse(&inputs);

            assert_eq!(
                wm.workspaces.workspaces[0].focused_frame, frame2,
                "Focus should follow pointer into empty frame"
            );
        }

        #[test]
        fn test_wm_focus_follows_across_monitors() {
            let mut wm = make_test_wm();

            // Focus starts on main workspace (monitor 1)
            let ws0_frame = wm.workspaces.workspaces[0].root.first_frame_id();
            wm.workspaces.workspaces[0].focused_frame = ws0_frame;
            wm.workspaces.focused_workspace = WorkspaceId(0);

            // Pointer moves to second monitor (x=1920+, empty workspace)
            let inputs = vec![FocusInput {
                hovered_window_id: None,
                pointer_x: 2400,
                pointer_y: 960,
            }];

            wm.apply_focus_follows_mouse(&inputs);

            assert_eq!(
                wm.workspaces.focused_workspace,
                WorkspaceId(1),
                "Focus should switch to workspace on second monitor"
            );
        }

        #[test]
        fn test_wm_focus_no_change_when_already_focused() {
            let mut wm = make_test_wm();

            let frame_ids = wm.workspaces.workspaces[0].root.all_frame_ids();
            let frame1 = frame_ids[0];

            wm.workspaces.workspaces[0]
                .root
                .find_frame_mut(frame1)
                .unwrap()
                .add_window(WindowRef {
                    window_id: 100,
                    app_id: "foot".into(),
                    title: "term1".into(),
                });

            wm.workspaces.workspaces[0].focused_frame = frame1;
            wm.workspaces.focused_workspace = WorkspaceId(0);

            let before_frame = wm.workspaces.workspaces[0].focused_frame;
            let before_ws = wm.workspaces.focused_workspace;

            // Hover the already-focused window
            let inputs = vec![FocusInput {
                hovered_window_id: Some(100),
                pointer_x: 400,
                pointer_y: 540,
            }];

            wm.apply_focus_follows_mouse(&inputs);

            assert_eq!(wm.workspaces.workspaces[0].focused_frame, before_frame);
            assert_eq!(wm.workspaces.focused_workspace, before_ws);
        }

        #[test]
        fn test_wm_focus_not_applied_with_zero_position_and_no_hover() {
            let mut wm = make_test_wm();

            let frame_ids = wm.workspaces.workspaces[0].root.all_frame_ids();
            let frame1 = frame_ids[0];

            wm.workspaces.workspaces[0].focused_frame = frame1;
            wm.workspaces.focused_workspace = WorkspaceId(0);

            // No hover, position at origin (not initialized yet)
            let inputs = vec![FocusInput {
                hovered_window_id: None,
                pointer_x: 0,
                pointer_y: 0,
            }];

            wm.apply_focus_follows_mouse(&inputs);

            // Should not change
            assert_eq!(wm.workspaces.workspaces[0].focused_frame, frame1);
            assert_eq!(wm.workspaces.focused_workspace, WorkspaceId(0));
        }
    }
}

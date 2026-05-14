//! Monitor (output) layout management via wlr-output-management-unstable-v1.
//!
//! Single source of truth for: detecting connected monitors, applying saved
//! layouts, recording user edits.
//!
//! Behavior:
//! - First time a given set of EDID descriptors is seen: do nothing, just
//!   record whatever the compositor's defaults are as the profile.
//! - Subsequent appearances of the same set: apply the saved profile.
//! - User changes layout via `wdisplays` / similar: the new layout is saved
//!   automatically (no time gate).
//! - Suspend/resume: the EDID set does not change, so no apply is triggered;
//!   layout is preserved.
//!
//! Profile storage: `~/.config/notion-river/monitors.json`, keyed by the
//! sorted, newline-joined set of EDID descriptors. Refresh rate is intentionally
//! not part of equality (used only as a hint when picking a `wl_output` mode).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::protocol::zwlr_output_manager_v1::ZwlrOutputManagerV1;

const FILE: &str = "monitors.json";

/// Maximum number of consecutive `failed`/`cancelled` events tolerated for a
/// given set key before we stop retrying. Boot-time DRM races typically
/// resolve after 1 retry; a couple more covers slow displays.
pub const MAX_APPLY_RETRIES: u32 = 4;

fn store_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("notion-river");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(FILE)
}

/// Strip a trailing ` (connector)` suffix added by wlroots from an EDID
/// description. The connector token may differ between sessions/ports, so we
/// drop it to get a stable identity.
pub fn canonical_descriptor(desc: &str) -> String {
    let trimmed = desc.trim();
    if let Some(open) = trimmed.rfind(" (")
        && trimmed.ends_with(')')
        && trimmed[open + 2..trimmed.len() - 1]
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-')
    {
        trimmed[..open].trim().to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Stable per-monitor key from `wlr_output_head` `name` + `description`.
///
/// External monitors get their EDID descriptor (connector-stripped). Built-in
/// panels often have generic descriptions ("Sharp Corporation 0x1515"); they
/// stay reasonably stable too. As a last resort, the connector name is used
/// for built-in panels (eDP-/LVDS-/DSI-) since those *are* stable.
pub fn monitor_key(name: Option<&str>, description: Option<&str>) -> Option<String> {
    if let Some(desc) = description {
        let canon = canonical_descriptor(desc);
        if !canon.is_empty() {
            return Some(canon);
        }
    }
    if let Some(name) = name
        && (name.starts_with("eDP-") || name.starts_with("LVDS-") || name.starts_with("DSI-"))
    {
        return Some(name.to_owned());
    }
    None
}

/// Identifies a *set* of monitors by their sorted EDID descriptors.
pub type SetKey = String;

pub fn set_key_from(keys: &[&str]) -> SetKey {
    let mut v: Vec<&str> = keys.to_vec();
    v.sort();
    v.dedup();
    v.join("\n")
}

/// Live state of a single head, populated from `zwlr_output_head_v1` events.
#[derive(Debug, Clone, Default)]
pub struct HeadLive {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub mode_w: i32,
    pub mode_h: i32,
    pub mode_refresh_mhz: i32,
    pub position_x: i32,
    pub position_y: i32,
    /// Stored as wl_fixed * 120000 units (per protocol).
    pub scale_fixed: i32,
    pub transform: i32,
    pub mode_ids: Vec<u64>,
    pub current_mode_id: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ModeLive {
    pub w: i32,
    pub h: i32,
    pub refresh_mhz: i32,
    #[allow(dead_code)]
    pub preferred: bool,
}

/// Per-head saved configuration. Refresh is intentionally absent from the
/// stored format: it's a hint, not a constraint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedHead {
    pub mode_w: i32,
    pub mode_h: i32,
    pub position_x: i32,
    pub position_y: i32,
    pub scale: f64,
    pub transform: i32,
    pub enabled: bool,
}

/// Set of saved profiles, on disk as a JSON map keyed by `SetKey`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profiles {
    #[serde(flatten)]
    pub map: HashMap<SetKey, HashMap<String, SavedHead>>,
}

impl Profiles {
    pub fn load() -> Self {
        let path = store_path();
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                log::warn!("Failed to parse {FILE}: {e} — starting fresh");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = store_path();
        let tmp = path.with_extension("json.tmp");
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if std::fs::write(&tmp, &json).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                } else {
                    log::warn!("Failed to write {FILE} tmp file");
                }
            }
            Err(e) => log::warn!("Failed to serialize {FILE}: {e}"),
        }
    }

    pub fn get(&self, set_key: &str) -> Option<&HashMap<String, SavedHead>> {
        self.map.get(set_key)
    }

    /// Insert or replace the profile for the given set key. Returns `true`
    /// if the stored value actually changed (so callers know whether to
    /// persist to disk).
    pub fn insert(&mut self, set_key: SetKey, heads: HashMap<String, SavedHead>) -> bool {
        match self.map.get(&set_key) {
            Some(existing) if existing == &heads => false,
            _ => {
                self.map.insert(set_key, heads);
                true
            }
        }
    }
}

/// Top-level monitor manager. Owns the wlr-output-management binding,
/// per-head live state, and the on-disk profile store.
#[derive(Debug, Default)]
pub struct Monitors {
    pub manager: Option<ZwlrOutputManagerV1>,
    /// Most recent serial from the manager. Required by the apply path.
    pub serial: Option<u32>,
    /// True between `create_configuration().apply()` and the corresponding
    /// `Succeeded`/`Failed`/`Cancelled` event.
    pub apply_in_flight: bool,
    /// The set key we last observed in a complete `Done` event, or `None`
    /// before the first `Done`.
    pub last_set_key: Option<SetKey>,
    /// If `Some(key)`, we issued an apply for this set key and are waiting
    /// for the compositor to ack it. The next `Done` for this set key will
    /// be treated as confirmation, not as a user edit.
    pub pending_self_apply: Option<SetKey>,
    /// Number of consecutive `failed`/`cancelled` applies per set key.
    /// Used to bound retry attempts: each new `Done` with divergence will
    /// retry until the counter reaches `MAX_APPLY_RETRIES`. Cleared on
    /// success. Boot-time DRM races (link training, EDID negotiation) often
    /// reject the first apply but accept a retry milliseconds later, so we
    /// must retry rather than give up forever on the first failure.
    pub apply_failures: std::collections::HashMap<SetKey, u32>,
    /// Live head state, indexed by `wl_object` id of the head proxy.
    pub heads: HashMap<u64, HeadLive>,
    /// Live mode state, indexed by `wl_object` id of the mode proxy.
    pub modes: HashMap<u64, ModeLive>,
    /// On-disk saved profiles.
    pub profiles: Profiles,
}

impl Monitors {
    pub fn load() -> Self {
        Self {
            profiles: Profiles::load(),
            ..Default::default()
        }
    }
}

/// Build a `(set_key, edid -> SavedHead)` snapshot from the current head live
/// state. Returns `None` if any enabled head has incomplete metadata (e.g. no
/// description yet, no mode chosen). In that case the caller should wait for
/// the next `Done`.
pub fn snapshot(
    heads: &HashMap<u64, HeadLive>,
    modes: &HashMap<u64, ModeLive>,
) -> Option<(SetKey, HashMap<String, SavedHead>)> {
    let mut entries: Vec<(String, SavedHead)> = Vec::new();
    for h in heads.values() {
        let key = monitor_key(h.name.as_deref(), h.description.as_deref())?;
        if !h.enabled {
            entries.push((
                key,
                SavedHead {
                    mode_w: 0,
                    mode_h: 0,
                    position_x: 0,
                    position_y: 0,
                    scale: 1.0,
                    transform: 0,
                    enabled: false,
                },
            ));
            continue;
        }
        let mode = h.current_mode_id.and_then(|id| modes.get(&id));
        let (w, h_px) = mode.map_or((h.mode_w, h.mode_h), |m| (m.w, m.h));
        if w <= 0 || h_px <= 0 || h.scale_fixed <= 0 {
            return None;
        }
        entries.push((
            key,
            SavedHead {
                mode_w: w,
                mode_h: h_px,
                position_x: h.position_x,
                position_y: h.position_y,
                scale: h.scale_fixed as f64 / 120_000.0,
                transform: h.transform,
                enabled: true,
            },
        ));
    }
    if entries.is_empty() {
        return None;
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
    let key = set_key_from(&keys);
    Some((key, entries.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(name: &str, desc: &str, w: i32, h: i32, x: i32, y: i32) -> HeadLive {
        HeadLive {
            name: Some(name.into()),
            description: Some(desc.into()),
            enabled: true,
            mode_w: w,
            mode_h: h,
            mode_refresh_mhz: 60_000,
            position_x: x,
            position_y: y,
            scale_fixed: 120_000,
            transform: 0,
            mode_ids: vec![],
            current_mode_id: None,
        }
    }

    #[test]
    fn canonical_strips_connector() {
        assert_eq!(canonical_descriptor("LG HDR 4K 503NTWG54001 (DP-3)"), "LG HDR 4K 503NTWG54001");
    }

    #[test]
    fn canonical_keeps_paren_in_name() {
        assert_eq!(canonical_descriptor("Some (Test) Monitor"), "Some (Test) Monitor");
    }

    #[test]
    fn monitor_key_falls_back_to_internal_connector() {
        assert_eq!(monitor_key(Some("eDP-1"), None).as_deref(), Some("eDP-1"));
        assert_eq!(monitor_key(Some("DP-3"), None), None);
    }

    #[test]
    fn set_key_is_sorted_and_deduped() {
        assert_eq!(set_key_from(&["B", "A", "B"]), "A\nB");
    }

    #[test]
    fn snapshot_complete() {
        let mut heads = HashMap::new();
        heads.insert(1, live("eDP-1", "Sharp 0x1515", 1920, 1200, 0, 0));
        heads.insert(
            2,
            live("DP-3", "LG 503NTWG54001 (DP-3)", 3840, 2160, 1920, 0),
        );
        let modes = HashMap::new();
        let (key, snap) = snapshot(&heads, &modes).unwrap();
        assert_eq!(key, "LG 503NTWG54001\nSharp 0x1515");
        assert_eq!(snap.len(), 2);
        assert_eq!(snap.get("Sharp 0x1515").unwrap().mode_w, 1920);
        assert_eq!(snap.get("LG 503NTWG54001").unwrap().position_x, 1920);
    }

    #[test]
    fn snapshot_none_when_metadata_missing() {
        let mut heads = HashMap::new();
        let mut h = live("eDP-1", "Sharp", 0, 0, 0, 0);
        h.scale_fixed = 0;
        heads.insert(1, h);
        assert!(snapshot(&heads, &HashMap::new()).is_none());
    }

    #[test]
    fn profiles_insert_returns_true_on_change() {
        let mut p = Profiles::default();
        let mut m = HashMap::new();
        m.insert(
            "A".into(),
            SavedHead {
                mode_w: 1920,
                mode_h: 1200,
                position_x: 0,
                position_y: 0,
                scale: 1.0,
                transform: 0,
                enabled: true,
            },
        );
        assert!(p.insert("k1".into(), m.clone()));
        assert!(!p.insert("k1".into(), m.clone()));
        let mut m2 = m.clone();
        m2.get_mut("A").unwrap().position_x = 100;
        assert!(p.insert("k1".into(), m2));
    }

    #[test]
    fn profiles_round_trip() {
        let mut p = Profiles::default();
        let mut m = HashMap::new();
        m.insert(
            "A".into(),
            SavedHead {
                mode_w: 1920,
                mode_h: 1200,
                position_x: 0,
                position_y: 0,
                scale: 1.5,
                transform: 0,
                enabled: true,
            },
        );
        p.insert("k1".into(), m);
        let json = serde_json::to_string(&p).unwrap();
        let back: Profiles = serde_json::from_str(&json).unwrap();
        assert_eq!(back.map.len(), 1);
        assert!(back.get("k1").is_some());
    }
}

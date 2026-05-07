//! Per-monitor workspace memory.
//!
//! Records, for each physical monitor (identified by EDID description, with
//! connector name as fallback), which workspace was last shown on it. When the
//! monitor reconnects, the same workspace returns to it.
//!
//! Replaces the old whole-setup `output-profiles.json` and the
//! `visible_workspaces` field in saved state. One source of truth, keyed per
//! physical monitor, not per geometry fingerprint.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::workspace::Output;

const FILE: &str = "monitor-memory.json";

fn store_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("notion-river");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(FILE)
}

fn is_stable_key(key: &str) -> bool {
    key.starts_with("edid:")
        || key
            .strip_prefix("conn:")
            .is_some_and(is_builtin_connector)
}

/// Stable identifier for a physical monitor.
///
/// Strict: only returns Some when the output has a *stable* identity that
/// survives port changes and resolution changes. There is intentionally no
/// geometry fallback — geometry is not identity. With two monitors of the same
/// resolution (common at home: dual 4K), a geometry key collides between them
/// and corrupts placement decisions.
///
/// Preference order:
/// 1. EDID description with the trailing `(connector)` suffix stripped, since
///    wlroots appends the current connector to the description string.
/// 2. Connector name only for built-in panels (`eDP-*`, `LVDS-*`, `DSI-*`),
///    which are physically tied to their connector. External monitor connector
///    names are not stable identity (you can plug into any port).
///
/// Returns None for outputs whose Wayland metadata hasn't fully arrived yet.
/// Callers must treat None as "defer the decision until later".
pub fn monitor_key(output: &Output) -> Option<String> {
    if let Some(desc) = output.description.as_deref() {
        let trimmed = strip_connector_suffix(desc).trim();
        if !trimmed.is_empty() {
            return Some(format!("edid:{trimmed}"));
        }
    }
    if let Some(name) = output.name.as_deref()
        && is_builtin_connector(name)
    {
        return Some(format!("conn:{name}"));
    }
    None
}

/// True if the connector name belongs to a built-in panel that is physically
/// tied to this connector. Unlike DP-* / HDMI-* connectors, these don't get
/// reassigned across replug, so the connector name is a stable identity.
fn is_builtin_connector(name: &str) -> bool {
    name.starts_with("eDP-") || name.starts_with("LVDS-") || name.starts_with("DSI-")
}

/// Strip a trailing `" (connector)"` suffix added by wlroots so that replugging
/// into a different port keeps the same identity.
fn strip_connector_suffix(desc: &str) -> &str {
    if let Some(open) = desc.rfind(" (")
        && desc.ends_with(')')
    {
        return &desc[..open];
    }
    desc
}

/// On-disk and in-memory map: monitor key -> last workspace name shown there.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MonitorMemory {
    #[serde(flatten)]
    pub entries: HashMap<String, String>,
}

impl MonitorMemory {
    pub fn load() -> Self {
        Self::load_from_path(&store_path())
    }

    fn load_from_path(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(json) => {
                let mut memory: Self = serde_json::from_str(&json).unwrap_or_default();
                if memory.prune_unstable() {
                    memory.save_to_path(path);
                }
                memory
            }
            Err(_) => Self::default(),
        }
    }

    fn save_to_path(&self, path: &std::path::Path) {
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    log::warn!("Failed to write monitor memory: {e}");
                }
            }
            Err(e) => log::warn!("Failed to serialize monitor memory: {e}"),
        }
    }

    fn prune_unstable(&mut self) -> bool {
        let old_len = self.entries.len();
        self.entries.retain(|key, _| is_stable_key(key));
        self.entries.len() != old_len
    }

    /// Look up the workspace that was last shown on the monitor matching `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Record `workspace` as the current occupant of the monitor with `key`.
    /// Returns true if the entry changed (caller decides whether to persist).
    #[cfg(test)]
    pub fn record(&mut self, key: String, workspace: String) -> bool {
        match self.entries.get(&key) {
            Some(existing) if existing == &workspace => false,
            _ => {
                self.entries.insert(key, workspace);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::OutputId;

    fn output(name: Option<&str>, desc: Option<&str>, w: i32, h: i32) -> Output {
        let mut o = Output::new(OutputId(1));
        o.name = name.map(str::to_owned);
        o.description = desc.map(str::to_owned);
        o.width = w;
        o.height = h;
        o
    }

    #[test]
    fn description_strips_connector_suffix() {
        let o = output(
            Some("DP-2"),
            Some("LG Electronics LG HDR 4K 503NTWG54001 (DP-2)"),
            2560,
            1440,
        );
        assert_eq!(
            monitor_key(&o).as_deref(),
            Some("edid:LG Electronics LG HDR 4K 503NTWG54001"),
        );
    }

    #[test]
    fn description_keeps_full_string_without_suffix() {
        let o = output(Some("DP-2"), Some("Some Monitor X1"), 2560, 1440);
        assert_eq!(monitor_key(&o).as_deref(), Some("edid:Some Monitor X1"));
    }

    #[test]
    fn monitor_key_returns_none_for_geometry_only() {
        let o = output(None, None, 2560, 1440);
        assert_eq!(monitor_key(&o), None);
    }

    #[test]
    fn monitor_key_returns_none_for_external_connector_only() {
        let o = output(Some("DP-2"), None, 2560, 1440);
        assert_eq!(monitor_key(&o), None);
    }

    #[test]
    fn monitor_key_returns_some_for_builtin_connector_only() {
        let o = output(Some("eDP-1"), None, 2560, 1440);
        assert_eq!(monitor_key(&o).as_deref(), Some("conn:eDP-1"));
    }

    #[test]
    fn returns_none_without_anything() {
        let o = output(None, None, 0, 0);
        assert_eq!(monitor_key(&o), None);
    }

    #[test]
    fn prune_drops_geom_and_external_conn_entries() {
        let path = std::env::temp_dir().join(format!(
            "notion-river-monitor-memory-{}-{}.json",
            std::process::id(),
            "prune"
        ));
        let _ = std::fs::remove_file(&path);

        std::fs::write(
            &path,
            concat!(
                "{\n",
                "  \"edid:one\": \"main\",\n",
                "  \"edid:two\": \"work\",\n",
                "  \"edid:three\": \"social\",\n",
                "  \"conn:eDP-1\": \"term\",\n",
                "  \"conn:DP-3\": \"secondary\",\n",
                "  \"geom:3840x2160\": \"utility\",\n",
                "  \"geom:1920x1200\": \"main\"\n",
                "}\n"
            ),
        )
        .unwrap();

        let memory = MonitorMemory::load_from_path(&path);

        assert_eq!(memory.entries.len(), 4);
        assert_eq!(memory.get("edid:one"), Some("main"));
        assert_eq!(memory.get("edid:two"), Some("work"));
        assert_eq!(memory.get("edid:three"), Some("social"));
        assert_eq!(memory.get("conn:eDP-1"), Some("term"));
        assert_eq!(memory.get("conn:DP-3"), None);
        assert_eq!(memory.get("geom:3840x2160"), None);

        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("conn:DP-3"));
        assert!(!disk.contains("geom:3840x2160"));
        assert!(disk.contains("conn:eDP-1"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prune_writes_to_disk_only_when_changed() {
        let clean_path = std::env::temp_dir().join(format!(
            "notion-river-monitor-memory-{}-{}.json",
            std::process::id(),
            "clean"
        ));
        let dirty_path = std::env::temp_dir().join(format!(
            "notion-river-monitor-memory-{}-{}.json",
            std::process::id(),
            "dirty"
        ));
        let _ = std::fs::remove_file(&clean_path);
        let _ = std::fs::remove_file(&dirty_path);

        let clean = concat!(
            "{\n",
            "  \"edid:one\": \"main\",\n",
            "  \"conn:eDP-1\": \"work\"\n",
            "}\n"
        );
        std::fs::write(&clean_path, clean).unwrap();
        std::fs::write(&dirty_path, "{\"edid:one\":\"main\",\"geom:1x1\":\"bad\"}\n")
            .unwrap();

        let _ = MonitorMemory::load_from_path(&clean_path);
        let _ = MonitorMemory::load_from_path(&dirty_path);

        assert_eq!(std::fs::read_to_string(&clean_path).unwrap(), clean);
        assert_eq!(
            std::fs::read_to_string(&dirty_path).unwrap(),
            "{\n  \"edid:one\": \"main\"\n}"
        );

        let _ = std::fs::remove_file(&clean_path);
        let _ = std::fs::remove_file(&dirty_path);
    }

    #[test]
    fn record_returns_true_on_change_only() {
        let mut m = MonitorMemory::default();
        assert!(m.record("a".into(), "main".into()));
        assert!(!m.record("a".into(), "main".into()));
        assert!(m.record("a".into(), "utility".into()));
    }
}

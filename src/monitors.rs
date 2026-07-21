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
//! Applies are **debounced**: a divergence from the saved profile schedules a
//! pending apply (`schedule_apply`) with a settle deadline; every topology
//! change pushes the deadline forward (`bump_pending_deadline`). The main loop
//! fires it only once the topology has been quiet for `SETTLE_DELAY`. This
//! avoids racing the compositor's own modeset while dock outputs flap up/down
//! on resume/hotplug. Failed applies retry up to `MAX_APPLY_ATTEMPTS` with
//! `RETRY_BACKOFF` spacing; the budget resets when the live set changes.
//!
//! Profile storage: `~/.config/notion-river/monitors.json`, keyed by the
//! sorted, newline-joined set of EDID descriptors. Refresh rate is intentionally
//! not part of equality (used only as a hint when picking a `wl_output` mode).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::protocol::zwlr_output_manager_v1::ZwlrOutputManagerV1;

const FILE: &str = "monitors.json";

/// How long the live monitor topology must stay unchanged before we act on a
/// divergence from the saved profile. On resume/hotplug the compositor often
/// flaps outputs up and down for a few hundred milliseconds; applying a config
/// during that window collides with the compositor's own modeset and fails.
pub const SETTLE_DELAY: Duration = Duration::from_millis(600);

/// Maximum number of apply attempts for a given monitor set before giving up
/// for this appearance of the set. Reset whenever the topology changes again.
pub const MAX_APPLY_ATTEMPTS: u32 = 5;

/// Base backoff between apply retries after a `Failed`/`Cancelled` result.
/// The effective backoff scales with the attempt count (see
/// [`Monitors::note_apply_failure`]) up to [`RETRY_BACKOFF_MAX`].
pub const RETRY_BACKOFF: Duration = Duration::from_millis(800);

/// Upper bound on the per-attempt retry backoff. Caps the escalating backoff so
/// the final retry of a persistently-flapping dock still happens within a few
/// seconds rather than minutes.
pub const RETRY_BACKOFF_MAX: Duration = Duration::from_millis(4000);

/// Rolling window over which repeated (re)appearances of the same monitor set
/// are counted for flap detection.
pub const FLAP_WINDOW: Duration = Duration::from_secs(20);

/// Number of (re)appearances of the same set within [`FLAP_WINDOW`] that marks
/// the set as flapping. A healthy hotplug/resume produces a small burst; a
/// physically failing link (marginal cable, dying panel, flaky MST hub) keeps
/// re-enumerating indefinitely. Past this count we quarantine the set.
pub const FLAP_THRESHOLD: u32 = 4;

/// How long a flapping set must stay quiet (no reappearance) before its
/// quarantine is lifted and normal apply behavior resumes.
pub const FLAP_COOLDOWN: Duration = Duration::from_secs(30);

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
    /// A divergence we intend to act on once the topology settles. Set when a
    /// `Done` event reports a set that diverges from its saved profile; the
    /// `deadline` is pushed forward on every subsequent topology change. The
    /// main loop fires the apply once `Instant::now() >= deadline` and the set
    /// key still matches the live topology.
    pub pending_apply: Option<PendingApply>,
    /// Per-set apply attempt counters for the current appearance of each set.
    /// Reset to 0 whenever the set's topology changes (i.e. it disappears and
    /// reappears, or its members change). A set that reaches
    /// `MAX_APPLY_ATTEMPTS` is left alone until it next changes.
    pub apply_attempts: HashMap<SetKey, u32>,
    /// Earliest instant at which the given set may be (re)applied. Populated
    /// after a `Failed`/`Cancelled` result to space out retries.
    pub retry_after: HashMap<SetKey, Instant>,
    /// Instant of the most recent topology change (output add/remove, head
    /// `Done`, or a self-apply ack). A pending apply only fires once this is at
    /// least `SETTLE_DELAY` in the past, so we never poke a topology that is
    /// still actively flapping (e.g. an MST dock mid enumeration). This is
    /// independent of the per-apply deadline and is the real quiet-gate.
    pub last_topology_change: Option<Instant>,
    /// Timestamps of recent (re)appearances of each monitor set, used for flap
    /// detection. Entries older than [`FLAP_WINDOW`] are pruned. A set whose
    /// count exceeds [`FLAP_THRESHOLD`] within the window is quarantined.
    ///
    /// This is deliberately independent of `apply_attempts`: that counter is
    /// wiped every time the live set key changes (i.e. on every reappearance),
    /// so it can never accumulate under a fast flap. Flap history persists
    /// across those reappearances precisely so a self-sustaining loop is caught.
    pub flap_history: HashMap<SetKey, Vec<Instant>>,
    /// Set keys currently quarantined due to flapping, mapped to the instant at
    /// which the quarantine may be lifted (extended on every reappearance while
    /// flapping). While quarantined, no apply is scheduled or fired for the set.
    pub flap_quarantine: HashMap<SetKey, Instant>,
    /// Live head state, indexed by `wl_object` id of the head proxy.
    pub heads: HashMap<u64, HeadLive>,
    /// Live mode state, indexed by `wl_object` id of the mode proxy.
    pub modes: HashMap<u64, ModeLive>,
    /// On-disk saved profiles.
    pub profiles: Profiles,
}

/// A debounced, pending profile application.
#[derive(Debug, Clone)]
pub struct PendingApply {
    /// The monitor set this apply targets.
    pub set_key: SetKey,
    /// The saved profile to apply (resolved at scheduling time).
    pub target: HashMap<String, SavedHead>,
    /// Earliest instant at which the apply may fire. Pushed forward on every
    /// topology change so the apply only runs once things have settled.
    pub deadline: Instant,
}

impl Monitors {
    pub fn load() -> Self {
        Self {
            profiles: Profiles::load(),
            ..Default::default()
        }
    }

    /// Schedule (or reschedule) a debounced apply for `set_key`. Pushes the
    /// settle deadline forward to `now + SETTLE_DELAY`, honoring any pending
    /// retry backoff for the set. Replaces any existing pending apply.
    pub fn schedule_apply(
        &mut self,
        set_key: SetKey,
        target: HashMap<String, SavedHead>,
        now: Instant,
    ) {
        let earliest = self
            .retry_after
            .get(&set_key)
            .copied()
            .map_or(now, |t| t.max(now));
        let deadline = earliest + SETTLE_DELAY;
        self.pending_apply = Some(PendingApply {
            set_key,
            target,
            deadline,
        });
    }

    /// Push the current pending apply's deadline forward so it only fires once
    /// the topology has been quiet for `SETTLE_DELAY`. Called on every topology
    /// change (output add/remove, `Done`) while an apply is pending.
    pub fn bump_pending_deadline(&mut self, now: Instant) {
        if let Some(pending) = self.pending_apply.as_mut() {
            pending.deadline = (now + SETTLE_DELAY).max(pending.deadline);
        }
    }

    /// Record that the live topology just changed. Resets the quiet timer used
    /// by [`Monitors::topology_quiet_for`], and also bumps any pending apply's
    /// deadline. Call on every output add/remove and head `Done`.
    pub fn note_topology_change(&mut self, now: Instant) {
        self.last_topology_change = Some(now);
        self.bump_pending_deadline(now);
    }

    /// True when the topology has been quiet (no change) for at least `delay`.
    /// Before the first observed change the topology is considered quiet.
    pub fn topology_quiet_for(&self, delay: Duration, now: Instant) -> bool {
        match self.last_topology_change {
            Some(t) => now.duration_since(t) >= delay,
            None => true,
        }
    }

    /// Cancel any pending apply (e.g. the live topology no longer matches the
    /// pending set, or a self-apply ack arrived).
    pub fn clear_pending_apply(&mut self) {
        self.pending_apply = None;
    }

    /// Record a failed/cancelled apply for `set_key`: bump the attempt counter
    /// and set a retry backoff. Returns `true` if more attempts remain.
    ///
    /// The backoff grows with the attempt count (`RETRY_BACKOFF * attempts`,
    /// capped at [`RETRY_BACKOFF_MAX`]). A monitor set that keeps failing is
    /// usually one whose topology is still flapping (e.g. an MST dock mid
    /// enumeration); poking it on a fixed short interval just prolongs the
    /// churn, so each failure waits progressively longer for things to settle.
    pub fn note_apply_failure(&mut self, set_key: &str, now: Instant) -> bool {
        let attempts = self.apply_attempts.entry(set_key.to_owned()).or_insert(0);
        *attempts += 1;
        let backoff = (RETRY_BACKOFF * *attempts).min(RETRY_BACKOFF_MAX);
        self.retry_after.insert(set_key.to_owned(), now + backoff);
        *attempts < MAX_APPLY_ATTEMPTS
    }

    /// Clear failure/retry/attempt bookkeeping for `set_key` (on success or
    /// when the user explicitly saves/forgets the profile).
    pub fn clear_failure(&mut self, set_key: &str) {
        self.apply_attempts.remove(set_key);
        self.retry_after.remove(set_key);
    }

    /// True if `set_key` has exhausted its apply attempts for this appearance.
    pub fn attempts_exhausted(&self, set_key: &str) -> bool {
        self.apply_attempts
            .get(set_key)
            .is_some_and(|n| *n >= MAX_APPLY_ATTEMPTS)
    }

    /// Reset the attempt counter for any set that is no longer the live set.
    /// Called when the live set key changes so a freshly (re)connected set
    /// starts with a clean retry budget.
    pub fn reset_attempts_except(&mut self, live_set_key: &str) {
        self.apply_attempts.retain(|k, _| k == live_set_key);
        self.retry_after.retain(|k, _| k == live_set_key);
    }

    /// Record a (re)appearance of `set_key` and update flap state. Returns
    /// `true` if the set is (now) quarantined and must not be applied to.
    ///
    /// A set is quarantined once it reappears more than [`FLAP_THRESHOLD`]
    /// times within [`FLAP_WINDOW`]. This catches a link that keeps dropping
    /// and re-enumerating on its own (apply may even "succeed" each time, so
    /// the failure-based retry budget never fires). Quarantine is refreshed on
    /// every subsequent reappearance and lifts only after [`FLAP_COOLDOWN`] of
    /// quiet (see [`Monitors::flap_quarantined`]).
    pub fn note_set_appearance(&mut self, set_key: &str, now: Instant) -> bool {
        let history = self.flap_history.entry(set_key.to_owned()).or_default();
        history.retain(|t| now.duration_since(*t) < FLAP_WINDOW);
        history.push(now);
        let count = history.len() as u32;

        // While a set is already quarantined, any reappearance during cooldown
        // means it is still unstable: refresh the deadline. This holds even if
        // the windowed count has dipped below the threshold (e.g. COOLDOWN
        // outlasts FLAP_WINDOW), so a slow but persistent flap can't sneak out.
        let already = self.flap_quarantine.contains_key(set_key);
        if count > FLAP_THRESHOLD || already {
            self.flap_quarantine
                .insert(set_key.to_owned(), now + FLAP_COOLDOWN);
            if !already {
                log::warn!(
                    "Monitor set '{set_key}' is flapping ({count} appearances in \
                     {}s); quarantining, will not apply until it stays stable for {}s. \
                     This usually indicates a marginal cable, dock/hub port, or panel.",
                    FLAP_WINDOW.as_secs(),
                    FLAP_COOLDOWN.as_secs(),
                );
            }
            return true;
        }

        false
    }

    /// True while `set_key` is quarantined for flapping. Expired quarantines are
    /// lifted (and their flap history cleared) as a side effect so the set can
    /// resume normal apply behavior once the link has settled.
    pub fn flap_quarantined(&mut self, set_key: &str, now: Instant) -> bool {
        match self.flap_quarantine.get(set_key).copied() {
            Some(until) if now < until => true,
            Some(_) => {
                self.flap_quarantine.remove(set_key);
                self.flap_history.remove(set_key);
                log::info!("Monitor set '{set_key}' stable again; lifting flap quarantine.");
                false
            }
            None => false,
        }
    }

    /// Clear all flap bookkeeping for `set_key` (on explicit user save/forget).
    pub fn clear_flap(&mut self, set_key: &str) {
        self.flap_history.remove(set_key);
        self.flap_quarantine.remove(set_key);
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

/// True when every enabled head carries at least one usable mode (a populated
/// `mode_ids` list and a known current/target mode). The apply path needs a
/// mode proxy to call `set_mode`; if modes haven't arrived yet, handing the
/// compositor an underspecified config during a hotplug/resume tends to fail.
/// Disabled heads impose no requirement.
pub fn heads_have_apply_modes(heads: &HashMap<u64, HeadLive>) -> bool {
    heads.values().filter(|h| h.enabled).all(|h| {
        !h.mode_ids.is_empty() && (h.current_mode_id.is_some() || (h.mode_w > 0 && h.mode_h > 0))
    })
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

    fn saved(w: i32, h: i32) -> HashMap<String, SavedHead> {
        let mut m = HashMap::new();
        m.insert(
            "A".into(),
            SavedHead {
                mode_w: w,
                mode_h: h,
                position_x: 0,
                position_y: 0,
                scale: 1.0,
                transform: 0,
                enabled: true,
            },
        );
        m
    }

    #[test]
    fn schedule_apply_sets_deadline_in_future() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        mons.schedule_apply("k".into(), saved(1920, 1080), now);
        let p = mons.pending_apply.as_ref().unwrap();
        assert_eq!(p.set_key, "k");
        assert!(p.deadline >= now + SETTLE_DELAY);
    }

    #[test]
    fn bump_pushes_deadline_forward_only() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        mons.schedule_apply("k".into(), saved(1920, 1080), now);
        let first = mons.pending_apply.as_ref().unwrap().deadline;
        // Bump from a later instant extends the deadline.
        let later = now + Duration::from_millis(300);
        mons.bump_pending_deadline(later);
        let second = mons.pending_apply.as_ref().unwrap().deadline;
        assert!(second > first);
        // Bump from an earlier instant never shortens it.
        mons.bump_pending_deadline(now);
        assert_eq!(mons.pending_apply.as_ref().unwrap().deadline, second);
    }

    #[test]
    fn topology_quiet_gate() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        // No change observed yet → considered quiet.
        assert!(mons.topology_quiet_for(SETTLE_DELAY, now));
        // A change resets the timer: not quiet until SETTLE_DELAY elapses.
        mons.note_topology_change(now);
        assert!(!mons.topology_quiet_for(SETTLE_DELAY, now));
        assert!(!mons.topology_quiet_for(SETTLE_DELAY, now + SETTLE_DELAY - Duration::from_millis(1)));
        assert!(mons.topology_quiet_for(SETTLE_DELAY, now + SETTLE_DELAY));
    }

    #[test]
    fn note_topology_change_also_bumps_deadline() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        mons.schedule_apply("k".into(), saved(1920, 1080), now);
        let first = mons.pending_apply.as_ref().unwrap().deadline;
        mons.note_topology_change(now + Duration::from_millis(300));
        assert!(mons.pending_apply.as_ref().unwrap().deadline > first);
    }

    #[test]
    fn retry_backoff_escalates_with_attempts() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        mons.note_apply_failure("k", now);
        let first = *mons.retry_after.get("k").unwrap();
        mons.note_apply_failure("k", now);
        let second = *mons.retry_after.get("k").unwrap();
        // Second failure backs off further than the first.
        assert!(second > first);
        // Capped at RETRY_BACKOFF_MAX.
        for _ in 0..20 {
            mons.note_apply_failure("k", now);
        }
        assert!(*mons.retry_after.get("k").unwrap() <= now + RETRY_BACKOFF_MAX);
    }

    #[test]
    fn retry_backoff_delays_next_schedule() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        assert!(mons.note_apply_failure("k", now));
        // Next schedule must not fire before the backoff elapses.
        mons.schedule_apply("k".into(), saved(1920, 1080), now);
        let deadline = mons.pending_apply.as_ref().unwrap().deadline;
        assert!(deadline >= now + RETRY_BACKOFF);
    }

    #[test]
    fn attempts_exhaust_after_max() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        for _ in 0..MAX_APPLY_ATTEMPTS - 1 {
            assert!(mons.note_apply_failure("k", now));
        }
        // The final attempt exhausts the budget.
        assert!(!mons.note_apply_failure("k", now));
        assert!(mons.attempts_exhausted("k"));
    }

    #[test]
    fn clear_failure_resets_budget() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        for _ in 0..MAX_APPLY_ATTEMPTS {
            mons.note_apply_failure("k", now);
        }
        assert!(mons.attempts_exhausted("k"));
        mons.clear_failure("k");
        assert!(!mons.attempts_exhausted("k"));
    }

    #[test]
    fn reset_attempts_except_keeps_live_set() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        mons.note_apply_failure("old", now);
        mons.note_apply_failure("live", now);
        mons.reset_attempts_except("live");
        assert!(!mons.apply_attempts.contains_key("old"));
        assert!(mons.apply_attempts.contains_key("live"));
    }

    #[test]
    fn apply_modes_ready_requires_modes_for_enabled_heads() {
        let mut heads = HashMap::new();
        // Enabled head with no mode_ids → not ready.
        heads.insert(1, live("eDP-1", "Sharp", 1920, 1200, 0, 0));
        assert!(!heads_have_apply_modes(&heads));
        // Give it a mode → ready.
        heads.get_mut(&1).unwrap().mode_ids = vec![10];
        assert!(heads_have_apply_modes(&heads));
    }

    #[test]
    fn flap_quarantines_after_threshold() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        // Appearances up to the threshold do not quarantine.
        for i in 0..FLAP_THRESHOLD {
            assert!(
                !mons.note_set_appearance("k", now + Duration::from_millis(i as u64 * 10)),
                "should not quarantine at appearance {i}"
            );
        }
        // One more within the window trips it.
        assert!(mons.note_set_appearance("k", now + Duration::from_millis(100)));
    }

    #[test]
    fn flap_ignores_appearances_outside_window() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        // Spread appearances so old ones fall out of the window: never trips.
        for i in 0..FLAP_THRESHOLD + 3 {
            let t = now + FLAP_WINDOW * i;
            assert!(!mons.note_set_appearance("k", t));
        }
    }

    #[test]
    fn flap_quarantine_lifts_after_cooldown() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        for _ in 0..=FLAP_THRESHOLD {
            mons.note_set_appearance("k", now);
        }
        assert!(mons.flap_quarantined("k", now));
        // Still quarantined just before cooldown.
        assert!(mons.flap_quarantined("k", now + FLAP_COOLDOWN - Duration::from_millis(1)));
        // Lifted at/after cooldown.
        assert!(!mons.flap_quarantined("k", now + FLAP_COOLDOWN));
    }

    #[test]
    fn clear_flap_removes_quarantine() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        for _ in 0..=FLAP_THRESHOLD {
            mons.note_set_appearance("k", now);
        }
        assert!(mons.flap_quarantined("k", now));
        mons.clear_flap("k");
        assert!(!mons.flap_quarantined("k", now));
    }

    #[test]
    fn flap_reappearance_extends_quarantine() {
        let mut mons = Monitors::default();
        let now = Instant::now();
        for _ in 0..=FLAP_THRESHOLD {
            mons.note_set_appearance("k", now);
        }
        // A reappearance near the end of cooldown pushes the deadline out.
        let late = now + FLAP_COOLDOWN - Duration::from_millis(10);
        assert!(mons.note_set_appearance("k", late));
        // Original cooldown would have lifted; the extension keeps it quarantined.
        assert!(mons.flap_quarantined("k", now + FLAP_COOLDOWN));
    }

    #[test]
    fn apply_modes_ready_ignores_disabled_heads() {
        let mut heads = HashMap::new();
        let mut h = live("eDP-1", "Sharp", 1920, 1200, 0, 0);
        h.enabled = false;
        heads.insert(1, h);
        // A disabled head with no modes does not block readiness.
        assert!(heads_have_apply_modes(&heads));
    }
}

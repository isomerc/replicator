//! Selective settings-group operations over EVE's blue.Marshal files.
//!
//! Both `core_char_*.dat` and `core_user_*.dat` decode to a top-level
//! dict whose keys are already human-meaningful groups (`windows`,
//! `ui`, `overview`, `shiptheme`, ...). Selective copy decodes source
//! and target, replaces only the chosen groups in the target, and
//! re-encodes - so unchecked groups keep the target's own values.
//!
//! Format notes, verified against real files:
//! - EVE writes version-0 streams whose format carries a trailing
//!   shared-object index map; `blue-marshal` reads those (`consumed`
//!   stops before the map) and re-encodes as version 1, which needs no
//!   map. `Marshal.cpp` reads both versions natively.
//! - Re-encoded bytes are NOT byte-identical (shared references are
//!   inlined), so the safety invariant is semantic: the encoded output
//!   must decode back to exactly the merged value, or we refuse to
//!   write. Whole-file copies never go through this module and stay
//!   byte-perfect.

use crate::error::{AppError, AppResult};
use blue_marshal::{decode, encode, EncodeOptions, Value};
use serde::Serialize;

fn parse_err(what: &str, e: impl std::fmt::Debug) -> AppError {
    AppError::Other(format!("{what}: {e:?}"))
}

fn decode_top_dict(bytes: &[u8], what: &str) -> AppResult<Vec<(Value, Value)>> {
    let d = decode(bytes).map_err(|e| parse_err(what, e))?;
    match d.value {
        Value::Dict(items) => Ok(items),
        other => Err(AppError::Other(format!(
            "{what}: expected a settings dict at the top level, found {other:?}"
        ))),
    }
}

fn key_name(k: &Value) -> Option<String> {
    match k {
        Value::Str(s) => Some(s.clone()),
        Value::Bytes(b) => Some(String::from_utf8_lossy(b).to_string()),
        _ => None,
    }
}

/// Semantic value equality: derived PartialEq except that NaN equals
/// NaN. py2 marshal encodes NaN legally, and float-derived NaN != NaN
/// would make a faithful round trip look like a mismatch - locking
/// every selective operation out of NaN-bearing files and injecting
/// phantom groups into diffs.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::Tuple(x), Value::Tuple(y)) | (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| values_equal(a, b))
        }
        (Value::Dict(x), Value::Dict(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((ka, va), (kb, vb))| values_equal(ka, kb) && values_equal(va, vb))
        }
        _ => a == b,
    }
}

/// Key equality for diffing: string-like keys (Str and py2 byte-str)
/// compare by their text, matching how `selective_merge` and the
/// pickers address groups - a client-era change of key encoding must
/// not make the same group diff as removed + added. Non-string keys
/// fall back to semantic equality.
fn keys_match(a: &Value, b: &Value) -> bool {
    match (key_name(a), key_name(b)) {
        (Some(x), Some(y)) => x == y,
        _ => values_equal(a, b),
    }
}

/// The top-level group names of a settings file, in file order.
pub fn list_groups(bytes: &[u8]) -> AppResult<Vec<String>> {
    let items = decode_top_dict(bytes, "settings file")?;
    Ok(items.iter().filter_map(|(k, _)| key_name(k)).collect())
}

/// Merge the selected `groups` from `source` into `target` and return
/// the re-encoded target. For each selected group: present in source
/// means the target gets the source's value; absent in source means it
/// is removed from the target ("make the target match the source for
/// this group"). Every other group keeps the target's own value.
pub fn selective_merge(source: &[u8], target: &[u8], groups: &[String]) -> AppResult<Vec<u8>> {
    let src = decode_top_dict(source, "source settings")?;
    let mut tgt = decode_top_dict(target, "target settings")?;

    for g in groups {
        let src_entry = src
            .iter()
            .find(|(k, _)| key_name(k).as_deref() == Some(g.as_str()));
        let tgt_pos = tgt
            .iter()
            .position(|(k, _)| key_name(k).as_deref() == Some(g.as_str()));
        match (src_entry, tgt_pos) {
            (Some((k, v)), Some(pos)) => tgt[pos] = (k.clone(), v.clone()),
            (Some((k, v)), None) => tgt.push((k.clone(), v.clone())),
            (None, Some(pos)) => {
                tgt.remove(pos);
            }
            (None, None) => {}
        }
    }

    let merged = Value::Dict(tgt);
    let out = encode(&merged, &EncodeOptions::default()).map_err(|e| parse_err("re-encode", e))?;

    // The invariant that makes writing safe: what we are about to put
    // on disk decodes back to exactly what we meant.
    let check = decode(&out).map_err(|e| parse_err("round-trip decode", e))?;
    if !values_equal(&check.value, &merged) {
        return Err(AppError::Other(
            "settings round-trip mismatch after merge; refusing to write".into(),
        ));
    }
    Ok(out)
}

/// One top-level group's difference between two settings files.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct GroupDiff {
    pub group: String,
    /// "added" | "removed" | "changed" (relative to the old side).
    pub kind: String,
    /// When both sides hold a dict: how many inner entries differ.
    pub entries_changed: Option<usize>,
    pub entries_added: Option<usize>,
    pub entries_removed: Option<usize>,
    /// Names of the differing inner entries, when their keys are
    /// strings (window names, overview preset names, ...). Capped so a
    /// wholesale rewrite doesn't flood the UI.
    pub entry_names: Vec<String>,
}

/// How many differing inner-entry names to surface per group.
const ENTRY_NAME_CAP: usize = 8;

fn changed_group(group: String, old: &Value, new: &Value) -> GroupDiff {
    let mut d = GroupDiff {
        group,
        kind: "changed".into(),
        entries_changed: None,
        entries_added: None,
        entries_removed: None,
        entry_names: Vec::new(),
    };
    if let (Value::Dict(a), Value::Dict(b)) = (old, new) {
        let mut changed = 0usize;
        let mut added = 0usize;
        let mut removed = 0usize;
        let mut names: Vec<String> = Vec::new();
        let mut note = |k: &Value| {
            if names.len() < ENTRY_NAME_CAP {
                if let Some(n) = key_name(k) {
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        };
        for (k, va) in a {
            match b.iter().find(|(kb, _)| keys_match(kb, k)) {
                Some((_, vb)) if values_equal(vb, va) => {}
                Some(_) => {
                    changed += 1;
                    note(k);
                }
                None => {
                    removed += 1;
                    note(k);
                }
            }
        }
        for (k, _) in b {
            if !a.iter().any(|(ka, _)| keys_match(ka, k)) {
                added += 1;
                note(k);
            }
        }
        d.entries_changed = Some(changed);
        d.entries_added = Some(added);
        d.entries_removed = Some(removed);
        d.entry_names = names;
    }
    d
}

/// The semantic difference between two settings files, group by group:
/// what `new` has that `old` lacked, what it dropped, and what it
/// changed - with per-entry counts and names where the group is a dict
/// (which the interesting ones all are).
pub fn diff_settings(old: &[u8], new: &[u8]) -> AppResult<Vec<GroupDiff>> {
    let a = decode_top_dict(old, "old settings")?;
    let b = decode_top_dict(new, "new settings")?;

    let mut out = Vec::new();
    for (k, va) in &a {
        let Some(name) = key_name(k) else { continue };
        match b.iter().find(|(kb, _)| keys_match(kb, k)) {
            Some((_, vb)) if values_equal(vb, va) => {}
            Some((_, vb)) => out.push(changed_group(name, va, vb)),
            None => out.push(GroupDiff {
                group: name,
                kind: "removed".into(),
                entries_changed: None,
                entries_added: None,
                entries_removed: None,
                entry_names: Vec::new(),
            }),
        }
    }
    for (k, _) in &b {
        if !a.iter().any(|(ka, _)| keys_match(ka, k)) {
            if let Some(name) = key_name(k) {
                out.push(GroupDiff {
                    group: name,
                    kind: "added".into(),
                    entries_changed: None,
                    entries_added: None,
                    entries_removed: None,
                    entry_names: Vec::new(),
                });
            }
        }
    }
    Ok(out)
}

/// One window's place on screen, normalized to the 0..1 viewport it
/// was saved against.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct WindowRect {
    pub name: String,
    /// Other windows sharing this exact frame - a stack's members.
    /// Moving the drawn box means moving all of them.
    pub stacked: Vec<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// The viewport this window's rect was saved against - pixel
    /// floors must be normalized per window, not per layout, when a
    /// file carries mixed resolutions.
    pub vw: i64,
    pub vh: i64,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct WindowLayout {
    /// The client viewport the (most recently saved) rects refer to -
    /// kept for aspect ratio.
    pub screen_w: i64,
    pub screen_h: i64,
    pub windows: Vec<WindowRect>,
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        // Longs are bignums; window coordinates always fit, so a
        // string round-trip is fine and avoids a num-traits dep.
        Value::Long(l) => l.to_string().parse().ok(),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// Group entries are (timestamp, payload) tuples; older data is
/// sometimes bare. Either way, hand back the payload.
fn payload(v: &Value) -> &Value {
    match v {
        Value::Tuple(t) if t.len() == 2 => &t[1],
        other => other,
    }
}

fn bool_map(entries: &[(Value, Value)], key: &str) -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::new();
    if let Some((_, v)) = entries
        .iter()
        .find(|(k, _)| key_name(k).as_deref() == Some(key))
    {
        if let Value::Dict(rows) = payload(v) {
            for (rk, rv) in rows {
                if let (Some(name), Value::Bool(b)) = (key_name(rk), rv) {
                    out.insert(name, *b);
                }
            }
        }
    }
    out
}

/// Extract the on-screen window layout from a settings file, if it
/// records one. The schema (verified against live files): the
/// `windows` group holds `windowSizesAndPositions*` mapping window
/// name to `(x, y, w, h, screenW, screenH)`, alongside `openWindows`
/// and `minimizedWindows` bool maps. Only windows that are open and
/// not minimized - the ones actually on screen - are returned, each
/// normalized by the viewport it was saved against.
pub fn window_layout(bytes: &[u8]) -> AppResult<Option<WindowLayout>> {
    let items = decode_top_dict(bytes, "settings file")?;
    let Some((_, windows_v)) = items
        .iter()
        .find(|(k, _)| key_name(k).as_deref() == Some("windows"))
    else {
        return Ok(None);
    };
    let Value::Dict(entries) = windows_v else {
        return Ok(None);
    };

    let Some((_, sizes_v)) = entries
        .iter()
        .find(|(k, _)| key_name(k).is_some_and(|n| n.starts_with("windowSizesAndPositions")))
    else {
        return Ok(None);
    };
    let Value::Dict(rows) = payload(sizes_v) else {
        return Ok(None);
    };

    let open = bool_map(entries, "openWindows");
    let minimized = bool_map(entries, "minimizedWindows");

    let mut out = Vec::new();
    let mut screens: Vec<(i64, i64, usize)> = Vec::new();
    for (rk, rv) in rows {
        let Some(name) = key_name(rk) else { continue };
        // A window is drawn only if it is actually on screen. A file
        // with no openWindows map at all keeps everything (better a
        // busy sketch than a blank one).
        if open.get(&name) == Some(&false) || minimized.get(&name) == Some(&true) {
            continue;
        }
        if !open.is_empty() && !open.contains_key(&name) {
            continue;
        }
        let Value::Tuple(t) = payload(rv) else {
            continue;
        };
        if t.len() < 6 {
            continue;
        }
        let nums: Vec<f64> = t.iter().filter_map(as_f64).collect();
        if nums.len() < 6 {
            continue;
        }
        let (x, y, w, h, sw, sh) = (nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]);
        if w <= 0.0 || h <= 0.0 || sw <= 0.0 || sh <= 0.0 {
            continue;
        }
        match screens
            .iter_mut()
            .find(|(a, b, _)| *a == sw as i64 && *b == sh as i64)
        {
            Some((_, _, n)) => *n += 1,
            None => screens.push((sw as i64, sh as i64, 1)),
        }
        out.push(WindowRect {
            name,
            stacked: Vec::new(),
            x: x / sw,
            y: y / sh,
            w: w / sw,
            h: h / sh,
            vw: sw as i64,
            vh: sh as i64,
        });
    }
    if out.is_empty() {
        return Ok(None);
    }
    // Biggest windows first: SVG paints in order, so the small ones
    // land on top and stay visible.
    out.sort_by(|a, b| {
        (b.w * b.h)
            .partial_cmp(&(a.w * a.h))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Stacked windows share one frame on screen; ten identical rects
    // are one box, not ten - but the box remembers its members so a
    // designer move carries the whole stack.
    // The key includes the save viewport: two windows from different
    // resolutions can normalize to the same rect without ever having
    // shared a frame on screen.
    let q = |v: f64| (v * 10_000.0).round() as i64;
    let mut merged: Vec<WindowRect> = Vec::new();
    let mut index: std::collections::HashMap<(i64, i64, i64, i64, i64, i64), usize> =
        std::collections::HashMap::new();
    for r in out {
        let key = (q(r.x), q(r.y), q(r.w), q(r.h), r.vw, r.vh);
        match index.get(&key) {
            Some(&i) => merged[i].stacked.push(r.name),
            None => {
                index.insert(key, merged.len());
                merged.push(r);
            }
        }
    }
    let out = merged;
    let (screen_w, screen_h) = screens
        .iter()
        .max_by_key(|(_, _, n)| *n)
        .map(|(a, b, _)| (*a, *b))
        .unwrap_or((16, 9));
    Ok(Some(WindowLayout {
        screen_w,
        screen_h,
        windows: out,
    }))
}

/// Every window's recorded pixel size in a settings file, whether or
/// not the window is open. Fuel for the designer's size floors: EVE
/// never saves a window smaller than it allows, so the smallest size
/// ever observed for a name is a proven-achievable minimum.
pub fn geometry_sizes(bytes: &[u8]) -> AppResult<Vec<(String, i64, i64)>> {
    let items = decode_top_dict(bytes, "settings file")?;
    let Some((_, windows_v)) = items
        .iter()
        .find(|(k, _)| key_name(k).as_deref() == Some("windows"))
    else {
        return Ok(Vec::new());
    };
    let Value::Dict(entries) = windows_v else {
        return Ok(Vec::new());
    };
    let Some((_, sizes_v)) = entries
        .iter()
        .find(|(k, _)| key_name(k).is_some_and(|n| n.starts_with("windowSizesAndPositions")))
    else {
        return Ok(Vec::new());
    };
    let Value::Dict(rows) = payload(sizes_v) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (rk, rv) in rows {
        let Some(name) = key_name(rk) else { continue };
        let Value::Tuple(t) = payload(rv) else {
            continue;
        };
        let (Some(w), Some(h)) = (t.get(2).and_then(as_f64), t.get(3).and_then(as_f64)) else {
            continue;
        };
        if w > 0.0 && h > 0.0 {
            out.push((name, w as i64, h as i64));
        }
    }
    Ok(out)
}

/// Shipped default minimum sizes, measured by the min-size probe on
/// 2026-08-23: every window of a live character written to 1x1 px, the
/// client left to clamp them, the corrected sizes harvested from what
/// it saved (viewport 3317x1670). These are exact for that machine's
/// UI scale and treated as PRIORS elsewhere: the designer's floor is
/// the larger of this and the smallest size observed in the user's own
/// files, and a local calibration run replaces both.
const SHIPPED_FLOORS: &[(&str, i64, i64)] = &[
    ("ChatWindowStack", 188, 120),
    ("chatchannel_local", 132, 120),
    ("lobbyWnd", 270, 495),
    ("locations", 250, 210),
    ("mail", 650, 435),
    ("market", 650, 435),
    ("message", 420, 260),
    ("walletWindow", 650, 435),
];

/// The shipped default floor for a window name, if any. Exact names
/// first, then family rules (every chat channel shares the channel
/// floor). Pure-digit names are stack containers - their floor is
/// whatever their members demand, so no default applies.
pub fn shipped_floor(name: &str) -> Option<(i64, i64)> {
    if let Some((_, w, h)) = SHIPPED_FLOORS.iter().find(|(n, _, _)| *n == name) {
        return Some((*w, *h));
    }
    if name.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if name.starts_with("chatchannel_") {
        return Some((188, 120));
    }
    None
}

/// One window's new place, normalized 0..1 - straight from the
/// designer canvas.
#[derive(Debug, serde::Deserialize, Clone)]
pub struct LayoutChange {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Write a designed layout into a settings file: each change lands in
/// the `windowSizesAndPositions*` entry for its window, denormalized
/// into that window's own recorded viewport, preserving the timestamp
/// wrapper and the viewport fields untouched. Everything else in the
/// file - every other group, every other windows entry - keeps its
/// exact value. Same refusal rule as every write: the output must
/// decode back to precisely the intended value.
pub fn apply_layout(
    bytes: &[u8],
    changes: &[LayoutChange],
) -> AppResult<(Vec<u8>, usize, Vec<String>)> {
    let mut items = decode_top_dict(bytes, "settings file")?;
    let windows_v = items
        .iter_mut()
        .find(|(k, _)| key_name(k).as_deref() == Some("windows"))
        .map(|(_, v)| v)
        .ok_or_else(|| AppError::Other("this file has no windows group".into()))?;
    let Value::Dict(entries) = windows_v else {
        return Err(AppError::Other("windows group is not a dict".into()));
    };
    let sizes_v = entries
        .iter_mut()
        .find(|(k, _)| key_name(k).is_some_and(|n| n.starts_with("windowSizesAndPositions")))
        .map(|(_, v)| v)
        .ok_or_else(|| AppError::Other("this file records no window geometry".into()))?;
    let payload = match sizes_v {
        Value::Tuple(t) if t.len() == 2 => &mut t[1],
        other => other,
    };
    let Value::Dict(rows) = payload else {
        return Err(AppError::Other("window geometry is not a dict".into()));
    };

    let mut applied = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for ch in changes {
        let Some(cell) = rows
            .iter_mut()
            .find(|(k, _)| key_name(k).as_deref() == Some(ch.name.as_str()))
            .map(|(_, v)| v)
        else {
            skipped.push(ch.name.clone());
            continue;
        };
        let cell = match cell {
            Value::Tuple(t) if t.len() == 2 => &mut t[1],
            other => other,
        };
        let Value::Tuple(t) = cell else {
            skipped.push(ch.name.clone());
            continue;
        };
        let (Some(sw), Some(sh)) = (t.get(4).and_then(as_f64), t.get(5).and_then(as_f64)) else {
            skipped.push(ch.name.clone());
            continue;
        };
        if sw <= 0.0 || sh <= 0.0 || ch.w <= 0.0 || ch.h <= 0.0 {
            skipped.push(ch.name.clone());
            continue;
        }
        t[0] = Value::Int((ch.x * sw).round() as i64);
        t[1] = Value::Int((ch.y * sh).round() as i64);
        t[2] = Value::Int((ch.w * sw).round().max(1.0) as i64);
        t[3] = Value::Int((ch.h * sh).round().max(1.0) as i64);
        applied += 1;
    }

    let merged = Value::Dict(items);
    let out = encode(&merged, &EncodeOptions::default()).map_err(|e| parse_err("re-encode", e))?;
    let check = decode(&out).map_err(|e| parse_err("round-trip decode", e))?;
    if !values_equal(&check.value, &merged) {
        return Err(AppError::Other(
            "settings round-trip mismatch after layout write; refusing to write".into(),
        ));
    }
    Ok((out, applied, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: &[(&str, Value)]) -> Value {
        Value::Dict(
            entries
                .iter()
                .map(|(k, v)| (Value::Str((*k).to_string()), v.clone()))
                .collect(),
        )
    }

    fn file(entries: &[(&str, Value)]) -> Vec<u8> {
        encode(&dict(entries), &EncodeOptions::default()).unwrap()
    }

    fn groups_of(bytes: &[u8]) -> Vec<(String, Value)> {
        decode_top_dict(bytes, "test")
            .unwrap()
            .into_iter()
            .map(|(k, v)| (key_name(&k).unwrap(), v))
            .collect()
    }

    #[test]
    fn list_groups_returns_top_level_keys_in_order() {
        let f = file(&[
            ("windows", dict(&[("a", Value::Int(1))])),
            ("ui", Value::Dict(vec![])),
            ("overview", Value::Int(7)),
        ]);
        assert_eq!(list_groups(&f).unwrap(), vec!["windows", "ui", "overview"]);
    }

    #[test]
    fn list_groups_rejects_a_non_dict_file() {
        let f = encode(&Value::Int(42), &EncodeOptions::default()).unwrap();
        assert!(list_groups(&f).is_err());
    }

    #[test]
    fn list_groups_rejects_garbage() {
        assert!(list_groups(b"not a marshal stream at all").is_err());
    }

    #[test]
    fn merge_replaces_only_the_selected_groups() {
        let source = file(&[
            ("windows", dict(&[("pos", Value::Int(100))])),
            ("ui", dict(&[("theme", Value::Int(1))])),
        ]);
        let target = file(&[
            ("windows", dict(&[("pos", Value::Int(5))])),
            ("ui", dict(&[("theme", Value::Int(2))])),
            ("notepad", Value::Str("target's notes".into())),
        ]);

        let out = selective_merge(&source, &target, &["windows".to_string()]).unwrap();
        let got = groups_of(&out);

        assert_eq!(got[0].0, "windows");
        assert_eq!(
            got[0].1,
            dict(&[("pos", Value::Int(100))]),
            "selected: source wins"
        );
        assert_eq!(
            got[1].1,
            dict(&[("theme", Value::Int(2))]),
            "unselected: target keeps its own"
        );
        assert_eq!(
            got[2].1,
            Value::Str("target's notes".into()),
            "groups the source lacks survive when unselected"
        );
    }

    #[test]
    fn merge_adds_a_selected_group_the_target_lacks() {
        let source = file(&[("overview", Value::Int(9))]);
        let target = file(&[("ui", Value::Int(1))]);

        let out = selective_merge(&source, &target, &["overview".to_string()]).unwrap();
        let got = groups_of(&out);

        assert_eq!(got.len(), 2);
        assert_eq!(got[1], ("overview".to_string(), Value::Int(9)));
    }

    #[test]
    fn merge_removes_a_selected_group_absent_from_the_source() {
        // "Copy the overview" when the source has none means the target
        // ends up with none either - the group state is synced, not
        // merely overlaid.
        let source = file(&[("ui", Value::Int(1))]);
        let target = file(&[("ui", Value::Int(2)), ("overview", Value::Int(9))]);

        let out = selective_merge(&source, &target, &["overview".to_string()]).unwrap();

        assert_eq!(groups_of(&out).len(), 1);
        assert_eq!(list_groups(&out).unwrap(), vec!["ui"]);
    }

    #[test]
    fn merge_with_no_groups_is_a_faithful_rewrite_of_the_target() {
        let target = file(&[("ui", Value::Int(2)), ("windows", Value::Int(3))]);
        let source = file(&[("ui", Value::Int(1))]);

        let out = selective_merge(&source, &target, &[]).unwrap();

        assert_eq!(
            groups_of(&out),
            groups_of(&target),
            "no selection must change nothing semantically"
        );
    }

    #[test]
    fn merge_errors_on_an_unparseable_target_instead_of_writing() {
        let source = file(&[("ui", Value::Int(1))]);
        assert!(selective_merge(&source, b"garbage", &["ui".to_string()]).is_err());
    }

    // --------------------------- diff_settings ------------------------

    #[test]
    fn diff_reports_added_removed_and_changed_groups() {
        let old = file(&[
            ("windows", dict(&[("chat", Value::Int(1))])),
            ("ui", Value::Int(1)),
            ("notepad", Value::Str("x".into())),
        ]);
        let new = file(&[
            ("windows", dict(&[("chat", Value::Int(2))])),
            ("ui", Value::Int(1)),
            ("overview", Value::Int(9)),
        ]);

        let d = diff_settings(&old, &new).unwrap();

        assert_eq!(d.len(), 3);
        assert_eq!(
            (d[0].group.as_str(), d[0].kind.as_str()),
            ("windows", "changed")
        );
        assert_eq!(
            (d[1].group.as_str(), d[1].kind.as_str()),
            ("notepad", "removed")
        );
        assert_eq!(
            (d[2].group.as_str(), d[2].kind.as_str()),
            ("overview", "added")
        );
    }

    #[test]
    fn identical_files_diff_to_nothing() {
        let f = file(&[("windows", dict(&[("chat", Value::Int(1))]))]);
        assert!(diff_settings(&f, &f).unwrap().is_empty());
    }

    #[test]
    fn a_changed_dict_group_counts_and_names_its_entries() {
        let old = file(&[(
            "windows",
            dict(&[
                ("chatchannel", Value::Int(1)),
                ("overview", Value::Int(2)),
                ("dscan", Value::Int(3)),
            ]),
        )]);
        let new = file(&[(
            "windows",
            dict(&[
                ("chatchannel", Value::Int(99)), // changed
                ("overview", Value::Int(2)),     // same
                ("fitting", Value::Int(4)),      // added; dscan removed
            ]),
        )]);

        let d = diff_settings(&old, &new).unwrap();

        assert_eq!(d.len(), 1);
        assert_eq!(d[0].entries_changed, Some(1));
        assert_eq!(d[0].entries_added, Some(1));
        assert_eq!(d[0].entries_removed, Some(1));
        assert_eq!(d[0].entry_names, vec!["chatchannel", "dscan", "fitting"]);
    }

    #[test]
    fn entry_names_are_capped_but_counts_are_not() {
        let many_old: Vec<(String, Value)> =
            (0..20).map(|i| (format!("w{i}"), Value::Int(i))).collect();
        let many_new: Vec<(String, Value)> = (0..20)
            .map(|i| (format!("w{i}"), Value::Int(i + 100)))
            .collect();
        let as_dict = |entries: &[(String, Value)]| {
            Value::Dict(
                entries
                    .iter()
                    .map(|(k, v)| (Value::Str(k.clone()), v.clone()))
                    .collect(),
            )
        };
        let old = encode(
            &dict(&[("windows", as_dict(&many_old))]),
            &EncodeOptions::default(),
        )
        .unwrap();
        let new = encode(
            &dict(&[("windows", as_dict(&many_new))]),
            &EncodeOptions::default(),
        )
        .unwrap();

        let d = diff_settings(&old, &new).unwrap();

        assert_eq!(d[0].entries_changed, Some(20));
        assert_eq!(d[0].entry_names.len(), ENTRY_NAME_CAP);
    }

    #[test]
    fn a_changed_non_dict_group_carries_no_counts() {
        let old = file(&[("ui", Value::Int(1))]);
        let new = file(&[("ui", Value::Int(2))]);

        let d = diff_settings(&old, &new).unwrap();

        assert_eq!(d[0].kind, "changed");
        assert_eq!(d[0].entries_changed, None);
        assert!(d[0].entry_names.is_empty());
    }

    /// Dev probe against a real settings file, kept out of the normal
    /// run: REPLICATOR_PROBE=<file.dat> cargo test -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real settings file via REPLICATOR_PROBE"]
    fn probe_real_layout() {
        let Ok(path) = std::env::var("REPLICATOR_PROBE") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let layout = window_layout(&bytes).unwrap();
        println!("{layout:#?}");
    }

    // --------------------------- window_layout ------------------------

    /// A `windows` group shaped like the real files: entries are
    /// (timestamp, payload) tuples.
    fn windows_group(
        rects: &[(&str, [i64; 6])],
        open: &[(&str, bool)],
        minimized: &[(&str, bool)],
    ) -> Value {
        let stamp = |v: Value| Value::Tuple(vec![Value::Long(134_000_000.into()), v]);
        let rect_dict = Value::Dict(
            rects
                .iter()
                .map(|(n, r)| {
                    (
                        Value::Str((*n).to_string()),
                        Value::Tuple(r.iter().map(|i| Value::Int(*i)).collect()),
                    )
                })
                .collect(),
        );
        let bools = |pairs: &[(&str, bool)]| {
            Value::Dict(
                pairs
                    .iter()
                    .map(|(n, b)| (Value::Str((*n).to_string()), Value::Bool(*b)))
                    .collect(),
            )
        };
        dict(&[
            ("windowSizesAndPositions_1", stamp(rect_dict)),
            ("openWindows", stamp(bools(open))),
            ("minimizedWindows", stamp(bools(minimized))),
        ])
    }

    #[test]
    fn window_layout_extracts_normalized_open_windows() {
        let f = file(&[(
            "windows",
            windows_group(
                &[
                    ("overview", [960, 540, 480, 270, 1920, 1080]),
                    ("chat", [0, 0, 384, 540, 1920, 1080]),
                    ("closedWnd", [0, 0, 100, 100, 1920, 1080]),
                    ("minimizedWnd", [0, 0, 100, 100, 1920, 1080]),
                ],
                &[
                    ("overview", true),
                    ("chat", true),
                    ("closedWnd", false),
                    ("minimizedWnd", true),
                ],
                &[("minimizedWnd", true)],
            ),
        )]);

        let layout = window_layout(&f).unwrap().unwrap();

        assert_eq!(layout.screen_w, 1920);
        assert_eq!(layout.screen_h, 1080);
        assert_eq!(layout.windows.len(), 2);
        // Biggest first (chat: 384x540 > overview: 480x270).
        assert_eq!(layout.windows[0].name, "chat");
        let ov = &layout.windows[1];
        assert_eq!(ov.name, "overview");
        assert!((ov.x - 0.5).abs() < 1e-9);
        assert!((ov.y - 0.5).abs() < 1e-9);
        assert!((ov.w - 0.25).abs() < 1e-9);
        assert!((ov.h - 0.25).abs() < 1e-9);
    }

    #[test]
    fn window_layout_without_an_open_map_keeps_everything() {
        let f = file(&[(
            "windows",
            windows_group(&[("overview", [0, 0, 100, 100, 1000, 1000])], &[], &[]),
        )]);

        let layout = window_layout(&f).unwrap().unwrap();

        assert_eq!(layout.windows.len(), 1);
    }

    #[test]
    fn window_layout_is_none_without_geometry() {
        // No windows group at all.
        let f = file(&[("ui", Value::Int(1))]);
        assert_eq!(window_layout(&f).unwrap(), None);

        // A windows group with state but no sizes dict.
        let f = file(&[(
            "windows",
            dict(&[(
                "openWindows",
                Value::Tuple(vec![
                    Value::Long(1.into()),
                    dict(&[("x", Value::Bool(true))]),
                ]),
            )]),
        )]);
        assert_eq!(window_layout(&f).unwrap(), None);
    }

    #[test]
    fn window_layout_skips_degenerate_rects_and_errors_on_garbage() {
        let f = file(&[(
            "windows",
            windows_group(
                &[
                    ("zeroWidth", [0, 0, 0, 100, 1000, 1000]),
                    ("zeroScreen", [0, 0, 100, 100, 0, 0]),
                ],
                &[("zeroWidth", true), ("zeroScreen", true)],
                &[],
            ),
        )]);
        assert_eq!(window_layout(&f).unwrap(), None);

        assert!(window_layout(b"garbage").is_err());
    }

    #[test]
    fn geometry_sizes_lists_every_window_even_closed_ones() {
        let f = file(&[(
            "windows",
            windows_group(
                &[
                    ("overview", [0, 0, 400, 300, 2000, 1000]),
                    ("closedWnd", [0, 0, 150, 120, 2000, 1000]),
                ],
                &[("overview", true), ("closedWnd", false)],
                &[],
            ),
        )]);

        let sizes = geometry_sizes(&f).unwrap();

        assert_eq!(sizes.len(), 2, "closed windows count too: {sizes:?}");
        assert!(sizes.contains(&("closedWnd".to_string(), 150, 120)));
    }

    #[test]
    fn nan_bearing_files_merge_and_diff_cleanly() {
        // py2 marshal encodes NaN legally; derived float equality would
        // report NaN != NaN, locking every selective operation out of
        // such files and injecting phantom diffs.
        let f = file(&[("ui", dict(&[("angle", Value::Float(f64::NAN))]))]);

        let merged = selective_merge(&f, &f, &["ui".to_string()])
            .expect("a faithful NaN round trip must not be refused");
        assert!(
            diff_settings(&f, &merged).unwrap().is_empty(),
            "NaN against NaN is not a change"
        );
        assert!(diff_settings(&f, &f).unwrap().is_empty());
    }

    #[test]
    fn a_key_encoding_change_is_not_a_remove_plus_add() {
        // The decoder yields Bytes for py2 byte-str keys and Str for
        // unicode keys; the same group across that client-era change
        // must diff as the same group.
        let old = encode(
            &Value::Dict(vec![(Value::Bytes(b"windows".to_vec()), Value::Int(1))]),
            &EncodeOptions::default(),
        )
        .unwrap();
        let new = file(&[("windows", Value::Int(2))]);

        let d = diff_settings(&old, &new).unwrap();

        assert_eq!(d.len(), 1);
        assert_eq!(
            (d[0].group.as_str(), d[0].kind.as_str()),
            ("windows", "changed")
        );

        // And identical values across the encoding change diff to
        // nothing at all.
        let same = file(&[("windows", Value::Int(1))]);
        assert!(diff_settings(&old, &same).unwrap().is_empty());
    }

    #[test]
    fn windows_from_different_viewports_never_stack() {
        // (0,0,500,500)@1000x1000 and (0,0,960,540)@1920x1080 both
        // normalize to the same rect but were never one frame.
        let f = file(&[(
            "windows",
            windows_group(
                &[
                    ("a", [0, 0, 500, 500, 1000, 1000]),
                    ("b", [0, 0, 960, 540, 1920, 1080]),
                ],
                &[],
                &[],
            ),
        )]);

        let layout = window_layout(&f).unwrap().unwrap();

        assert_eq!(layout.windows.len(), 2, "{:?}", layout.windows);
        assert!(layout.windows.iter().all(|r| r.stacked.is_empty()));
        let a = layout.windows.iter().find(|r| r.name == "a").unwrap();
        assert_eq!((a.vw, a.vh), (1000, 1000));
    }

    #[test]
    fn shipped_floors_match_exact_names_families_and_nothing_else() {
        assert_eq!(shipped_floor("market"), Some((650, 435)));
        assert_eq!(shipped_floor("chatchannel_local"), Some((132, 120)));
        assert_eq!(
            shipped_floor("chatchannel_player_deadbeef"),
            Some((188, 120)),
            "unseen channels inherit the family floor"
        );
        assert_eq!(shipped_floor("153"), None, "stack ids have no prior");
        assert_eq!(shipped_floor("someFutureWindow"), None);
    }

    // --------------------------- apply_layout -------------------------

    fn change(name: &str, x: f64, y: f64, w: f64, h: f64) -> LayoutChange {
        LayoutChange {
            name: name.into(),
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn apply_layout_moves_a_window_in_its_own_viewport() {
        let f = file(&[(
            "windows",
            windows_group(
                &[("overview", [100, 100, 400, 300, 2000, 1000])],
                &[("overview", true)],
                &[],
            ),
        )]);

        let (out, applied, skipped) =
            apply_layout(&f, &[change("overview", 0.5, 0.25, 0.2, 0.5)]).unwrap();

        assert_eq!(applied, 1);
        assert!(skipped.is_empty());
        let layout = window_layout(&out).unwrap().unwrap();
        let r = &layout.windows[0];
        assert!((r.x - 0.5).abs() < 1e-3);
        assert!((r.y - 0.25).abs() < 1e-3);
        assert!((r.w - 0.2).abs() < 1e-3);
        assert!((r.h - 0.5).abs() < 1e-3);
        // The viewport reference frame is untouched.
        assert_eq!(layout.screen_w, 2000);
        assert_eq!(layout.screen_h, 1000);
    }

    #[test]
    fn apply_layout_touches_nothing_else() {
        let f = file(&[
            (
                "windows",
                windows_group(
                    &[
                        ("overview", [100, 100, 400, 300, 2000, 1000]),
                        ("chat", [0, 0, 200, 200, 2000, 1000]),
                    ],
                    &[("overview", true), ("chat", true)],
                    &[],
                ),
            ),
            ("ui", dict(&[("theme", Value::Int(7))])),
        ]);

        let (out, _, _) = apply_layout(&f, &[change("overview", 0.5, 0.5, 0.1, 0.1)]).unwrap();

        let d = diff_settings(&f, &out).unwrap();
        assert_eq!(d.len(), 1, "only the windows group changes: {d:?}");
        assert_eq!(d[0].group, "windows");
        assert_eq!(
            d[0].entry_names,
            vec!["windowSizesAndPositions_1"],
            "and inside it, only the geometry entry"
        );
    }

    #[test]
    fn apply_layout_preserves_the_timestamp_wrapper() {
        let f = file(&[(
            "windows",
            windows_group(&[("overview", [1, 1, 10, 10, 100, 100])], &[], &[]),
        )]);

        let (out, _, _) = apply_layout(&f, &[change("overview", 0.2, 0.2, 0.3, 0.3)]).unwrap();

        // Dig the wrapper back out: group -> (ts, dict) -> (ts, tuple).
        let items = decode_top_dict(&out, "t").unwrap();
        let Value::Dict(entries) = &items[0].1 else {
            panic!()
        };
        let Value::Tuple(sized) = &entries[0].1 else {
            panic!()
        };
        assert_eq!(sized[0], Value::Long(134_000_000.into()));
        let Value::Dict(rows) = &sized[1] else {
            panic!()
        };
        let Value::Tuple(cell) = &rows[0].1 else {
            panic!()
        };
        assert_eq!(cell.len(), 6);
        assert_eq!(cell[4], Value::Int(100), "viewport w preserved");
        assert_eq!(cell[5], Value::Int(100), "viewport h preserved");
    }

    #[test]
    fn apply_layout_skips_unknown_windows_and_reports_them() {
        let f = file(&[(
            "windows",
            windows_group(&[("overview", [1, 1, 10, 10, 100, 100])], &[], &[]),
        )]);

        let (_, applied, skipped) = apply_layout(
            &f,
            &[
                change("overview", 0.1, 0.1, 0.2, 0.2),
                change("ghostWindow", 0.5, 0.5, 0.1, 0.1),
            ],
        )
        .unwrap();

        assert_eq!(applied, 1);
        assert_eq!(skipped, vec!["ghostWindow"]);
    }

    #[test]
    fn apply_layout_refuses_files_without_geometry() {
        let no_windows = file(&[("ui", Value::Int(1))]);
        assert!(apply_layout(&no_windows, &[change("x", 0.1, 0.1, 0.1, 0.1)]).is_err());
        assert!(apply_layout(b"garbage", &[change("x", 0.1, 0.1, 0.1, 0.1)]).is_err());
    }

    #[test]
    fn stacked_windows_are_merged_with_their_members_recorded() {
        let f = file(&[(
            "windows",
            windows_group(
                &[
                    ("chatchannel_local", [0, 0, 300, 400, 1000, 1000]),
                    ("chatchannel_corp", [0, 0, 300, 400, 1000, 1000]),
                    ("overview", [500, 0, 300, 400, 1000, 1000]),
                ],
                &[],
                &[],
            ),
        )]);

        let layout = window_layout(&f).unwrap().unwrap();

        assert_eq!(layout.windows.len(), 2);
        let stack = layout
            .windows
            .iter()
            .find(|r| r.name.starts_with("chatchannel"))
            .unwrap();
        assert_eq!(stack.stacked.len(), 1);
    }

    #[test]
    fn diff_errors_on_garbage_instead_of_guessing() {
        let good = file(&[("ui", Value::Int(1))]);
        assert!(diff_settings(&good, b"garbage").is_err());
        assert!(diff_settings(b"garbage", &good).is_err());
    }

    #[test]
    fn merge_preserves_rich_nested_values() {
        // Exercise the value variety a real file carries.
        let deep = dict(&[
            (
                "list",
                Value::List(vec![Value::Int(1), Value::Str("x".into())]),
            ),
            (
                "tuple",
                Value::Tuple(vec![Value::Bool(true), Value::Float(1.5)]),
            ),
            ("bytes", Value::Bytes(vec![0x00, 0xFF, 0x80])),
        ]);
        let source = file(&[("windows", deep.clone())]);
        let target = file(&[("windows", Value::Dict(vec![]))]);

        let out = selective_merge(&source, &target, &["windows".to_string()]).unwrap();

        assert_eq!(groups_of(&out)[0].1, deep);
    }
}

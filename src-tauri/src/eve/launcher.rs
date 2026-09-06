use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Locate EVE launcher log directories across platforms. The launcher
/// writes JS-ish object dumps containing `userId: <id>, characterId: <id>`
/// pairs for every session it tracks - the only on-disk source of truth
/// for which character belongs to which local account.
pub fn discover_log_dirs() -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    let candidates: Vec<PathBuf> = if cfg!(target_os = "linux") {
        vec![
            home.join(".steam/steam/steamapps/compatdata/8500/pfx/drive_c/users/steamuser/AppData/Roaming/EVE Online/logs"),
            home.join(".local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/users/steamuser/AppData/Roaming/EVE Online/logs"),
            home.join(".steam/debian-installation/steamapps/compatdata/8500/pfx/drive_c/users/steamuser/AppData/Roaming/EVE Online/logs"),
            home.join(".var/app/com.valvesoftware.Steam/data/Steam/steamapps/compatdata/8500/pfx/drive_c/users/steamuser/AppData/Roaming/EVE Online/logs"),
        ]
    } else if cfg!(target_os = "windows") {
        vec![std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("EVE Online/logs")]
    } else if cfg!(target_os = "macos") {
        macos_log_dirs(&home)
    } else {
        Vec::new()
    };
    candidates.into_iter().filter(|p| p.is_dir()).collect()
}

/// The native Mac launcher logs to `~/Library/Logs/EVE Online` (CCP's
/// documented location). The Wine-wrapped launchers before it wrote
/// inside the p_drive; those paths stay for anyone still on one.
fn macos_log_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Logs/EVE Online"),
        home.join("Library/Application Support/EVE Online/p_drive/users/crossover/Application Data/EVE Online/logs"),
        home.join("Library/Application Support/EVE Online/p_drive/users/steamuser/AppData/Roaming/EVE Online/logs"),
    ]
}

/// Scan launcher logs in `dir` for (character_id -> user_id) pairs.
///
/// Only reads log files with mtime strictly newer than `since_mtime_secs`
/// (unix seconds). The user's logs can grow into the gigabytes, so this
/// is a critical bound - we parse only what's new since the last cached
/// pass. Pass `0` to read everything (initial scan).
///
/// As a safety net, regardless of `since_mtime_secs` we always also
/// parse the most recent log file (the launcher writes the active
/// session there).
///
/// Returns (pairs-with-source-mtime, newest_mtime_secs_seen).
/// A char->user pair plus the mtime of the log it came from, so merges
/// (across files here, across log dirs in the caller) can let the
/// newest evidence win regardless of iteration order.
pub type TimedPair = (i64, i64);

/// Record `char_id -> user_id` seen in a log with `mtime`, keeping the
/// pair from the newest log. `>=` so that within one file (one mtime)
/// later lines - chronologically later logins - win.
pub fn merge_pair(map: &mut HashMap<i64, TimedPair>, char_id: i64, user_id: i64, mtime: i64) {
    match map.get(&char_id) {
        Some((_, existing)) if *existing > mtime => {}
        _ => {
            map.insert(char_id, (user_id, mtime));
        }
    }
}

/// When resuming the active log from a stored offset, rewind this far
/// first: a userId line and its characterId line sit up to six lines
/// apart, and a previous read may have ended between them. Re-parsing
/// an overlap is free (`merge_pair` is idempotent); losing a pair that
/// straddled the boundary would not be.
const CURSOR_OVERLAP: u64 = 8 * 1024;

/// How much of the file's head the identity fingerprint covers.
const CURSOR_HEAD: u64 = 1024;

/// Where a previous scan of an active log stopped - plus a fingerprint
/// of the file's head, because rotation can put a brand-new file under
/// the same name (even at the same size) and an offset alone would
/// silently skip it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogCursor {
    pub offset: u64,
    pub head_len: u64,
    pub head_hash: u64,
}

/// Parse a log from `start` to EOF, streaming: the launcher's logs run
/// to gigabytes and must never be read into memory whole.
fn parse_file_from(
    path: &Path,
    start: u64,
    mtime: i64,
    map: &mut HashMap<i64, TimedPair>,
) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    parse_pairs_from_reader(f, mtime, map);
    Ok(())
}

/// FNV-1a over the first `len` bytes - identity, not integrity, so a
/// tiny non-cryptographic hash with no dependency is plenty.
fn head_fingerprint(path: &Path, len: u64) -> std::io::Result<u64> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(len).read_to_end(&mut buf)?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &buf {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(h)
}

/// Returns (pairs-with-source-mtime, newest_mtime_secs_seen).
///
/// `cursors` maps an active-log path to the byte offset already
/// parsed. The launcher appends to its active log forever - the file
/// the mtime watermark can never bound, because it is always the
/// newest. The cursor bounds it instead: only appended bytes are read,
/// an unchanged size reads nothing at all, and a shrunken file (log
/// rotation) resets to a full parse. Rotated-away files fall to the
/// ordinary mtime rule and are read once, whole.
pub fn extract_char_user_map(
    dir: &Path,
    since_mtime_secs: i64,
    cursors: &mut HashMap<String, LogCursor>,
) -> (HashMap<i64, TimedPair>, i64) {
    let mut map = HashMap::new();
    let mut newest_mtime = since_mtime_secs;
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return (map, newest_mtime),
    };

    // Collect (path, mtime, size) for all launcher logs.
    let mut entries: Vec<(PathBuf, i64, u64)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !fname.starts_with("eve-online-launcher") || !fname.ends_with(".log") {
            continue;
        }
        let (mtime, size) = entry
            .metadata()
            .map(|m| {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                (mtime, m.len())
            })
            .unwrap_or((0, 0));
        entries.push((path, mtime, size));
    }

    // Sort newest first so the always-included latest is at index 0.
    entries.sort_by_key(|(_, mtime, _)| std::cmp::Reverse(*mtime));

    for (i, (path, mtime, size)) in entries.iter().enumerate() {
        if i == 0 {
            // The active log: cursor-bounded. Only one cursor per dir
            // stays live - a rotation moves the active name, and the
            // old entry must not linger.
            let key = path.to_string_lossy().to_string();
            let dir_prefix = dir.to_string_lossy().to_string();
            cursors.retain(|k, _| !k.starts_with(&dir_prefix) || *k == key);

            // Same-file check: the stored head fingerprint must match,
            // or this is a different file wearing the old name
            // (rotation) and the cursor means nothing.
            let stored = cursors.get(&key).and_then(|c| {
                let hash = head_fingerprint(path, c.head_len).ok()?;
                (hash == c.head_hash && c.offset <= *size).then_some(c.offset)
            });
            let start = match stored {
                Some(offset) if offset == *size => None, // nothing new
                Some(offset) => Some(offset.saturating_sub(CURSOR_OVERLAP)),
                None => Some(0),
            };
            if let Some(start) = start {
                let _ = parse_file_from(path, start, *mtime, &mut map);
            }
            let head_len = (*size).min(CURSOR_HEAD);
            if let Ok(head_hash) = head_fingerprint(path, head_len) {
                cursors.insert(
                    key,
                    LogCursor {
                        offset: *size,
                        head_len,
                        head_hash,
                    },
                );
            } else {
                cursors.remove(&key);
            }
            if *mtime > newest_mtime {
                newest_mtime = *mtime;
            }
            continue;
        }
        // Everything else: skip if older than what we've already seen.
        if *mtime <= since_mtime_secs {
            continue;
        }
        if parse_file_from(path, 0, *mtime, &mut map).is_err() {
            continue;
        }
        if *mtime > newest_mtime {
            newest_mtime = *mtime;
        }
    }
    (map, newest_mtime)
}

/// A `userId:` line waits this many following lines for its
/// `characterId:`; the launcher writes them up to five lines apart.
const PAIR_WINDOW: usize = 5;

/// Stream `char_id -> user_id` pairs out of launcher-log text one line
/// at a time, so a multi-gigabyte log never has to fit in memory. Each
/// `userId:` line claims the first `characterId:` within the next
/// PAIR_WINDOW lines; one `characterId:` line settles every userId
/// still waiting, later ones winning on equal mtime. Lines are decoded
/// lossily, so a stray non-UTF-8 byte cannot void a whole log.
fn parse_pairs_from_reader<R: std::io::Read>(
    reader: R,
    mtime: i64,
    map: &mut HashMap<i64, TimedPair>,
) {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(reader);
    let mut buf: Vec<u8> = Vec::new();
    // (user_id, lines left in its window)
    let mut pending: Vec<(i64, usize)> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        if !pending.is_empty() {
            if let Some(char_id) = extract_int_after(&line, "characterId:") {
                for (user_id, _) in pending.drain(..) {
                    // The launcher emits `characterId: 1` as a
                    // placeholder before a character is selected - the
                    // wait ends, nothing is recorded.
                    if char_id > 1_000_000 {
                        merge_pair(map, char_id, user_id, mtime);
                    }
                }
            } else {
                for p in pending.iter_mut() {
                    p.1 -= 1;
                }
                pending.retain(|p| p.1 > 0);
            }
        }
        if let Some(user_id) = extract_int_after(&line, "userId:") {
            pending.push((user_id, PAIR_WINDOW));
        }
    }
}

/// Test convenience over in-memory text.
#[cfg(test)]
fn parse_pairs_into(content: &str, mtime: i64, map: &mut HashMap<i64, TimedPair>) {
    parse_pairs_from_reader(content.as_bytes(), mtime, map);
}

fn extract_int_after(line: &str, key: &str) -> Option<i64> {
    let idx = line.find(key)?;
    let tail = &line[idx + key.len()..];
    let digits: String = tail
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::set_mtime;
    use tempfile::TempDir;

    fn parse(s: &str) -> HashMap<i64, i64> {
        let mut map = HashMap::new();
        parse_pairs_into(s, 0, &mut map);
        strip(map)
    }

    /// Drop the mtimes for assertions that only care who owns whom.
    fn strip(map: HashMap<i64, TimedPair>) -> HashMap<i64, i64> {
        map.into_iter().map(|(c, (u, _))| (c, u)).collect()
    }

    fn write_log(dir: &Path, name: &str, contents: &str, mtime: u64) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        set_mtime(&p, mtime);
        p
    }

    fn block(user_id: i64, char_id: i64) -> String {
        format!(
            "  product: 'eve-online',\n  tenant: 'tranquility',\n  userId: {user_id},\n  characterId: {char_id},\n  profile: 'Default',\n"
        )
    }

    // ------------------------- extract_int_after ----------------------

    #[test]
    fn extract_int_after_reads_a_trailing_comma_delimited_value() {
        assert_eq!(
            extract_int_after("  userId: 4342021,", "userId:"),
            Some(4342021)
        );
    }

    #[test]
    fn extract_int_after_tolerates_missing_and_extra_whitespace() {
        assert_eq!(extract_int_after("userId:7", "userId:"), Some(7));
        assert_eq!(extract_int_after("userId:    7 ", "userId:"), Some(7));
    }

    #[test]
    fn extract_int_after_returns_none_when_the_key_is_absent() {
        assert_eq!(extract_int_after("nothing here", "userId:"), None);
    }

    #[test]
    fn extract_int_after_returns_none_for_a_non_numeric_value() {
        assert_eq!(extract_int_after("userId: null,", "userId:"), None);
        assert_eq!(extract_int_after("userId: 'abc',", "userId:"), None);
    }

    // -------------------------- parse_pairs_into ----------------------

    #[test]
    fn parses_sample_block() {
        let map = parse(&block(4342021, 2035047876));
        assert_eq!(map.get(&2035047876), Some(&4342021));
    }

    #[test]
    fn skips_placeholder_char_id() {
        let map = parse("userId: 32214835,\n  characterId: 1,");
        assert!(
            map.is_empty(),
            "the launcher emits characterId: 1 before a character is picked"
        );
    }

    #[test]
    fn skips_any_char_id_below_the_plausibility_floor() {
        let map = parse("userId: 32214835,\n  characterId: 999999,");
        assert!(map.is_empty(), "real EVE character ids are 8+ digits");
    }

    #[test]
    fn collects_every_session_block_in_one_file() {
        let mut content = block(1111111, 2035047876);
        content.push_str(&block(2222222, 2035047877));
        content.push_str(&block(1111111, 2035047878));

        let map = parse(&content);

        assert_eq!(map.len(), 3);
        assert_eq!(map.get(&2035047876), Some(&1111111));
        assert_eq!(map.get(&2035047877), Some(&2222222));
        assert_eq!(
            map.get(&2035047878),
            Some(&1111111),
            "one account can own several characters"
        );
    }

    #[test]
    fn a_later_block_wins_when_a_character_moves_accounts() {
        let mut content = block(1111111, 2035047876);
        content.push_str(&block(2222222, 2035047876));

        assert_eq!(parse(&content).get(&2035047876), Some(&2222222));
    }

    #[test]
    fn finds_character_id_up_to_five_lines_below_user_id() {
        let content = "userId: 4342021,\n a\n b\n c\n d\n characterId: 2035047876,";
        assert_eq!(parse(content).get(&2035047876), Some(&4342021));
    }

    #[test]
    fn ignores_a_character_id_beyond_the_search_window() {
        // Guards against pairing a userId with an unrelated characterId
        // from a different block further down the log.
        let content = "userId: 4342021,\n a\n b\n c\n d\n e\n f\n characterId: 2035047876,";
        assert!(parse(content).is_empty());
    }

    #[test]
    fn stops_at_the_first_character_id_after_a_user_id() {
        let content = "userId: 1111111,\n characterId: 2035047876,\n characterId: 2035047899,";
        let map = parse(content);
        assert_eq!(map.get(&2035047876), Some(&1111111));
        assert_eq!(map.get(&2035047899), None);
    }

    #[test]
    fn a_user_id_with_no_following_character_id_yields_nothing() {
        assert!(parse("userId: 4342021,\n  profile: 'Default',").is_empty());
    }

    #[test]
    fn empty_and_garbage_input_is_handled() {
        assert!(parse("").is_empty());
        assert!(parse("\u{0}\u{1}not a log at all\n\n").is_empty());
    }

    // ------------------------ extract_char_user_map -------------------

    #[test]
    fn scans_launcher_logs_in_a_directory() {
        let tmp = TempDir::new().unwrap();
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(4342021, 2035047876),
            1000,
        );

        let (map, newest) = extract_char_user_map(tmp.path(), 0, &mut HashMap::new());
        let map = strip(map);

        assert_eq!(map.get(&2035047876), Some(&4342021));
        assert_eq!(newest, 1000);
    }

    #[test]
    fn ignores_files_that_are_not_launcher_logs() {
        let tmp = TempDir::new().unwrap();
        write_log(tmp.path(), "game.log", &block(4342021, 2035047876), 1000);
        write_log(
            tmp.path(),
            "eve-online-launcher.txt",
            &block(4342021, 2035047877),
            1000,
        );

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut HashMap::new());
        let map = strip(map);

        assert!(map.is_empty());
    }

    #[test]
    fn skips_logs_older_than_the_watermark() {
        // Logs reach gigabytes; re-reading them on every copy is the
        // difference between an instant copy and a multi-second stall.
        let tmp = TempDir::new().unwrap();
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(1111111, 2035047876),
            5000,
        );
        write_log(
            tmp.path(),
            "eve-online-launcher.1.log",
            &block(2222222, 2035047877),
            1000,
        );

        let (map, newest) = extract_char_user_map(tmp.path(), 4000, &mut HashMap::new());
        let map = strip(map);

        assert_eq!(map.get(&2035047876), Some(&1111111));
        assert_eq!(
            map.get(&2035047877),
            None,
            "the stale log should not have been re-read"
        );
        assert_eq!(newest, 5000);
    }

    #[test]
    fn always_rereads_the_newest_log_even_when_not_newer_than_the_watermark() {
        // The launcher appends to the active log without necessarily
        // bumping mtime past our last watermark, so the newest file is
        // unconditionally re-parsed.
        let tmp = TempDir::new().unwrap();
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(1111111, 2035047876),
            1000,
        );

        let (map, _) = extract_char_user_map(tmp.path(), 999_999, &mut HashMap::new());
        let map = strip(map);

        assert_eq!(map.get(&2035047876), Some(&1111111));
    }

    #[test]
    fn watermark_never_moves_backwards() {
        let tmp = TempDir::new().unwrap();
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(1111111, 2035047876),
            1000,
        );

        let (_, newest) = extract_char_user_map(tmp.path(), 9000, &mut HashMap::new());

        assert_eq!(newest, 9000, "a stale log must not rewind the watermark");
    }

    #[test]
    fn merges_pairs_across_several_fresh_logs() {
        let tmp = TempDir::new().unwrap();
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(1111111, 2035047876),
            5000,
        );
        write_log(
            tmp.path(),
            "eve-online-launcher.1.log",
            &block(2222222, 2035047877),
            4000,
        );

        let (map, newest) = extract_char_user_map(tmp.path(), 0, &mut HashMap::new());
        let map = strip(map);

        assert_eq!(map.len(), 2);
        assert_eq!(newest, 5000);
    }

    // ------------------------- cursor behavior ------------------------

    #[test]
    fn the_active_log_is_read_incrementally_via_its_cursor() {
        let tmp = TempDir::new().unwrap();
        let path = write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(1111111, 2035047876),
            1000,
        );
        let mut cursors = HashMap::new();

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut cursors);
        assert_eq!(strip(map).get(&2035047876), Some(&1111111));
        let size = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            cursors.values().map(|c| c.offset).collect::<Vec<_>>(),
            vec![size],
            "the cursor lands at EOF"
        );

        // Nothing appended: a rescan finds nothing new (and, per the
        // size fast path, reads nothing at all).
        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut cursors);
        assert!(map.is_empty(), "unchanged file yields no pairs: {map:?}");

        // Append a new session block: only the appended pair surfaces.
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str(&block(2222222, 2035047999));
        std::fs::write(&path, &content).unwrap();
        crate::testutil::set_mtime(&path, 2000);

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut cursors);
        let map = strip(map);
        assert_eq!(map.get(&2035047999), Some(&2222222));
        assert_eq!(
            cursors.values().map(|c| c.offset).collect::<Vec<_>>(),
            vec![std::fs::metadata(&path).unwrap().len()]
        );
    }

    #[test]
    fn a_pair_straddling_the_read_boundary_is_rescued_by_the_overlap() {
        // The launcher can flush mid-block: a scan may end right after
        // a userId line, with its characterId arriving later. The
        // resume must rewind far enough to reform the pair - and no
        // further: a pair buried deep in already-parsed content must
        // NOT resurface, or the read wasn't incremental at all.
        let tmp = TempDir::new().unwrap();
        let mut content = block(1111111, 2035047876); // deep in the file
        content.push_str(&"noise line, nothing to see\n".repeat(600)); // ~16KB
        content.push_str("userId: 5555555,\n"); // dangling at EOF
        let path = write_log(tmp.path(), "eve-online-launcher.log", &content, 1000);
        let mut cursors = HashMap::new();

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut cursors);
        let map = strip(map);
        assert_eq!(map.get(&2035047876), Some(&1111111));
        assert_eq!(map.get(&2035048000), None, "no characterId yet");

        // The other half of the pair arrives.
        content.push_str("  characterId: 2035048000,\n");
        std::fs::write(&path, &content).unwrap();
        crate::testutil::set_mtime(&path, 2000);

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut cursors);
        let map = strip(map);

        assert_eq!(
            map.get(&2035048000),
            Some(&5555555),
            "the overlap rewind must reform the boundary-straddling pair"
        );
        assert_eq!(
            map.get(&2035047876),
            None,
            "the deep pair must not resurface - proof the read was incremental"
        );
    }

    #[test]
    fn a_shrunken_active_log_resets_to_a_full_parse() {
        // Rotation can replace the active log with a fresh, smaller
        // file under the same name; a stale cursor past EOF must not
        // blind the parse.
        let tmp = TempDir::new().unwrap();
        let path = write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &format!("{}{}", block(1, 2), block(1111111, 2035047876).repeat(20)),
            1000,
        );
        let mut cursors = HashMap::new();
        extract_char_user_map(tmp.path(), 0, &mut cursors);

        // Replaced with something much smaller.
        std::fs::write(&path, block(3333333, 2035047901)).unwrap();
        crate::testutil::set_mtime(&path, 2000);

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut cursors);

        assert_eq!(strip(map).get(&2035047901), Some(&3333333));
    }

    #[test]
    fn rotation_moves_the_cursor_to_the_new_active_log() {
        let tmp = TempDir::new().unwrap();
        let active = write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(1111111, 2035047876),
            1000,
        );
        let mut cursors = HashMap::new();
        extract_char_user_map(tmp.path(), 0, &mut cursors);

        // Rotate: the old active becomes .1.log, a new active appears.
        let rotated = tmp.path().join("eve-online-launcher.1.log");
        std::fs::rename(&active, &rotated).unwrap();
        crate::testutil::set_mtime(&rotated, 1500);
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(2222222, 2035047999),
            2000,
        );

        let (map, _) = extract_char_user_map(tmp.path(), 1000, &mut cursors);

        let map = strip(map);
        assert_eq!(
            map.get(&2035047999),
            Some(&2222222),
            "the new active log is parsed from zero"
        );
        assert_eq!(cursors.len(), 1, "the stale cursor is pruned: {cursors:?}");
        assert!(cursors
            .keys()
            .next()
            .unwrap()
            .ends_with("eve-online-launcher.log"));
    }

    #[test]
    fn the_newest_log_wins_when_a_character_moved_accounts() {
        // Regression: files used to be parsed newest-first with
        // last-write-wins inserts, so an older rotated log would
        // overwrite the newest evidence - and the wrong account's
        // core_user file would then be used as a copy source.
        let tmp = TempDir::new().unwrap();
        write_log(
            tmp.path(),
            "eve-online-launcher.1.log",
            &block(1111111, 2035047876), // old home account
            1000,
        );
        write_log(
            tmp.path(),
            "eve-online-launcher.log",
            &block(2222222, 2035047876), // where the character lives now
            5000,
        );

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut HashMap::new());
        let map = strip(map);

        assert_eq!(
            map.get(&2035047876),
            Some(&2222222),
            "the newest log's account must win regardless of parse order"
        );
    }

    #[test]
    fn a_log_with_a_bad_byte_still_yields_its_pairs() {
        // Rotated logs used to go through read_to_string, which fails
        // the whole file on one invalid UTF-8 byte; lossy line decoding
        // keeps every pair.
        let tmp = TempDir::new().unwrap();
        let mut bytes = block(1111111, 2035047876).into_bytes();
        bytes.extend_from_slice(b"  profile: '\xff\xfe',\n");
        bytes.extend_from_slice(block(2222222, 2035047877).as_bytes());
        let rotated = tmp.path().join("eve-online-launcher.1.log");
        std::fs::write(&rotated, &bytes).unwrap();
        set_mtime(&rotated, 1000);
        // A newer, empty active log makes the bad-byte file a rotated one.
        write_log(tmp.path(), "eve-online-launcher.log", "", 2000);

        let (map, _) = extract_char_user_map(tmp.path(), 0, &mut HashMap::new());
        let map = strip(map);

        assert_eq!(map.get(&2035047876), Some(&1111111));
        assert_eq!(map.get(&2035047877), Some(&2222222));
    }

    #[test]
    fn a_missing_log_dir_yields_nothing_and_preserves_the_watermark() {
        let tmp = TempDir::new().unwrap();

        let (map, newest) =
            extract_char_user_map(&tmp.path().join("nope"), 42, &mut HashMap::new());
        let map = strip(map);

        assert!(map.is_empty());
        assert_eq!(newest, 42);
    }

    #[test]
    fn discover_log_dirs_only_returns_existing_directories() {
        for d in discover_log_dirs() {
            assert!(d.is_dir());
        }
    }

    #[test]
    fn macos_reads_the_native_launchers_log_folder_first() {
        // Only the Wine-era p_drive paths were listed, so a current Mac
        // never learned a single char->user pair.
        let dirs = macos_log_dirs(Path::new("/Users/pilot"));
        assert_eq!(dirs[0], Path::new("/Users/pilot/Library/Logs/EVE Online"));
        assert!(dirs
            .iter()
            .skip(1)
            .all(|p| p.to_string_lossy().contains("p_drive")));
    }
}

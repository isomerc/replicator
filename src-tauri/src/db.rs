use crate::error::{AppError, AppResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

pub struct Db {
    conn: Connection,
}

/// (char bytes, optional account-level user bytes) of a stored
/// template.
pub type TemplateBlobs = (Vec<u8>, Option<Vec<u8>>);

/// SQLite's raw UNIQUE-constraint message reaches the UI banner
/// verbatim; turn it into something a person can act on.
fn friendly_unique(e: rusqlite::Error, friendly: &str) -> AppError {
    if e.to_string().contains("UNIQUE constraint failed") {
        AppError::Config(friendly.to_string())
    } else {
        e.into()
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub member_ids: Vec<i64>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct AccountMeta {
    pub user_id: i64,
    pub alias: Option<String>,
    pub note: Option<String>,
}

/// One char->account pairing as the accounts view needs it: who the
/// character is filed under, whether that was set by hand, and what
/// the launcher logs last said (when they ever said anything).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharPairing {
    pub user_id: i64,
    pub manual: bool,
    pub log_user_id: Option<i64>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Template {
    pub id: i64,
    pub name: String,
    pub source_character_id: Option<i64>,
    pub source_name: Option<String>,
    pub created_at: i64,
    pub size: i64,
    /// Whether the template captured the account-level user file -
    /// the one that holds window positions, overview and chat. False
    /// for templates saved without a launcher-log mapping (or from
    /// before the user file was captured at all); the UI warns on
    /// these, because applying them moves none of the layout.
    pub has_user_data: bool,
    /// The server the source character's settings dir belonged to when
    /// the template was saved. Apply stays on it unless told to cross,
    /// the same rule as a copy. None for older templates and for
    /// sources with no recognizable server; those apply anywhere.
    pub source_server: Option<String>,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut me = Self { conn };
        me.migrate()?;
        Ok(me)
    }

    fn migrate(&mut self) -> AppResult<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS name_cache (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              category TEXT NOT NULL,
              fetched_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS groups (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              name TEXT UNIQUE NOT NULL
            );
            CREATE TABLE IF NOT EXISTS group_members (
              group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
              character_id INTEGER NOT NULL,
              PRIMARY KEY (group_id, character_id)
            );
            CREATE TABLE IF NOT EXISTS templates (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              name TEXT UNIQUE NOT NULL,
              source_character_id INTEGER,
              data BLOB NOT NULL,
              user_data BLOB,
              created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS char_user_map (
              character_id INTEGER PRIMARY KEY,
              user_id INTEGER NOT NULL,
              updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS account_meta (
              user_id INTEGER PRIMARY KEY,
              alias TEXT,
              note TEXT
            );
            CREATE TABLE IF NOT EXISTS window_floors (
              name TEXT PRIMARY KEY,
              min_w INTEGER NOT NULL,
              min_h INTEGER NOT NULL
            );
            "#,
        )?;

        // `user_data` was added after templates shipped; CREATE IF NOT
        // EXISTS leaves an existing table alone, so older databases
        // need the column bolted on.
        if !self.has_column("templates", "user_data")? {
            self.conn
                .execute("ALTER TABLE templates ADD COLUMN user_data BLOB", [])?;
        }
        // `source` distinguishes launcher-log pairings from ones the
        // user set by hand; pre-existing rows can only have come from
        // the logs, so the default backfills them correctly.
        if !self.has_column("char_user_map", "source")? {
            self.conn.execute(
                "ALTER TABLE char_user_map ADD COLUMN source TEXT NOT NULL DEFAULT 'log'",
                [],
            )?;
        }
        // `log_user_id` keeps recording what the logs say even after
        // the user pins a pairing by hand, so a hand-set row that the
        // logs contradict can be flagged instead of staying silently
        // wrong. For log rows the two columns are the same thing.
        if !self.has_column("char_user_map", "log_user_id")? {
            self.conn.execute(
                "ALTER TABLE char_user_map ADD COLUMN log_user_id INTEGER",
                [],
            )?;
            self.conn.execute(
                "UPDATE char_user_map SET log_user_id = user_id WHERE source = 'log'",
                [],
            )?;
        }
        // `source_server` lets apply stay on the source's server the way
        // copy does; older templates carry NULL and go anywhere.
        if !self.has_column("templates", "source_server")? {
            self.conn
                .execute("ALTER TABLE templates ADD COLUMN source_server TEXT", [])?;
        }
        Ok(())
    }

    fn has_column(&self, table: &str, column: &str) -> AppResult<bool> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if row.get::<_, String>(1)? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> AppResult<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn upsert_names(&self, names: &[crate::eve::esi::EsiName], now: i64) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO name_cache (id, name, category, fetched_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                   name=excluded.name, category=excluded.category, fetched_at=excluded.fetched_at",
            )?;
            for n in names {
                stmt.execute(params![n.id, n.name, n.category, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn lookup_names(&self, ids: &[i64]) -> AppResult<std::collections::HashMap<i64, String>> {
        let mut out = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        // Tombstones (ids ESI could not resolve) are cached rows too,
        // but they carry no name - the UI must keep rendering those
        // characters by id.
        let sql = format!(
            "SELECT id, name FROM name_cache
             WHERE category != 'unresolvable' AND id IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            out.insert(id, name);
        }
        Ok(out)
    }

    /// Every id whose cache entry is still fresh (`fetched_at >=
    /// fresh_since`) - including unresolvable tombstones - so
    /// resolve_names can decide what needs a network fetch. Entries
    /// older than the cutoff count as absent and get re-resolved,
    /// which is how character renames eventually propagate; the stale
    /// name keeps displaying until the refetch lands (lookup_names has
    /// no cutoff on purpose).
    pub fn cached_ids(
        &self,
        ids: &[i64],
        fresh_since: i64,
    ) -> AppResult<std::collections::HashSet<i64>> {
        let mut out = std::collections::HashSet::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql =
            format!("SELECT id FROM name_cache WHERE fetched_at >= ? AND id IN ({placeholders})");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&fresh_since];
        params.extend(ids.iter().map(|i| i as &dyn rusqlite::ToSql));
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))?;
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    }

    pub fn list_groups(&self) -> AppResult<Vec<Group>> {
        let mut groups: Vec<Group> = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name FROM groups ORDER BY name")?;
            let rows = stmt.query_map([], |r| {
                Ok(Group {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    member_ids: Vec::new(),
                })
            })?;
            for r in rows {
                groups.push(r?);
            }
        }
        for g in &mut groups {
            let mut stmt = self.conn.prepare(
                "SELECT character_id FROM group_members WHERE group_id = ?1 ORDER BY character_id",
            )?;
            let rows = stmt.query_map([g.id], |r| r.get::<_, i64>(0))?;
            for r in rows {
                g.member_ids.push(r?);
            }
        }
        Ok(groups)
    }

    pub fn create_group(&self, name: &str) -> AppResult<i64> {
        self.conn
            .execute("INSERT INTO groups (name) VALUES (?1)", params![name])
            .map_err(|e| friendly_unique(e, &format!("a group named \"{name}\" already exists")))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_group(&self, id: i64) -> AppResult<()> {
        self.conn
            .execute("DELETE FROM groups WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_group_members(&self, id: i64, member_ids: &[i64]) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM group_members WHERE group_id = ?1", params![id])?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO group_members (group_id, character_id) VALUES (?1, ?2)")?;
            for cid in member_ids {
                stmt.execute(params![id, cid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn group_member_ids(&self, id: i64) -> AppResult<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT character_id FROM group_members WHERE group_id = ?1")?;
        let rows = stmt.query_map([id], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_templates(&self) -> AppResult<Vec<Template>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, t.source_character_id, n.name, t.created_at, length(t.data),
                    t.user_data IS NOT NULL, t.source_server
             FROM templates t
             LEFT JOIN name_cache n ON n.id = t.source_character_id
             ORDER BY t.created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Template {
                id: r.get(0)?,
                name: r.get(1)?,
                source_character_id: r.get(2)?,
                source_name: r.get(3)?,
                created_at: r.get(4)?,
                size: r.get(5)?,
                has_user_data: r.get(6)?,
                source_server: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn create_template(
        &self,
        name: &str,
        source_character_id: Option<i64>,
        data: &[u8],
        user_data: Option<&[u8]>,
        now: i64,
        source_server: Option<&str>,
    ) -> AppResult<i64> {
        self.conn
            .execute(
                "INSERT INTO templates (name, source_character_id, data, user_data, created_at, source_server)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![name, source_character_id, data, user_data, now, source_server],
            )
            .map_err(|e| {
                friendly_unique(e, &format!("a template named \"{name}\" already exists"))
            })?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_template(&self, id: i64) -> AppResult<()> {
        self.conn
            .execute("DELETE FROM templates WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Store or clear an account's alias and note. Blank fields
    /// normalize away, and a row with nothing left in it is deleted so
    /// the table only ever holds accounts the user actually annotated.
    pub fn set_account_meta(
        &self,
        user_id: i64,
        alias: Option<&str>,
        note: Option<&str>,
    ) -> AppResult<()> {
        let alias = alias.map(str::trim).filter(|s| !s.is_empty());
        let note = note.map(str::trim).filter(|s| !s.is_empty());
        if alias.is_none() && note.is_none() {
            self.conn.execute(
                "DELETE FROM account_meta WHERE user_id = ?1",
                params![user_id],
            )?;
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO account_meta (user_id, alias, note) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET alias = excluded.alias, note = excluded.note",
            params![user_id, alias, note],
        )?;
        Ok(())
    }

    pub fn list_account_meta(&self) -> AppResult<Vec<AccountMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT user_id, alias, note FROM account_meta")?;
        let rows = stmt.query_map([], |r| {
            Ok(AccountMeta {
                user_id: r.get(0)?,
                alias: r.get(1)?,
                note: r.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Calibrated per-window minimum sizes, measured by letting the
    /// game clamp deliberately-tiny windows (the min-size probe).
    /// Ground truth for this machine's UI scale; the designer prefers
    /// these over floors merely observed in saved files. Written only
    /// by the probe today, hence test-gated.
    #[cfg(test)]
    pub fn set_window_floors(&self, floors: &[(String, i64, i64)]) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO window_floors (name, min_w, min_h) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET
                   min_w = MIN(window_floors.min_w, excluded.min_w),
                   min_h = MIN(window_floors.min_h, excluded.min_h)",
            )?;
            for (name, w, h) in floors {
                stmt.execute(params![name, w, h])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_window_floors(&self) -> AppResult<Vec<(String, i64, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, min_w, min_h FROM window_floors")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn upsert_char_user_pairs(
        &self,
        pairs: &std::collections::HashMap<i64, i64>,
        now: i64,
    ) -> AppResult<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            // A hand-set pairing is a deliberate user decision; the
            // background log parse never stomps it. (The newest log is
            // re-read on every refresh, so a log-wins rule would keep
            // resurrecting exactly the stale pair the user corrected.)
            // It does keep the log's own answer on record, though, so
            // a hand-set row the logs contradict can be flagged.
            let mut stmt = tx.prepare(
                "INSERT INTO char_user_map (character_id, user_id, updated_at, source, log_user_id)
                 VALUES (?1, ?2, ?3, 'log', ?2)
                 ON CONFLICT(character_id) DO UPDATE SET
                   log_user_id = excluded.log_user_id,
                   user_id = CASE WHEN char_user_map.source = 'manual'
                                  THEN char_user_map.user_id ELSE excluded.user_id END,
                   updated_at = CASE WHEN char_user_map.source = 'manual'
                                     THEN char_user_map.updated_at ELSE excluded.updated_at END",
            )?;
            for (char_id, user_id) in pairs {
                stmt.execute(params![char_id, user_id, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Pair a character with an account by hand. Overwrites whatever
    /// is there - log-learned or a previous manual choice.
    pub fn set_char_user_manual(&self, character_id: i64, user_id: i64, now: i64) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO char_user_map (character_id, user_id, updated_at, source)
             VALUES (?1, ?2, ?3, 'manual')
             ON CONFLICT(character_id) DO UPDATE SET
               user_id = excluded.user_id,
               updated_at = excluded.updated_at,
               source = 'manual'",
            params![character_id, user_id, now],
        )?;
        Ok(())
    }

    /// Remove a hand-set pairing, handing control back to the logs
    /// literally: when the logs have an answer on record the row
    /// becomes that log pairing; otherwise it goes away. Log-learned
    /// rows are left alone - they record something that actually
    /// happened.
    pub fn clear_char_user_manual(&self, character_id: i64, now: i64) -> AppResult<()> {
        let reverted = self.conn.execute(
            "UPDATE char_user_map
             SET user_id = log_user_id, source = 'log', updated_at = ?2
             WHERE character_id = ?1 AND source = 'manual' AND log_user_id IS NOT NULL",
            params![character_id, now],
        )?;
        if reverted == 0 {
            self.conn.execute(
                "DELETE FROM char_user_map WHERE character_id = ?1 AND source = 'manual'",
                params![character_id],
            )?;
        }
        Ok(())
    }

    /// Like `lookup_user_ids`, but each hit also carries whether it was
    /// set by hand and what the logs last said - the accounts view
    /// marks hand-set rows as editable and flags the ones the logs
    /// contradict.
    pub fn lookup_user_pairs(
        &self,
        char_ids: &[i64],
    ) -> AppResult<std::collections::HashMap<i64, CharPairing>> {
        let mut out = std::collections::HashMap::new();
        if char_ids.is_empty() {
            return Ok(out);
        }
        let placeholders = std::iter::repeat("?")
            .take(char_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT character_id, user_id, source, log_user_id FROM char_user_map
             WHERE character_id IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            char_ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                CharPairing {
                    user_id: r.get(1)?,
                    manual: r.get::<_, String>(2)? == "manual",
                    log_user_id: r.get(3)?,
                },
            ))
        })?;
        for row in rows {
            let (char_id, pairing) = row?;
            out.insert(char_id, pairing);
        }
        Ok(out)
    }

    pub fn lookup_user_ids(
        &self,
        char_ids: &[i64],
    ) -> AppResult<std::collections::HashMap<i64, i64>> {
        let mut out = std::collections::HashMap::new();
        if char_ids.is_empty() {
            return Ok(out);
        }
        let placeholders = std::iter::repeat("?")
            .take(char_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT character_id, user_id FROM char_user_map WHERE character_id IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            char_ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for r in rows {
            let (c, u) = r?;
            out.insert(c, u);
        }
        Ok(out)
    }

    /// The stored char bytes and, when captured, the account-level
    /// user bytes for a template.
    pub fn template_blobs(&self, id: i64) -> AppResult<Option<TemplateBlobs>> {
        Ok(self
            .conn
            .query_row(
                "SELECT data, user_data FROM templates WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eve::esi::EsiName;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// A real on-disk SQLite file rather than `:memory:` - `open` also
    /// sets WAL and foreign-key pragmas, and those only behave like
    /// production against a file.
    fn db() -> (TempDir, Db) {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("replicator.sqlite")).unwrap();
        (tmp, db)
    }

    fn name(id: i64, n: &str) -> EsiName {
        EsiName {
            id,
            name: n.to_string(),
            category: "character".to_string(),
        }
    }

    #[test]
    fn open_is_idempotent_across_reopens() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("replicator.sqlite");
        {
            let db = Db::open(&path).unwrap();
            db.set_setting("k", "v").unwrap();
        }
        // migrate() runs again on the second open; it must not clobber.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.get_setting("k").unwrap(), Some("v".to_string()));
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let (_t, db) = db();
        let enabled: i64 = db
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(enabled, 1, "group_members relies on ON DELETE CASCADE");
    }

    // ----------------------------- settings ---------------------------

    #[test]
    fn settings_roundtrip_and_overwrite() {
        let (_t, db) = db();
        assert_eq!(db.get_setting("missing").unwrap(), None);

        db.set_setting("install_override", "/a").unwrap();
        assert_eq!(
            db.get_setting("install_override").unwrap(),
            Some("/a".to_string())
        );

        db.set_setting("install_override", "/b").unwrap();
        assert_eq!(
            db.get_setting("install_override").unwrap(),
            Some("/b".to_string()),
            "upsert should replace, not fail on the primary key"
        );
    }

    #[test]
    fn deleting_a_setting_makes_it_absent() {
        let (_t, db) = db();
        db.set_setting("k", "v").unwrap();
        db.delete_setting("k").unwrap();
        assert_eq!(db.get_setting("k").unwrap(), None);
    }

    #[test]
    fn deleting_a_missing_setting_is_not_an_error() {
        let (_t, db) = db();
        assert!(db.delete_setting("never_existed").is_ok());
    }

    #[test]
    fn launcher_watermark_survives_a_reopen() {
        // copy_character stores its incremental-parse watermark here;
        // losing it would mean re-reading gigabytes of logs.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("replicator.sqlite");
        {
            let db = Db::open(&path).unwrap();
            db.set_setting("launcher_logs_parsed_through", "1700000000")
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.get_setting("launcher_logs_parsed_through").unwrap(),
            Some("1700000000".to_string())
        );
    }

    // ---------------------------- name cache --------------------------

    #[test]
    fn names_roundtrip() {
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Alice"), name(2002, "Bob")], 100)
            .unwrap();

        let got = db.lookup_names(&[1001, 2002]).unwrap();

        assert_eq!(got.get(&1001), Some(&"Alice".to_string()));
        assert_eq!(got.get(&2002), Some(&"Bob".to_string()));
    }

    #[test]
    fn lookup_names_omits_ids_it_has_never_seen() {
        // resolve_names diffs the result against the request to decide
        // what to fetch from ESI, so absent keys must stay absent.
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Alice")], 100).unwrap();

        let got = db.lookup_names(&[1001, 9999]).unwrap();

        assert_eq!(got.len(), 1);
        assert!(!got.contains_key(&9999));
    }

    #[test]
    fn upserting_a_name_replaces_the_previous_value() {
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Old Name")], 100).unwrap();
        db.upsert_names(&[name(1001, "New Name")], 200).unwrap();

        assert_eq!(
            db.lookup_names(&[1001]).unwrap().get(&1001),
            Some(&"New Name".to_string())
        );
    }

    #[test]
    fn lookup_names_with_no_ids_is_empty_and_does_not_build_bad_sql() {
        let (_t, db) = db();
        assert!(db.lookup_names(&[]).unwrap().is_empty());
    }

    #[test]
    fn upsert_names_with_an_empty_slice_is_a_noop() {
        let (_t, db) = db();
        assert!(db.upsert_names(&[], 100).is_ok());
    }

    #[test]
    fn lookup_names_handles_a_large_id_batch() {
        // The IN (...) clause is built by hand from placeholders.
        let (_t, db) = db();
        let names: Vec<EsiName> = (0..500).map(|i| name(i, &format!("Char {i}"))).collect();
        db.upsert_names(&names, 100).unwrap();

        let ids: Vec<i64> = (0..500).collect();
        assert_eq!(db.lookup_names(&ids).unwrap().len(), 500);
    }

    #[test]
    fn a_tombstoned_id_is_cached_but_never_named() {
        // ESI refused this id (a deleted character). It must not be
        // re-fetched (cached_ids sees it) and must not surface as a
        // name (lookup_names hides it, so the UI keeps rendering the
        // character by id).
        let (_t, db) = db();
        db.upsert_names(
            &[EsiName {
                id: 4004,
                name: String::new(),
                category: "unresolvable".to_string(),
            }],
            100,
        )
        .unwrap();

        assert!(db.lookup_names(&[4004]).unwrap().is_empty());
        assert!(db.cached_ids(&[4004], 0).unwrap().contains(&4004));
    }

    #[test]
    fn cached_ids_reports_exactly_the_ids_present() {
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Alice")], 100).unwrap();

        let got = db.cached_ids(&[1001, 9999], 0).unwrap();

        assert!(got.contains(&1001));
        assert!(!got.contains(&9999));
        assert!(db.cached_ids(&[], 0).unwrap().is_empty());
    }

    #[test]
    fn cached_ids_treats_stale_entries_as_absent() {
        // resolve_names passes now - TTL as the cutoff; an entry older
        // than that must count as unknown so renames re-resolve, while
        // one exactly at the cutoff still counts as fresh.
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Alice")], 100).unwrap();

        assert!(db.cached_ids(&[1001], 100).unwrap().contains(&1001));
        assert!(!db.cached_ids(&[1001], 101).unwrap().contains(&1001));
    }

    #[test]
    fn a_refetched_name_becomes_fresh_again() {
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Old Name")], 100).unwrap();
        db.upsert_names(&[name(1001, "New Name")], 500).unwrap();

        assert!(db.cached_ids(&[1001], 400).unwrap().contains(&1001));
        assert_eq!(
            db.lookup_names(&[1001]).unwrap().get(&1001),
            Some(&"New Name".to_string())
        );
    }

    // ------------------------------ groups ----------------------------

    #[test]
    fn groups_create_list_and_delete() {
        let (_t, db) = db();
        let id = db.create_group("Miners").unwrap();

        let groups = db.list_groups().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, id);
        assert_eq!(groups[0].name, "Miners");
        assert!(groups[0].member_ids.is_empty());

        db.delete_group(id).unwrap();
        assert!(db.list_groups().unwrap().is_empty());
    }

    #[test]
    fn groups_are_listed_alphabetically() {
        let (_t, db) = db();
        db.create_group("Zeta").unwrap();
        db.create_group("Alpha").unwrap();
        db.create_group("Mid").unwrap();

        let names: Vec<String> = db
            .list_groups()
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();

        assert_eq!(names, vec!["Alpha", "Mid", "Zeta"]);
    }

    #[test]
    fn duplicate_group_names_are_rejected_with_a_friendly_message() {
        let (_t, db) = db();
        db.create_group("Miners").unwrap();

        let err = db.create_group("Miners").unwrap_err();

        assert!(
            err.to_string().contains("already exists"),
            "the banner shows this verbatim; raw constraint text is unreadable, got: {err}"
        );
    }

    #[test]
    fn set_group_members_replaces_the_whole_membership() {
        let (_t, db) = db();
        let id = db.create_group("Miners").unwrap();

        db.set_group_members(id, &[1, 2, 3]).unwrap();
        assert_eq!(db.group_member_ids(id).unwrap(), vec![1, 2, 3]);

        db.set_group_members(id, &[4]).unwrap();
        assert_eq!(
            db.group_member_ids(id).unwrap(),
            vec![4],
            "setting members is a replace, not a merge"
        );
    }

    #[test]
    fn set_group_members_can_empty_a_group() {
        let (_t, db) = db();
        let id = db.create_group("Miners").unwrap();
        db.set_group_members(id, &[1, 2]).unwrap();

        db.set_group_members(id, &[]).unwrap();

        assert!(db.group_member_ids(id).unwrap().is_empty());
    }

    #[test]
    fn duplicate_member_ids_are_rejected_and_roll_back_cleanly() {
        let (_t, db) = db();
        let id = db.create_group("Miners").unwrap();
        db.set_group_members(id, &[10, 20]).unwrap();

        // (group_id, character_id) is a composite primary key, so a
        // repeated id aborts the insert. The UI only ever sends a Set,
        // so this is a guard rather than a live path - what matters is
        // that the failure leaves the previous membership intact
        // rather than half-applied.
        assert!(db.set_group_members(id, &[1, 1, 2]).is_err());
        assert_eq!(
            db.group_member_ids(id).unwrap(),
            vec![10, 20],
            "a failed membership update must roll back, not clear the group"
        );
    }

    #[test]
    fn list_groups_includes_members() {
        let (_t, db) = db();
        let a = db.create_group("Alpha").unwrap();
        let b = db.create_group("Beta").unwrap();
        db.set_group_members(a, &[10, 20]).unwrap();
        db.set_group_members(b, &[30]).unwrap();

        let groups = db.list_groups().unwrap();

        assert_eq!(groups[0].member_ids, vec![10, 20]);
        assert_eq!(groups[1].member_ids, vec![30]);
    }

    #[test]
    fn deleting_a_group_cascades_to_its_members() {
        let (_t, db) = db();
        let id = db.create_group("Miners").unwrap();
        db.set_group_members(id, &[1, 2, 3]).unwrap();

        db.delete_group(id).unwrap();

        let orphans: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM group_members", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0, "ON DELETE CASCADE should clear membership rows");
    }

    #[test]
    fn group_member_ids_of_an_unknown_group_is_empty() {
        let (_t, db) = db();
        assert!(db.group_member_ids(999).unwrap().is_empty());
    }

    // ----------------------------- templates --------------------------

    #[test]
    fn templates_store_and_return_exact_bytes() {
        let (_t, db) = db();
        // Settings files are binary; a text round-trip would hide
        // corruption on non-UTF8 bytes.
        let payload: Vec<u8> = vec![0x00, 0xFF, 0x1B, 0x7F, 0x80];
        let user_payload: Vec<u8> = vec![0x01, 0xFE, 0x00, 0x9C];
        let id = db
            .create_template(
                "Combat",
                Some(1001),
                &payload,
                Some(&user_payload),
                100,
                None,
            )
            .unwrap();

        assert_eq!(
            db.template_blobs(id).unwrap(),
            Some((payload, Some(user_payload)))
        );
    }

    #[test]
    fn a_template_without_user_bytes_stores_and_reports_none() {
        let (_t, db) = db();
        let id = db
            .create_template("Combat", Some(1001), b"char", None, 100, None)
            .unwrap();

        assert_eq!(
            db.template_blobs(id).unwrap(),
            Some((b"char".to_vec(), None))
        );
        assert!(
            !db.list_templates().unwrap()[0].has_user_data,
            "the UI warns off this flag; it must reflect the stored NULL"
        );
    }

    #[test]
    fn has_user_data_is_true_when_user_bytes_were_captured() {
        let (_t, db) = db();
        db.create_template("Combat", Some(1001), b"char", Some(b"user"), 100, None)
            .unwrap();

        assert!(db.list_templates().unwrap()[0].has_user_data);
    }

    #[test]
    fn a_pre_user_data_database_gains_the_column_on_open() {
        // Databases created before templates carried the user file have
        // no user_data column; migrate() must bolt it on rather than
        // fail the INSERT.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("replicator.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE templates (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   name TEXT UNIQUE NOT NULL,
                   source_character_id INTEGER,
                   data BLOB NOT NULL,
                   created_at INTEGER NOT NULL
                 );
                 INSERT INTO templates (name, source_character_id, data, created_at)
                 VALUES ('Old', 1001, x'00ff', 50);",
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();

        let templates = db.list_templates().unwrap();
        assert_eq!(templates.len(), 1);
        assert!(!templates[0].has_user_data, "pre-migration rows hold NULL");
        db.create_template("New", None, b"c", Some(b"u"), 100, None)
            .unwrap();
    }

    #[test]
    fn templates_remember_their_source_server() {
        let (_t, db) = db();
        db.create_template("TQ", Some(1001), b"x", None, 100, Some("tranquility"))
            .unwrap();
        db.create_template("Anywhere", None, b"x", None, 200, None)
            .unwrap();

        let t = db.list_templates().unwrap();

        assert_eq!(t[0].source_server, None, "newest first; no server recorded");
        assert_eq!(t[1].source_server.as_deref(), Some("tranquility"));
    }

    #[test]
    fn template_blobs_of_an_unknown_id_is_none() {
        let (_t, db) = db();
        assert_eq!(db.template_blobs(999).unwrap(), None);
    }

    #[test]
    fn listing_templates_reports_size_and_joins_the_source_name() {
        let (_t, db) = db();
        db.upsert_names(&[name(1001, "Alice")], 100).unwrap();
        db.create_template("Combat", Some(1001), b"12345", None, 100, None)
            .unwrap();

        let templates = db.list_templates().unwrap();

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Combat");
        assert_eq!(templates[0].source_character_id, Some(1001));
        assert_eq!(templates[0].source_name, Some("Alice".to_string()));
        assert_eq!(templates[0].size, 5);
    }

    #[test]
    fn a_template_whose_source_name_is_uncached_still_lists() {
        // LEFT JOIN, not INNER - a template must not vanish just
        // because ESI never resolved its source character.
        let (_t, db) = db();
        db.create_template("Combat", Some(1001), b"x", None, 100, None)
            .unwrap();

        let templates = db.list_templates().unwrap();

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].source_name, None);
    }

    #[test]
    fn templates_are_listed_newest_first() {
        let (_t, db) = db();
        db.create_template("Old", None, b"x", None, 100, None)
            .unwrap();
        db.create_template("New", None, b"x", None, 300, None)
            .unwrap();
        db.create_template("Mid", None, b"x", None, 200, None)
            .unwrap();

        let names: Vec<String> = db
            .list_templates()
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();

        assert_eq!(names, vec!["New", "Mid", "Old"]);
    }

    #[test]
    fn duplicate_template_names_are_rejected_with_a_friendly_message() {
        let (_t, db) = db();
        db.create_template("Combat", None, b"x", None, 100, None)
            .unwrap();

        let err = db
            .create_template("Combat", None, b"y", None, 100, None)
            .unwrap_err();

        assert!(err.to_string().contains("already exists"), "got: {err}");
    }

    #[test]
    fn deleting_a_template_removes_it() {
        let (_t, db) = db();
        let id = db
            .create_template("Combat", None, b"x", None, 100, None)
            .unwrap();

        db.delete_template(id).unwrap();

        assert!(db.list_templates().unwrap().is_empty());
        assert_eq!(db.template_blobs(id).unwrap(), None);
    }

    // -------------------------- char -> user map -----------------------

    #[test]
    fn char_user_pairs_roundtrip() {
        let (_t, db) = db();
        let pairs = HashMap::from([(2035047876, 4342021), (2035047877, 4342021)]);
        db.upsert_char_user_pairs(&pairs, 100).unwrap();

        let got = db.lookup_user_ids(&[2035047876, 2035047877]).unwrap();

        assert_eq!(got.get(&2035047876), Some(&4342021));
        assert_eq!(got.get(&2035047877), Some(&4342021));
    }

    #[test]
    fn a_character_moving_accounts_updates_its_mapping() {
        // Without the ON CONFLICT clause this would silently keep the
        // stale account and copy the wrong user file.
        let (_t, db) = db();
        db.upsert_char_user_pairs(&HashMap::from([(2035047876, 1111111)]), 100)
            .unwrap();
        db.upsert_char_user_pairs(&HashMap::from([(2035047876, 2222222)]), 200)
            .unwrap();

        assert_eq!(
            db.lookup_user_ids(&[2035047876]).unwrap().get(&2035047876),
            Some(&2222222)
        );
    }

    #[test]
    fn lookup_user_ids_omits_unmapped_characters() {
        let (_t, db) = db();
        db.upsert_char_user_pairs(&HashMap::from([(2035047876, 4342021)]), 100)
            .unwrap();

        let got = db.lookup_user_ids(&[2035047876, 9999999]).unwrap();

        assert_eq!(got.len(), 1);
        assert!(
            !got.contains_key(&9999999),
            "an unmapped character must yield None so copy_character reports it"
        );
    }

    #[test]
    fn char_user_map_edge_cases_do_not_panic() {
        let (_t, db) = db();
        assert!(db.upsert_char_user_pairs(&HashMap::new(), 100).is_ok());
        assert!(db.lookup_user_ids(&[]).unwrap().is_empty());
    }

    // ----------------------- manual char pairing ----------------------

    fn cp(user_id: i64, manual: bool, log_user_id: Option<i64>) -> CharPairing {
        CharPairing {
            user_id,
            manual,
            log_user_id,
        }
    }

    #[test]
    fn a_manual_pairing_survives_a_log_upsert_but_records_the_dispute() {
        let (_t, db) = db();
        db.set_char_user_manual(1001, 9001, 100).unwrap();

        // The launcher logs claim a different account; the user's own
        // decision stands, but the disagreement goes on record.
        db.upsert_char_user_pairs(&HashMap::from([(1001, 8888)]), 200)
            .unwrap();

        assert_eq!(
            db.lookup_user_pairs(&[1001]).unwrap().get(&1001),
            Some(&cp(9001, true, Some(8888)))
        );
    }

    #[test]
    fn a_log_pairing_is_overwritten_by_hand_and_by_newer_logs() {
        let (_t, db) = db();
        db.upsert_char_user_pairs(&HashMap::from([(1001, 8888)]), 100)
            .unwrap();
        assert_eq!(
            db.lookup_user_pairs(&[1001]).unwrap().get(&1001),
            Some(&cp(8888, false, Some(8888)))
        );

        db.upsert_char_user_pairs(&HashMap::from([(1001, 7777)]), 200)
            .unwrap();
        assert_eq!(
            db.lookup_user_pairs(&[1001]).unwrap().get(&1001),
            Some(&cp(7777, false, Some(7777))),
            "log rows keep following the logs"
        );

        db.set_char_user_manual(1001, 9001, 300).unwrap();
        assert_eq!(
            db.lookup_user_pairs(&[1001]).unwrap().get(&1001),
            Some(&cp(9001, true, Some(7777))),
            "pinning by hand keeps the logs' answer on record"
        );
    }

    #[test]
    fn clearing_manual_reverts_to_the_logs_or_deletes() {
        let (_t, db) = db();
        // 1001: log row - clearing must not touch it.
        db.upsert_char_user_pairs(&HashMap::from([(1001, 8888)]), 100)
            .unwrap();
        // 2002: hand-set with no log knowledge - clearing deletes.
        db.set_char_user_manual(2002, 9001, 100).unwrap();
        // 3003: hand-set against what the logs said - clearing accepts
        // the logs' answer.
        db.upsert_char_user_pairs(&HashMap::from([(3003, 7777)]), 100)
            .unwrap();
        db.set_char_user_manual(3003, 9001, 150).unwrap();

        db.clear_char_user_manual(1001, 200).unwrap();
        db.clear_char_user_manual(2002, 200).unwrap();
        db.clear_char_user_manual(3003, 200).unwrap();

        let pairs = db.lookup_user_pairs(&[1001, 2002, 3003]).unwrap();
        assert_eq!(
            pairs.get(&1001),
            Some(&cp(8888, false, Some(8888))),
            "a log row records something that actually happened"
        );
        assert_eq!(pairs.get(&2002), None);
        assert_eq!(
            pairs.get(&3003),
            Some(&cp(7777, false, Some(7777))),
            "clearing hands control back to the logs literally"
        );
    }

    #[test]
    fn lookup_user_ids_sees_manual_pairs_too() {
        // Account-level copy resolves the user file through
        // lookup_user_ids; a hand-set pairing must feed it the same way
        // a logged one does.
        let (_t, db) = db();
        db.set_char_user_manual(1001, 9001, 100).unwrap();
        assert_eq!(db.lookup_user_ids(&[1001]).unwrap().get(&1001), Some(&9001));
    }

    // --------------------------- window_floors ------------------------

    #[test]
    fn window_floors_round_trip_and_keep_the_smallest() {
        let (_t, db) = db();
        db.set_window_floors(&[("overview".into(), 200, 150)])
            .unwrap();
        db.set_window_floors(&[("overview".into(), 180, 220)])
            .unwrap();

        let floors = db.list_window_floors().unwrap();

        assert_eq!(
            floors,
            vec![("overview".to_string(), 180, 150)],
            "each axis keeps its own smallest calibrated value"
        );
    }

    // --------------------------- account_meta -------------------------

    #[test]
    fn account_meta_round_trips_and_updates() {
        let (_t, db) = db();
        assert!(db.list_account_meta().unwrap().is_empty());

        db.set_account_meta(4342021, Some("Main"), Some("has the good skins"))
            .unwrap();
        assert_eq!(
            db.list_account_meta().unwrap(),
            vec![AccountMeta {
                user_id: 4342021,
                alias: Some("Main".into()),
                note: Some("has the good skins".into()),
            }]
        );

        db.set_account_meta(4342021, Some("Cyno alt"), None)
            .unwrap();
        assert_eq!(
            db.list_account_meta().unwrap(),
            vec![AccountMeta {
                user_id: 4342021,
                alias: Some("Cyno alt".into()),
                note: None,
            }]
        );
    }

    #[test]
    fn blank_account_meta_fields_normalize_away() {
        let (_t, db) = db();
        db.set_account_meta(1, Some("  Main  "), Some("   "))
            .unwrap();
        assert_eq!(
            db.list_account_meta().unwrap(),
            vec![AccountMeta {
                user_id: 1,
                alias: Some("Main".into()),
                note: None,
            }]
        );
    }

    #[test]
    fn clearing_both_fields_deletes_the_row() {
        let (_t, db) = db();
        db.set_account_meta(1, Some("Main"), Some("note")).unwrap();
        db.set_account_meta(1, Some(""), None).unwrap();
        assert!(
            db.list_account_meta().unwrap().is_empty(),
            "an all-blank row must not linger as (None, None)"
        );
    }
}

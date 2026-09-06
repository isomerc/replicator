export type InstallKind =
  | "windows-native"
  | "macos-native"
  | "linux-proton"
  | "manual";

export interface EveInstall {
  label: string;
  kind: InstallKind;
  root: string;
  settings_dirs: string[];
}

export interface InstallSummary {
  installs: EveInstall[];
  override_path: string | null;
}

export interface CharacterEntry {
  id: number;
  name: string | null;
  install_root: string;
  settings_dirs: string[];
  /** Distinct servers the character's settings dirs belong to; empty
   *  when none carry a recognizable server component. */
  servers: string[];
  /** Owning account per the launcher-log mapping, when known. */
  user_id: number | null;
  /** The user's alias for that account, when set. */
  account_alias: string | null;
  modified: number | null;
  size: number;
}

export interface UserEntry {
  id: number;
  install_root: string;
  modified: number | null;
}

export interface CharactersResponse {
  characters: CharacterEntry[];
  users: UserEntry[];
}

export interface Group {
  id: number;
  name: string;
  member_ids: number[];
}

export interface Template {
  id: number;
  name: string;
  source_character_id: number | null;
  source_name: string | null;
  created_at: number;
  size: number;
  /** False when the account-level user file (window layout, overview,
   *  chat) was not captured - applying moves only the character file. */
  has_user_data: boolean;
  /** The server the source lived on when saved; apply stays on it
   *  unless told to cross. Null for older templates, which apply
   *  anywhere. */
  source_server: string | null;
}

export interface CopyReport {
  written: string[];
  skipped: string[];
}

/** Which settings groups to copy; null anywhere it's accepted means
 *  "everything" (byte-identical whole-file copy). */
export interface CopySelection {
  char_groups: string[];
  user_groups: string[];
}

export interface CopyGroups {
  char_groups: string[];
  user_groups: string[];
}

export interface MirrorStatus {
  initialized: boolean;
  head: string | null;
  remote: string | null;
  branch: string;
  dirty: boolean;
}

export interface CommitEntry {
  oid: string;
  short: string;
  summary: string;
  timestamp: number;
  author: string;
}

export interface RestoreReport {
  restored: number;
  skipped: string[];
}

/** One settings group's difference between two files. */
export interface GroupDiff {
  group: string;
  kind: "added" | "removed" | "changed";
  entries_changed: number | null;
  entries_added: number | null;
  entries_removed: number | null;
  entry_names: string[];
}

/** One profile file's semantic change within a commit. */
export interface FileDiff {
  kind: "character" | "user";
  id: number;
  label: string;
  /** The mirror profile subtree; the same character can change
   *  independently per settings profile. */
  profile: string;
  status: "changed" | "added" | "removed" | "opaque";
  groups: GroupDiff[];
}

/** One window's place on screen, normalized 0..1. */
export interface WindowRect {
  name: string;
  /** Other windows sharing this exact frame (a stack's members). */
  stacked: string[];
  x: number;
  y: number;
  w: number;
  h: number;
  /** The viewport this rect was saved against - pixel floors must be
   *  normalized per window when a file carries mixed resolutions. */
  vw: number;
  vh: number;
}

/** A designed position for one window, normalized 0..1. */
export interface LayoutChange {
  name: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface WindowLayout {
  screen_w: number;
  screen_h: number;
  windows: WindowRect[];
}

/** How two characters differ; notes are machine codes:
 *  "undecodable" | "same_account" | "unavailable". */
export interface CompareReport {
  char_groups: GroupDiff[];
  char_note: string | null;
  char_bytes_differ: boolean;
  user_groups: GroupDiff[];
  user_note: string | null;
}

export interface PullReport {
  updated: boolean;
  head: string | null;
}

export interface ExportSummary {
  files: number;
  characters: number;
}

export interface AccountCharacter {
  id: number;
  name: string | null;
  /** True when the pairing was set by hand rather than learned from
   *  the launcher logs; hand-set pairings are editable in the UI. */
  manual: boolean;
  /** True when a hand-set pairing contradicts what the launcher logs
   *  have since recorded. */
  disputed: boolean;
  /** What the logs last said, for the dispute message. */
  log_user_id: number | null;
}

export interface AccountEntry {
  user_id: number;
  alias: string | null;
  note: string | null;
  characters: AccountCharacter[];
  modified: number | null;
}

export interface CharacterRef {
  id: number;
  name: string | null;
}

export interface TemplateMeta {
  name: string;
  source_character_id: number | null;
  source_name: string | null;
  has_user_data: boolean;
  source_server?: string | null;
}

/** What importing a zip would do; would_write is empty for template
 *  archives (importing those only adds to the shelf). */
export interface ImportPreview {
  kind: "setup" | "template";
  app_version: string;
  created_at: number;
  characters: CharacterRef[];
  file_count: number;
  template: TemplateMeta | null;
  would_write: string[];
  skipped: string[];
}

/** Result of the one-shot GitHub release check. The frontend renders
 *  nothing when the promise rejects (offline, rate limit, no public
 *  release yet). */
export interface UpdateCheck {
  current: string;
  latest: string;
  url: string;
  outdated: boolean;
}

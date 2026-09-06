/** The master dictionary. Keys here define the app's translatable
 *  surface; every other locale falls back to these strings. */
export const en = {
  // Masthead / app chrome
  tagline: "The EVE Online UI manager that never forgets",
  sg_label: "SURGEON GENERAL'S WARNING:",
  eve_running_blocked:
    "EVE is running. Copies, applies and restores are blocked until every client closes.",
  installs_on_file: "{n} installs on file",
  install_on_file: "1 install on file",
  no_installs: "no EVE installs detected",
  refresh: "Refresh",
  loading: "Loading…",

  // Departments
  dept_characters: "Characters",
  dept_groups: "Groups",
  dept_templates: "Templates",
  dept_git: "Version Control",
  dept_settings: "Settings",
  dept_about: "About",

  // Common
  cancel: "Cancel",
  save: "Save",
  close: "Close",
  apply: "Apply",
  delete: "Delete",
  remove: "Remove",
  clear: "Clear",
  character_n: "Character {id}",
  close_eve_first: "Close EVE first",

  // Characters: roster
  no_chars_title: "No characters found",
  no_chars_help:
    "Replicator looks for core_char_*.dat files in EVE's settings folders. If you've never logged in on this machine, no characters will appear. Open Settings to point at a custom location.",
  filter_placeholder: "Filter by name or ID…",
  n_shown: "{n} shown",
  select_all: "Select all",
  n_profiles: "{n} profiles",

  // Characters: order panel
  source_label: "Source",
  targets_label: "Targets",
  hint_source: "Pick the character to copy from",
  hint_targets: "Shift-click characters, or tick their boxes",
  n_characters: "{n} characters",
  one_character: "1 character",
  copy_settings: "Copy settings",
  save_as_template: "Save source as template",
  or_apply_template: "Or apply a template to targets",
  pick_template: "Pick a template…",
  or_copy_group: "Or copy source to a group",
  n_members: "{n} members",
  one_member: "1 member",

  // Characters: messages and reports
  msg_copied: "Copied to {n} file(s)",
  msg_skipped_suffix: ", {n} skipped",
  report_copy_title: "Copy report",
  msg_applied_template: 'Applied "{name}" to {n} file(s)',
  report_apply_title: "Template apply report",
  msg_copied_group: 'Copied to group "{name}" → {n} file(s)',
  report_group_title: 'Copy to "{name}" report',
  msg_saved_template: 'Saved template "{name}"',
  msg_saved_template_partial:
    'Saved template "{name}" - character file only; no account-level settings were captured (no launcher-log mapping for this character)',

  // Copy dialog
  confirm_copy_title: "Confirm copy",
  confirm_copy_body:
    "Overwrite the UI settings of the following characters with {name}'s setup?",
  cross_server_label: "Also write targets on other servers",
  copy_snapshot_note:
    "If version control is initialized, a snapshot of the current state is committed automatically before the copy, so you can restore it. Otherwise there is no backup.",
  copy_account_note:
    "Account-level settings (window layout, overview, chat) go to every account file in each target's settings folder, not only the targets' own accounts. Untick the account groups to leave them alone.",
  copy: "Copy",
  nothing_selected: "Nothing is selected to copy",

  // Group picker
  choose_what: "Choose what to copy",
  char_settings: "Character settings",
  account_settings: "Account settings",
  not_available_source: "Not available for this source",
  selective_unavailable:
    "Selective copy is not available for this source (its settings could not be parsed) - everything will be copied.",
  picker_note: "Unchecked groups keep each target's current values.",

  // Settings groups (raw keys fall through untranslated)
  group_windows: "Windows & positions",
  group_ui: "General UI state",
  group_overview: "Overview & presets",
  group_defaultoverview: "Default overview",
  group_tabgroups: "Window tabs",
  group_shiptheme: "Ship theme",
  group_autoreload: "Ammo auto-reload",
  group_autorepeat: "Auto-repeat",
  group_dockPanels: "Docked panels",
  group_notifications: "Notifications",
  group_notepad: "Notepad",
  group_inbox: "Mail",
  group_suppress: "Suppressed prompts",
  group_cmd: "Custom commands",
  group_localization: "Localization",
  group_audio: "Audio",
  group_generic: "Generic",
  group_zaction: "Actions",
  group_enableWindowBlur: "Window blur",
  group_unseenInventoryItems: "Unseen inventory",

  // Save template dialog
  save_template_title: "Save as template",
  save_template_help:
    "Snapshot {name}'s current UI setup as a reusable template. Templates live inside Replicator - your EVE settings folder is not touched.",
  template_name_placeholder: "Template name",

  // Templates shelf
  shelf_empty_title: "The shelf is empty",
  shelf_empty_help:
    "From Characters, click a character to mark it as the source, then use Save source as template on the order form. Templates are stored inside Replicator and can be applied to any character later - even ones that don't exist yet at the time of saving.",
  template_kicker: "Template",
  from_name: "from {name}",
  char_file_only: "Character file only",
  char_file_only_tip:
    "The account-level settings (window layout, overview, chat) were not captured when this template was saved. Applying it moves only the character file.",
  apply_ellipsis: "Apply…",
  confirm_delete_template: 'Delete template "{name}"?',
  msg_deleted: 'Deleted "{name}"',
  apply_template_title: 'Apply "{name}"',
  apply_template_help:
    "Pick the characters to overwrite. If version control is initialized, a snapshot of the current state is committed automatically before the overwrite; otherwise there is no backup.",
  apply_to_n: "Apply to {n}",

  // Groups
  new_group_placeholder: "New group name…",
  create: "Create",
  n_groups: "{n} groups",
  one_group: "1 group",
  groups_empty_title: "No groups yet",
  groups_empty_help:
    "Groups let you bundle characters that share a UI layout. Create a group, drop characters into it, and you can copy any source's UI to the entire group in one click from the Characters view.",
  edit_members: "Edit members",
  confirm_delete_group: 'Delete group "{name}"?',
  msg_created_group: 'Created group "{name}"',
  msg_deleted_group: 'Deleted "{name}"',
  msg_updated_group: 'Updated members of "{name}"',
  members_of: 'Members of "{name}"',
  save_n: "Save ({n})",

  // Version control
  ledger_empty_title: "No snapshots yet",
  ledger_empty_help:
    "Take a snapshot and every version of your settings starts being recorded here - copies, applies and restores commit automatically from then on. Restoring an entry writes those files back over your live settings.",
  entry: "Entry",
  recorded: "Recorded",
  restore: "Restore",
  no_message: "(no message)",
  mirror_label: "Mirror",
  status_label: "Status",
  status_clean: "clean",
  status_dirty: "uncommitted changes",
  status_uninit: "not initialized",
  head_label: "Head",
  remote_label: "Remote",
  none_label: "(none)",
  edit_remote: "Edit remote",
  clone_title: "Restore from a remote",
  clone_help:
    "Already pushed your settings from another machine? Clone that repository and the whole history lands in the ledger. HTTPS URL; for a private repository, save your PAT in Settings first.",
  clone: "Clone",
  cloning: "Cloning…",
  snapshot_label: "Snapshot",
  snapshot_placeholder: "Optional message…",
  take_snapshot: "Take snapshot",
  snapshot_fine:
    "Copies, applies and restores snapshot automatically; take one by hand before anything drastic.",
  sync_label: "Sync",
  push: "Push",
  pull: "Pull",
  pushing: "Pushing…",
  pulling: "Pulling…",
  pat_fine: "Pushing needs a PAT, set in Settings.",
  take_snapshot_first: "Take a snapshot first",
  set_remote_first: "Set the remote above",
  set_pat_first: "Set a GitHub PAT in Settings",
  clone_or_snapshot_first: "Clone or take a snapshot first",
  restore_confirm_title: "Restore snapshot?",
  restore_confirm_body:
    "This will overwrite each character's current UI with the version from {short}:",
  restore_note:
    "A snapshot of the current state is committed automatically before the restore, so you can always restore back to it.",
  msg_no_changes: "No changes since last snapshot",
  msg_snapshot_created: "Snapshot {short} created",
  msg_pushed: "Pushed to remote",
  msg_pulled: "Pulled - history updated",
  msg_up_to_date: "Already up to date",
  msg_cloned: "Cloned - history restored from the remote",
  msg_remote_saved: "Remote saved",
  msg_restored: "Restored {n} file(s) from {short}",
  restore_report_title: "Restore from {short} report",
  set_pat_first_error: "Set a GitHub PAT in Settings first",

  // Accounts
  dept_accounts: "Accounts",
  account_n: "Account {id}",
  n_accounts: "{n} accounts",
  one_account: "1 account",
  no_accounts_title: "No accounts found",
  no_accounts_help:
    "Accounts appear when EVE leaves a core_user_*.dat in a settings folder, and characters attach to them via the launcher's logs. Log in through the EVE launcher once and both fill in.",
  accounts_fine: "Aliases and notes live inside Replicator only.",
  no_chars_mapped: "no characters mapped yet",
  unmapped_n: "{n} accounts with no characters mapped",
  unmapped_one: "1 account with no characters mapped",
  unmapped_chars_n: "{n} characters not yet tied to an account",
  unmapped_chars_one: "1 character not yet tied to an account",
  mapping_explainer:
    "EVE's own files never record which account owns a character. Replicator learns each pairing from the EVE launcher's logs: log a character in through the launcher and it files itself under its account on the next refresh.",
  edit_ellipsis: "Edit…",
  alias_label: "Alias",
  note_label: "Note",
  alias_placeholder: "Main, cyno alt, indy…",
  note_placeholder: "Anything worth remembering about this account…",
  msg_account_saved: "Account details saved",

  assign_ellipsis: "Assign…",
  assign_title: 'File "{name}" under an account',
  assign_help:
    "Pick the account this character belongs to. Hand-set pairings feed account-level copies just like logged ones, and the launcher logs never overwrite them - clear the pairing to hand control back to the logs.",
  assign_pick: "Pick an account…",
  assign_clear: "Clear pairing",
  set_by_hand: "set by hand",
  msg_assigned: 'Filed "{name}" under {account}',
  msg_cleared_pair: 'Unpaired "{name}"',
  logs_disagree_tip: "the launcher logs say {account}",
  use_logs_account: "Use the logs' answer ({account})",

  // Semantic diffs
  wf_cat_chat: "Chat",
  wf_cat_overview: "Overview & targets",
  wf_cat_scanning: "Scanning",
  wf_cat_commerce: "Market & wallet",
  wf_cat_ship: "Ship & fitting",
  wf_cat_inventory: "Inventory & assets",
  wf_cat_social: "Fleet & social",
  wf_cat_other: "Other",
  wf_click_hint:
    "Click a window in the drawing to hide it; the legend toggles whole families. This is only a lens - nothing is written.",
  wf_show_all: "Show all",
  design_edit: "Design…",
  design_hint:
    "Drag windows to move them; drag the corner handle to resize. Sizes stop at the smallest EVE has ever saved for each window, so the game will honor what you draw. Nothing is written until you save.",
  design_save_tpl: "Save as template…",
  design_write_to: 'Write to "{name}"',
  design_reset: "Discard changes",
  msg_layout_written: 'New layout written to "{name}"',
  compare: "Compare",
  layout_label: "Window layout",
  compare_title: "{a} vs {b}",
  diff_added: "added",
  diff_removed: "removed",
  diff_changed: "changed",
  diff_n_changed: "{n} changed",
  diff_n_added: "{n} added",
  diff_n_removed: "{n} removed",
  diff_identical: "No differences in decodable settings.",
  diff_opaque: "changed (could not be decoded)",
  diff_none_commit: "Nothing decodable changed in this entry.",
  tip_show_changes: "Show what changed",
  choose_what_restore: "Choose what to restore",
  picker_note_restore: "Unchecked groups keep their current values.",
  note_same_account:
    "Both characters live on the same account - their account settings are the same file.",
  note_user_unavailable:
    "Account settings could not be compared (missing mapping or files).",
  note_undecodable:
    "These files could not be decoded, so they were compared as raw bytes.",
  diff_bytes_differ: "The bytes differ.",
  diff_bytes_same: "The bytes are identical.",

  // Portable zip export/import
  portable_label: "Portable copy",
  export_zip: "Export zip…",
  import_zip: "Import zip…",
  portable_fine:
    "One zip carries every profile's settings plus a checksummed manifest - hand it to a corpmate or carry it to another machine. No git, no account.",
  msg_exported: "Exported {n} file(s) to {path}",
  msg_exported_template: 'Exported "{name}" to {path}',
  msg_imported: "Imported {n} file(s)",
  msg_imported_template: 'Imported template "{name}"',
  import_report_title: "Import report",
  import_confirm_title: "Import setup?",
  import_meta_line: "Replicator {v} archive, recorded {date}.",
  import_chars_label: "Characters in the archive:",
  import_would_write: "{n} file(s) will be overwritten:",
  import_none_match:
    "Nothing in this archive matches the settings on this machine - nothing would be written.",
  import_skipped_note:
    "{n} item(s) will be skipped - details in the report afterwards.",
  import_word: "Import",
  export_ellipsis: "Export…",

  // Settings view
  installs_title: "EVE installs",
  installs_help:
    "No installs auto-detected. Use Browse to point at the folder containing your settings_* directories - typically %LOCALAPPDATA%\\CCP\\EVE on Windows, ~/Library/Application Support/CCP/EVE on macOS, or the EVE folder inside your Proton prefix on Linux.",
  n_settings_dirs: "{n} settings dirs",
  one_settings_dir: "1 settings dir",
  browse: "Browse…",
  override_label: "Override:",
  msg_override_set: "Override set: {path}",
  msg_override_cleared: "Override cleared",
  pat_title: "GitHub Personal Access Token",
  pat_help:
    "The PAT is used only when pushing the mirror repo to a GitHub remote. It is stored in the OS keychain (Windows Credential Manager / macOS Keychain / Linux secret-service) - never written to disk by Replicator. Use a fine-grained token with contents: read & write on a single repository.",
  pat_placeholder_stored: "(token stored - paste to replace)",
  pat_placeholder_empty: "ghp_…",
  msg_pat_saved: "PAT saved to OS keychain",
  msg_pat_removed: "PAT removed",
  language_title: "Language",
  language_help:
    "Chrome and labels translate; report details and error messages from the engine stay in English.",

  // About
  version_line: "Version {v}",
  mit_licensed: "MIT licensed",
  about_docs: "Documentation",
  about_credit_author: "Blended and packed by Isomerc.",
  about_credit_fonts:
    "Set in Alfa Slab One, Source Serif 4, Pinyon Script and IBM Plex Mono, all under the SIL Open Font License.",
  about_credit_network:
    "No account, no telemetry. The network calls are to the public EVE ESI and image servers, to GitHub once per launch to check for a newer release, and to any git remote you configure yourself.",
  about_credit_trademark:
    "Not affiliated with Fenris Creations. EVE Online and all related marks are trademarks of Fenris Creations.",

  // Report dialog
  n_written: "{n} file(s) written",
  n_skipped_below: ", {n} skipped - each with a reason below",
  written_files_n: "Written files ({n})",

  // Footline
  footer_recruiting: "Illuminated is recruiting",
  latest_version: "Latest version",
  new_version_available: "New version available (v{v})",

  // Updater
  update_ask_title: "Update available",
  update_ask_body:
    "Download and install version {v} now? Replicator will restart.",
  update_installing: "Installing update…",
  open_settings: "Open Settings",
};

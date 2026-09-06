import { invoke } from "@tauri-apps/api/core";
import type {
  CharactersResponse,
  CommitEntry,
  CompareReport,
  CopyGroups,
  CopyReport,
  AccountEntry,
  CopySelection,
  FileDiff,
  ExportSummary,
  Group,
  ImportPreview,
  InstallSummary,
  LayoutChange,
  MirrorStatus,
  PullReport,
  RestoreReport,
  Template,
  UpdateCheck,
  WindowLayout,
} from "./types";

export const api = {
  detectInstalls: () => invoke<InstallSummary>("detect_installs"),
  setInstallOverride: (path: string | null) =>
    invoke<InstallSummary>("set_install_override", { path }),
  listCharacters: () => invoke<CharactersResponse>("list_characters"),
  resolveNames: (ids: number[]) =>
    invoke<Record<string, string>>("resolve_names", { ids }),
  copyCharacter: (
    sourceId: number,
    targetIds: number[],
    selection?: CopySelection | null,
    crossServer?: boolean
  ) =>
    invoke<CopyReport>("copy_character", {
      sourceId,
      targetIds,
      selection: selection ?? null,
      crossServer: crossServer ?? null,
    }),
  copyToGroup: (
    sourceId: number,
    groupId: number,
    selection?: CopySelection | null,
    crossServer?: boolean
  ) =>
    invoke<CopyReport>("copy_to_group", {
      sourceId,
      groupId,
      selection: selection ?? null,
      crossServer: crossServer ?? null,
    }),
  copyGroups: (sourceId: number) =>
    invoke<CopyGroups>("copy_groups", { sourceId }),
  templateGroups: (templateId: number) =>
    invoke<CopyGroups>("template_groups", { templateId }),
  saveTemplate: (sourceId: number, name: string) =>
    invoke<Template>("save_template", { sourceId, name }),
  listTemplates: () => invoke<Template[]>("list_templates"),
  applyTemplate: (
    templateId: number,
    targetIds: number[],
    selection?: CopySelection | null,
    crossServer?: boolean
  ) =>
    invoke<CopyReport>("apply_template", {
      templateId,
      targetIds,
      selection: selection ?? null,
      crossServer: crossServer ?? null,
    }),
  deleteTemplate: (templateId: number) =>
    invoke<void>("delete_template", { templateId }),
  listGroups: () => invoke<Group[]>("list_groups"),
  createGroup: (name: string) => invoke<Group>("create_group", { name }),
  deleteGroup: (groupId: number) => invoke<void>("delete_group", { groupId }),
  setGroupMembers: (groupId: number, memberIds: number[]) =>
    invoke<Group>("set_group_members", { groupId, memberIds }),
  isEveRunning: () => invoke<boolean>("is_eve_running"),
  characterPortraits: (ids: number[]) =>
    invoke<Record<string, string>>("character_portraits", { ids }),
  gitStatus: () => invoke<MirrorStatus>("git_status"),
  gitSetRemote: (url: string) => invoke<void>("git_set_remote", { url }),
  gitSnapshot: (message?: string) =>
    invoke<string | null>("git_snapshot", { message: message ?? null }),
  gitHistory: (limit?: number) =>
    invoke<CommitEntry[]>("git_history", { limit: limit ?? null }),
  gitRestore: (oid: string, selection?: CopySelection | null) =>
    invoke<RestoreReport>("git_restore", { oid, selection: selection ?? null }),
  gitDiff: (oid: string) => invoke<FileDiff[]>("git_diff", { oid }),
  commitGroups: (oid: string) => invoke<CopyGroups>("commit_groups", { oid }),
  diffCharacters: (aId: number, bId: number) =>
    invoke<CompareReport>("diff_characters", { aId, bId }),
  characterLayout: (characterId: number) =>
    invoke<WindowLayout | null>("character_layout", { characterId }),
  templateLayout: (templateId: number) =>
    invoke<WindowLayout | null>("template_layout", { templateId }),
  designSaveTemplate: (
    characterId: number,
    changes: LayoutChange[],
    name: string
  ) =>
    invoke<Template>("design_save_template", { characterId, changes, name }),
  designApply: (characterId: number, changes: LayoutChange[]) =>
    invoke<CopyReport>("design_apply", { characterId, changes }),
  layoutFloors: () =>
    invoke<Record<string, [number, number]>>("layout_floors"),
  gitPush: () => invoke<void>("git_push"),
  gitClone: (url: string) => invoke<MirrorStatus>("git_clone", { url }),
  gitPull: () => invoke<PullReport>("git_pull"),
  listAccounts: () => invoke<AccountEntry[]>("list_accounts"),
  setAccountMeta: (userId: number, alias: string | null, note: string | null) =>
    invoke<void>("set_account_meta", { userId, alias, note }),
  setCharAccount: (characterId: number, userId: number | null) =>
    invoke<void>("set_char_account", { characterId, userId }),
  exportSetup: (path: string) =>
    invoke<ExportSummary>("export_setup", { path }),
  previewImport: (path: string) =>
    invoke<ImportPreview>("preview_import", { path }),
  importSetup: (path: string) => invoke<CopyReport>("import_setup", { path }),
  exportTemplate: (templateId: number, path: string) =>
    invoke<void>("export_template", { templateId, path }),
  importTemplate: (path: string) =>
    invoke<Template>("import_template", { path }),
  patSet: (token: string) => invoke<void>("pat_set", { token }),
  patClear: () => invoke<void>("pat_clear"),
  patPresent: () => invoke<boolean>("pat_present"),
  playJazzChord: () => invoke<void>("play_jazz_chord"),
  checkUpdate: () => invoke<UpdateCheck>("check_update"),
};

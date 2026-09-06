import type { MessageKey } from "./i18n";

/** Known top-level settings groups in EVE's .dat files, mapped to their
 *  i18n keys. Anything unknown falls back to the raw key, so new groups
 *  appearing after a patch still show up. */
const KEYS: Record<string, MessageKey> = {
  windows: "group_windows",
  ui: "group_ui",
  overview: "group_overview",
  defaultoverview: "group_defaultoverview",
  tabgroups: "group_tabgroups",
  shiptheme: "group_shiptheme",
  autoreload: "group_autoreload",
  autorepeat: "group_autorepeat",
  dockPanels: "group_dockPanels",
  notifications: "group_notifications",
  notepad: "group_notepad",
  inbox: "group_inbox",
  suppress: "group_suppress",
  cmd: "group_cmd",
  localization: "group_localization",
  audio: "group_audio",
  generic: "group_generic",
  zaction: "group_zaction",
  enableWindowBlur: "group_enableWindowBlur",
  unseenInventoryItems: "group_unseenInventoryItems",
};

export function groupMessageKey(key: string): MessageKey | null {
  return KEYS[key] ?? null;
}

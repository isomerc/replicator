import type { MessageKey } from "./i18n";

/** A family of EVE windows, with its blueprint ink. */
export interface WfCategory {
  key: string;
  labelKey: MessageKey;
  color: string;
}

/** Print-palette inks, one per family. Order is legend order. */
export const WF_CATEGORIES: WfCategory[] = [
  { key: "chat", labelKey: "wf_cat_chat", color: "#2e5d8c" },
  { key: "overview", labelKey: "wf_cat_overview", color: "#c8102e" },
  { key: "scanning", labelKey: "wf_cat_scanning", color: "#2e6e6a" },
  { key: "commerce", labelKey: "wf_cat_commerce", color: "#b98a1d" },
  { key: "ship", labelKey: "wf_cat_ship", color: "#7c3a5f" },
  { key: "inventory", labelKey: "wf_cat_inventory", color: "#a07440" },
  { key: "social", labelKey: "wf_cat_social", color: "#4c6b2f" },
  { key: "other", labelKey: "wf_cat_other", color: "#22190d" },
];

/** First match wins; window names come straight from EVE's files. */
const RULES: [string, RegExp][] = [
  ["chat", /^chat|chatwindowstack/i],
  ["scanning", /scanner/i],
  ["overview", /^overview|selecteditem|watchlist/i],
  ["commerce", /market|wallet|contract|industry|job_board/i],
  ["ship", /^ship|fitting/i],
  ["inventory", /inventory|assets|cargo|loot/i],
  ["social", /fleet|mail|message|notification/i],
];

export function categorize(windowName: string): WfCategory {
  for (const [key, re] of RULES) {
    if (re.test(windowName)) {
      return WF_CATEGORIES.find((c) => c.key === key)!;
    }
  }
  return WF_CATEGORIES[WF_CATEGORIES.length - 1];
}

import type { GroupDiff } from "../types";
import { groupMessageKey } from "../settingsGroups";
import { useI18n } from "../i18n";

/**
 * A semantic diff rendered as ruled lines: group label, what happened
 * to it, and - when the decoder could see inside - which entries moved.
 */
export function DiffLines({ groups }: { groups: GroupDiff[] }) {
  const { t } = useI18n();
  const label = (g: string) => {
    const key = groupMessageKey(g);
    return key ? t(key) : g;
  };
  return (
    <ul className="diff-lines">
      {groups.map((g) => {
        const counts: string[] = [];
        if (g.entries_changed)
          counts.push(t("diff_n_changed", { n: g.entries_changed }));
        if (g.entries_added)
          counts.push(t("diff_n_added", { n: g.entries_added }));
        if (g.entries_removed)
          counts.push(t("diff_n_removed", { n: g.entries_removed }));
        const word =
          g.kind === "added"
            ? t("diff_added")
            : g.kind === "removed"
            ? t("diff_removed")
            : t("diff_changed");
        return (
          <li key={g.group} className={"diff-line " + g.kind}>
            <span className="diff-group">{label(g.group)}</span>
            <span className="diff-kind">
              {g.kind === "changed" && counts.length
                ? counts.join(", ")
                : word}
            </span>
            {g.entry_names.length > 0 && (
              <span className="diff-names">{g.entry_names.join(", ")}</span>
            )}
          </li>
        );
      })}
    </ul>
  );
}

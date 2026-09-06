import { useEffect, useState } from "react";
import type { CopyGroups, CopySelection } from "../types";
import { groupMessageKey } from "../settingsGroups";
import { useI18n, type MessageKey } from "../i18n";

interface Props {
  /** Fetches the available groups when the picker mounts. */
  load: () => Promise<CopyGroups>;
  /** null = copy everything (byte-identical files). */
  onChange: (sel: CopySelection | null) => void;
  /** Overrides for reuse outside the copy flow (restore). */
  titleKey?: MessageKey;
  noteKey?: MessageKey;
}

/**
 * The "what to copy" control shared by the copy and template dialogs.
 * Defaults to everything; ticking "choose what to copy" reveals the
 * settings groups of the source's char and account files, all checked.
 * Any custom selection routes the copy through a decode-merge of EVE's
 * settings format, so unchecked groups keep each target's own values.
 */
export function GroupPicker({ load, onChange, titleKey, noteKey }: Props) {
  const { t } = useI18n();
  const [groups, setGroups] = useState<CopyGroups | null>(null);
  const [failed, setFailed] = useState(false);
  const [custom, setCustom] = useState(false);
  const [chars, setChars] = useState<Set<string>>(new Set());
  const [users, setUsers] = useState<Set<string>>(new Set());

  const label = (g: string) => {
    const key = groupMessageKey(g);
    return key ? t(key) : g;
  };

  useEffect(() => {
    let alive = true;
    load()
      .then((g) => {
        if (!alive) return;
        setGroups(g);
        setChars(new Set(g.char_groups));
        setUsers(new Set(g.user_groups));
      })
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selectable =
    groups !== null && groups.char_groups.length + groups.user_groups.length > 0;

  function emit(nextCustom: boolean, c: Set<string>, u: Set<string>) {
    onChange(
      nextCustom
        ? { char_groups: Array.from(c), user_groups: Array.from(u) }
        : null
    );
  }

  function toggleCustom() {
    const next = !custom;
    setCustom(next);
    emit(next, chars, users);
  }

  function toggle(set: "chars" | "users", key: string) {
    const src = set === "chars" ? chars : users;
    const next = new Set(src);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    if (set === "chars") {
      setChars(next);
      emit(custom, next, users);
    } else {
      setUsers(next);
      emit(custom, chars, next);
    }
  }

  if (failed || (groups !== null && !selectable)) {
    return <p className="picker-unavailable">{t("selective_unavailable")}</p>;
  }

  return (
    <div className="picker">
      <label className="picker-mode">
        <input
          type="checkbox"
          className="checkbox"
          checked={custom}
          onChange={toggleCustom}
          disabled={groups === null}
        />
        <span>{t(titleKey ?? "choose_what")}</span>
      </label>

      {custom && groups && (
        <>
          <div className="picker-columns">
            <div>
              <div className="picker-head">{t("char_settings")}</div>
              {groups.char_groups.map((g) => (
                <label key={g} className="picker-item">
                  <input
                    type="checkbox"
                    className="checkbox"
                    checked={chars.has(g)}
                    onChange={() => toggle("chars", g)}
                  />
                  <span>{label(g)}</span>
                </label>
              ))}
            </div>
            <div>
              <div className="picker-head">{t("account_settings")}</div>
              {groups.user_groups.length === 0 ? (
                <div className="picker-none">{t("not_available_source")}</div>
              ) : (
                groups.user_groups.map((g) => (
                  <label key={g} className="picker-item">
                    <input
                      type="checkbox"
                      className="checkbox"
                      checked={users.has(g)}
                      onChange={() => toggle("users", g)}
                    />
                    <span>{label(g)}</span>
                  </label>
                ))
              )}
            </div>
          </div>
          <p className="help picker-note">{t(noteKey ?? "picker_note")}</p>
        </>
      )}
    </div>
  );
}

import { useState } from "react";
import { api } from "../api";
import type { AccountEntry, CharacterEntry } from "../types";
import { useI18n } from "../i18n";

interface Props {
  accounts: AccountEntry[];
  /** The full roster, for surfacing characters the launcher-log
   *  mapping hasn't tied to an account yet. */
  characters: CharacterEntry[];
  /** Character portraits as data URIs, keyed by character id. */
  portraits: Record<string, string>;
  onAfterMutation: (msg: string) => void;
  onError: (e: string) => void;
}

/**
 * The account register: one ruled sheet, one row per account that has
 * a crew or an annotation, the crew's portraits fanned at the left and
 * the numeric id demoted to small print. Stray user files with nothing
 * attached wait in a drawer below. Aliases and notes live in
 * Replicator's database only - the EVE folder is not touched.
 */
export function AccountsView({
  accounts,
  characters,
  portraits,
  onAfterMutation,
  onError,
}: Props) {
  const { t } = useI18n();
  const [editing, setEditing] = useState<AccountEntry | null>(null);
  const [assigning, setAssigning] = useState<AssignTarget | null>(null);

  const charLabel = (id: number, name: string | null) =>
    name ?? t("character_n", { id });
  const accountLabel = (userId: number) => {
    const acc = accounts.find((a) => a.user_id === userId);
    return acc?.alias ?? `#${userId}`;
  };

  async function assign(target: AssignTarget, userId: number | null) {
    try {
      await api.setCharAccount(target.id, userId);
      onAfterMutation(
        userId === null
          ? t("msg_cleared_pair", { name: charLabel(target.id, target.name) })
          : t("msg_assigned", {
              name: charLabel(target.id, target.name),
              account: accountLabel(userId),
            })
      );
    } catch (e) {
      onError(String(e));
    }
    setAssigning(null);
  }

  if (accounts.length === 0) {
    return (
      <div className="card">
        <h3>{t("no_accounts_title")}</h3>
        <p className="help">{t("no_accounts_help")}</p>
      </div>
    );
  }

  const registered = accounts
    .filter((a) => a.characters.length > 0 || a.alias || a.note)
    .sort(
      (a, b) =>
        b.characters.length - a.characters.length || a.user_id - b.user_id
    );
  const stray = accounts.filter(
    (a) => !(a.characters.length > 0 || a.alias || a.note)
  );
  const unmappedChars = characters.filter((c) => c.user_id === null);

  return (
    <>
      <div className="action-bar">
        <span className="muted">
          {accounts.length === 1
            ? t("one_account")
            : t("n_accounts", { n: accounts.length })}
        </span>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="fine">{t("accounts_fine")}</span>
      </div>

      {registered.length > 0 && (
        <div className="areg">
          {registered.map((a) => {
            const names = a.characters
              .map((c) => c.name ?? `#${c.id}`)
              .join(", ");
            return (
              <div key={a.user_id} className="areg-row">
                <div className="areg-faces">
                  {a.characters.map((c) => {
                    const label = c.name ?? `#${c.id}`;
                    const portrait = portraits[String(c.id)];
                    const cls =
                      "areg-face" +
                      (c.manual ? " manual" : "") +
                      (c.disputed ? " disputed" : "");
                    const title = c.disputed
                      ? `${label} · ${t("logs_disagree_tip", {
                          account: accountLabel(c.log_user_id ?? 0),
                        })}`
                      : c.manual
                      ? `${label} · ${t("set_by_hand")}`
                      : label;
                    // Hand-set pairings stay editable: click the face
                    // to move or clear it. Logged ones record reality
                    // and are not clickable.
                    const onClick = c.manual
                      ? () =>
                          setAssigning({
                            id: c.id,
                            name: c.name,
                            currentUserId: a.user_id,
                            manual: true,
                            logUserId: c.disputed ? c.log_user_id : null,
                          })
                      : undefined;
                    return portrait ? (
                      <img
                        key={c.id}
                        className={cls}
                        src={portrait}
                        alt=""
                        title={title}
                        onClick={onClick}
                      />
                    ) : (
                      <div
                        key={c.id}
                        className={cls + " mono"}
                        title={title}
                        onClick={onClick}
                        aria-hidden
                      >
                        {label.charAt(0).toUpperCase()}
                      </div>
                    );
                  })}
                </div>
                <div className="areg-body">
                  <div className="areg-title">
                    {a.alias ?? (names || <em>{t("no_chars_mapped")}</em>)}
                  </div>
                  {((a.alias && names) || a.note) && (
                    <div className="areg-sub">
                      {a.alias && names ? names : null}
                      {a.alias && names && a.note ? " · " : null}
                      {a.note ? <em>{a.note}</em> : null}
                    </div>
                  )}
                </div>
                <span className="areg-leader" aria-hidden />
                <span className="areg-id">#{a.user_id}</span>
                <button
                  className="ghost areg-edit"
                  onClick={() => setEditing(a)}
                >
                  {t("edit_ellipsis")}
                </button>
              </div>
            );
          })}
        </div>
      )}

      {unmappedChars.length > 0 && (
        <details className="acct-more">
          <summary>
            {unmappedChars.length === 1
              ? t("unmapped_chars_one")
              : t("unmapped_chars_n", { n: unmappedChars.length })}
          </summary>
          <p className="help acct-more-help">{t("mapping_explainer")}</p>
          <div className="acct-more-rows">
            {unmappedChars.map((c) => {
              const label = c.name ?? t("character_n", { id: c.id });
              const portrait = portraits[String(c.id)];
              return (
                <button
                  key={c.id}
                  className="ghost acct-strag"
                  title={t("assign_ellipsis")}
                  onClick={() =>
                    setAssigning({
                      id: c.id,
                      name: c.name,
                      currentUserId: null,
                      manual: false,
                      logUserId: null,
                    })
                  }
                >
                  {portrait ? (
                    <img className="acct-strag-face" src={portrait} alt="" />
                  ) : (
                    <span className="acct-strag-face mono" aria-hidden>
                      {label.charAt(0).toUpperCase()}
                    </span>
                  )}
                  <span className="acct-strag-name">{label}</span>
                </button>
              );
            })}
          </div>
        </details>
      )}

      {stray.length > 0 && (
        <details className="acct-more">
          <summary>
            {stray.length === 1
              ? t("unmapped_one")
              : t("unmapped_n", { n: stray.length })}
          </summary>
          <div className="acct-more-rows">
            {stray.map((a) => (
              <button
                key={a.user_id}
                className="ghost acct-more-id"
                title={t("edit_ellipsis")}
                onClick={() => setEditing(a)}
              >
                #{a.user_id}
              </button>
            ))}
          </div>
        </details>
      )}

      {assigning && (
        <AssignAccountDialog
          target={assigning}
          accounts={accounts}
          onClose={() => setAssigning(null)}
          onConfirm={(uid) => assign(assigning, uid)}
          onClear={() => assign(assigning, null)}
        />
      )}

      {editing && (
        <AccountEditDialog
          account={editing}
          onClose={() => setEditing(null)}
          onConfirm={async (alias, note) => {
            try {
              await api.setAccountMeta(
                editing.user_id,
                alias || null,
                note || null
              );
              onAfterMutation(t("msg_account_saved"));
              setEditing(null);
            } catch (e) {
              onError(String(e));
              setEditing(null);
            }
          }}
        />
      )}
    </>
  );
}

/** A character being filed under an account by hand. */
interface AssignTarget {
  id: number;
  name: string | null;
  currentUserId: number | null;
  manual: boolean;
  /** Set when the launcher logs contradict the hand-set pairing. */
  logUserId: number | null;
}

interface AssignProps {
  target: AssignTarget;
  accounts: AccountEntry[];
  onClose: () => void;
  onConfirm: (userId: number) => void;
  onClear: () => void;
}

function AssignAccountDialog({
  target,
  accounts,
  onClose,
  onConfirm,
  onClear,
}: AssignProps) {
  const { t } = useI18n();
  const [pick, setPick] = useState<number | "">(target.currentUserId ?? "");

  const sorted = [...accounts].sort(
    (a, b) =>
      b.characters.length - a.characters.length || a.user_id - b.user_id
  );
  const shortLabel = (userId: number) =>
    accounts.find((a) => a.user_id === userId)?.alias ?? `#${userId}`;
  const optionLabel = (a: AccountEntry) => {
    const base = a.alias ?? `#${a.user_id}`;
    const names = a.characters.map((c) => c.name ?? `#${c.id}`).join(", ");
    return names ? `${base} · ${names}` : base;
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>
          {t("assign_title", {
            name: target.name ?? t("character_n", { id: target.id }),
          })}
        </h2>
        <p className="help">{t("assign_help")}</p>
        {target.logUserId !== null && (
          <p className="dispute-note">
            {t("logs_disagree_tip", {
              account: shortLabel(target.logUserId),
            })}
          </p>
        )}
        <div style={{ marginTop: 12 }}>
          <select
            value={pick}
            onChange={(e) =>
              setPick(e.target.value === "" ? "" : Number(e.target.value))
            }
          >
            <option value="">{t("assign_pick")}</option>
            {sorted.map((a) => (
              <option key={a.user_id} value={a.user_id}>
                {optionLabel(a)}
              </option>
            ))}
          </select>
        </div>
        <div className="actions">
          {target.manual && (
            <button className="danger" onClick={onClear}>
              {target.logUserId !== null
                ? t("use_logs_account", {
                    account: shortLabel(target.logUserId),
                  })
                : t("assign_clear")}
            </button>
          )}
          <button onClick={onClose}>{t("cancel")}</button>
          <button
            className="primary"
            disabled={pick === "" || pick === target.currentUserId}
            onClick={() => pick !== "" && onConfirm(pick)}
          >
            {t("save")}
          </button>
        </div>
      </div>
    </div>
  );
}

interface EditProps {
  account: AccountEntry;
  onClose: () => void;
  onConfirm: (alias: string, note: string) => void;
}

function AccountEditDialog({ account, onClose, onConfirm }: EditProps) {
  const { t } = useI18n();
  const [alias, setAlias] = useState(account.alias ?? "");
  const [note, setNote] = useState(account.note ?? "");
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{account.alias ?? t("account_n", { id: account.user_id })}</h2>
        <p className="help">
          <span className="oid">#{account.user_id}</span>
          {account.characters.length > 0 && (
            <>
              {" · "}
              {account.characters
                .map((c) => c.name ?? `#${c.id}`)
                .join(", ")}
            </>
          )}
        </p>
        <div style={{ marginTop: 12 }}>
          <label className="field-label">{t("alias_label")}</label>
          <input
            type="text"
            value={alias}
            onChange={(e) => setAlias(e.target.value)}
            placeholder={t("alias_placeholder")}
            autoFocus
          />
        </div>
        <div style={{ marginTop: 12 }}>
          <label className="field-label">{t("note_label")}</label>
          <textarea
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder={t("note_placeholder")}
            rows={3}
          />
        </div>
        <div className="actions">
          <button onClick={onClose}>{t("cancel")}</button>
          <button
            className="primary"
            onClick={() => onConfirm(alias.trim(), note.trim())}
          >
            {t("save")}
          </button>
        </div>
      </div>
    </div>
  );
}

import { useState } from "react";
import { api } from "../api";
import type { CharacterEntry, CopySelection } from "../types";
import { GroupPicker } from "./GroupPicker";
import { useI18n } from "../i18n";

interface Props {
  source: CharacterEntry;
  targets: CharacterEntry[];
  onClose: () => void;
  onConfirm: (
    selection: CopySelection | null,
    crossServer: boolean
  ) => void | Promise<void>;
}

export function CopyDialog({ source, targets, onClose, onConfirm }: Props) {
  const { t } = useI18n();
  // null = copy everything, byte-identical.
  const [selection, setSelection] = useState<CopySelection | null>(null);
  const [crossServer, setCrossServer] = useState(false);
  const [busy, setBusy] = useState(false);
  const emptySelection =
    selection !== null &&
    selection.char_groups.length === 0 &&
    selection.user_groups.length === 0;

  // Offer the cross-server switch only when it would change anything.
  const servers = new Set<string>([
    ...source.servers,
    ...targets.flatMap((tc) => tc.servers),
  ]);
  const spansServers = servers.size > 1;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{t("confirm_copy_title")}</h2>
        <p>
          {t("confirm_copy_body", { name: source.name ?? `#${source.id}` })}
        </p>
        <div className="list" style={{ maxHeight: 280, overflow: "auto" }}>
          {targets.map((tc) => (
            <div key={tc.id} className="list-item">
              <span className="grow">
                {tc.name ?? t("character_n", { id: tc.id })}
                <span className="meta"> · #{tc.id}</span>
              </span>
            </div>
          ))}
        </div>

        <GroupPicker
          load={() => api.copyGroups(source.id)}
          onChange={setSelection}
        />

        {spansServers && (
          <label className="picker-mode" style={{ marginTop: 10 }}>
            <input
              type="checkbox"
              className="checkbox"
              checked={crossServer}
              onChange={() => setCrossServer(!crossServer)}
            />
            <span>{t("cross_server_label")}</span>
          </label>
        )}

        {(selection === null || selection.user_groups.length > 0) && (
          <p className="help" style={{ marginTop: 10 }}>
            {t("copy_account_note")}
          </p>
        )}
        <p className="help" style={{ marginTop: 10 }}>
          {t("copy_snapshot_note")}
        </p>
        <div className="actions">
          <button onClick={onClose}>{t("cancel")}</button>
          <button
            className="primary"
            disabled={emptySelection || busy}
            title={emptySelection ? t("nothing_selected") : ""}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm(selection, crossServer);
              } finally {
                setBusy(false);
              }
            }}
          >
            {t("copy")}
          </button>
        </div>
      </div>
    </div>
  );
}

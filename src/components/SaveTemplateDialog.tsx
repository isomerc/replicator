import { useState } from "react";
import type { CharacterEntry } from "../types";
import { useI18n } from "../i18n";

interface Props {
  source: CharacterEntry;
  onClose: () => void;
  onConfirm: (name: string) => void | Promise<void>;
}

export function SaveTemplateDialog({ source, onClose, onConfirm }: Props) {
  const { t } = useI18n();
  const [name, setName] = useState(source.name ?? `Template-${source.id}`);
  const [busy, setBusy] = useState(false);

  async function submit() {
    if (busy || !name.trim()) return;
    setBusy(true);
    try {
      await onConfirm(name.trim());
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{t("save_template_title")}</h2>
        <p className="help">
          {t("save_template_help", { name: source.name ?? `#${source.id}` })}
        </p>
        <div style={{ marginTop: 10 }}>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("template_name_placeholder")}
            autoFocus
          />
        </div>
        <div className="actions">
          <button onClick={onClose}>{t("cancel")}</button>
          <button
            className="primary"
            onClick={submit}
            disabled={busy || !name.trim()}
          >
            {t("save")}
          </button>
        </div>
      </div>
    </div>
  );
}

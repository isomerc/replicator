import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { InstallSummary } from "../types";
import { LOCALES, LOCALE_NAMES, useI18n, type Locale } from "../i18n";

interface Props {
  installs: InstallSummary | null;
  patPresent: boolean;
  onAfterMutation: (msg: string) => void;
  onError: (e: string) => void;
}

export function SettingsView({
  installs,
  patPresent,
  onAfterMutation,
  onError,
}: Props) {
  const { t, locale, setLocale } = useI18n();
  const [pat, setPat] = useState("");

  async function pickFolder() {
    try {
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === "string") {
        await api.setInstallOverride(picked);
        onAfterMutation(t("msg_override_set", { path: picked }));
      }
    } catch (e) {
      onError(String(e));
    }
  }

  async function clearOverride() {
    try {
      await api.setInstallOverride(null);
      onAfterMutation(t("msg_override_cleared"));
    } catch (e) {
      onError(String(e));
    }
  }

  async function savePat() {
    if (!pat.trim()) return;
    try {
      await api.patSet(pat.trim());
      setPat("");
      onAfterMutation(t("msg_pat_saved"));
    } catch (e) {
      onError(String(e));
    }
  }

  async function clearPat() {
    try {
      await api.patClear();
      onAfterMutation(t("msg_pat_removed"));
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <>
      <div className="card" style={{ marginBottom: 14 }}>
        <h3>{t("installs_title")}</h3>
        {installs?.installs.length === 0 ? (
          <p className="help">{t("installs_help")}</p>
        ) : (
          <div className="list">
            {installs?.installs.map((inst) => (
              <div key={inst.root} className="list-item">
                <span className="grow">
                  <strong>{inst.label}</strong>
                  <div className="meta">
                    <code>{inst.root}</code>
                    <div>
                      {inst.settings_dirs.length === 1
                        ? t("one_settings_dir")
                        : t("n_settings_dirs", {
                            n: inst.settings_dirs.length,
                          })}
                    </div>
                  </div>
                </span>
              </div>
            ))}
          </div>
        )}
        <div className="row" style={{ marginTop: 10 }}>
          <button onClick={pickFolder}>{t("browse")}</button>
          {installs?.override_path && (
            <>
              <span className="muted">
                {t("override_label")} <code>{installs.override_path}</code>
              </span>
              <button className="ghost" onClick={clearOverride}>
                {t("clear")}
              </button>
            </>
          )}
        </div>
      </div>

      <div className="card" style={{ marginBottom: 14 }}>
        <h3>{t("language_title")}</h3>
        <p className="help">{t("language_help")}</p>
        <div className="row" style={{ marginTop: 10 }}>
          <select
            value={locale}
            onChange={(e) => setLocale(e.target.value as Locale)}
            style={{ maxWidth: 220 }}
          >
            {LOCALES.map((l) => (
              <option key={l} value={l}>
                {LOCALE_NAMES[l]}
              </option>
            ))}
          </select>
        </div>
      </div>

      <div className="card">
        <h3>{t("pat_title")}</h3>
        <p className="help">{t("pat_help")}</p>
        <div className="row" style={{ marginTop: 10 }}>
          <input
            type="password"
            value={pat}
            placeholder={
              patPresent
                ? t("pat_placeholder_stored")
                : t("pat_placeholder_empty")
            }
            onChange={(e) => setPat(e.target.value)}
            autoComplete="off"
          />
          <button className="primary" onClick={savePat} disabled={!pat.trim()}>
            {t("save")}
          </button>
          {patPresent && (
            <button className="danger" onClick={clearPat}>
              {t("remove")}
            </button>
          )}
        </div>
      </div>
    </>
  );
}

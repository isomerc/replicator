import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type {
  CharacterEntry,
  CopySelection,
  Template,
  WindowLayout,
} from "../types";
import { BlueprintDialog } from "./BlueprintDialog";
import { GroupPicker } from "./GroupPicker";
import { LayoutWireframe } from "./LayoutWireframe";
import { ReportDialog } from "./ReportDialog";
import { useI18n } from "../i18n";

const ZIP_FILTER = [{ name: "Zip archive", extensions: ["zip"] }];

interface Props {
  templates: Template[];
  characters: CharacterEntry[];
  eveRunning: boolean;
  onAfterMutation: (msg: string) => void;
  onError: (e: string) => void;
}

/**
 * The shelf. Every saved template is a pack: the profile you froze,
 * sitting there until you take it down and hand it to somebody.
 */
export function TemplatesView({
  templates,
  characters,
  eveRunning,
  onAfterMutation,
  onError,
}: Props) {
  const { t } = useI18n();
  const [applying, setApplying] = useState<Template | null>(null);
  const [report, setReport] = useState<{
    title: string;
    written: string[];
    skipped: string[];
  } | null>(null);
  const [layouts, setLayouts] = useState<Record<number, WindowLayout | null>>(
    {}
  );
  const [blueprint, setBlueprint] = useState<{
    layout: WindowLayout;
    title: string;
  } | null>(null);

  // Each pack shows the layout it froze - fetched once per template.
  useEffect(() => {
    for (const tpl of templates) {
      if (layouts[tpl.id] !== undefined) continue;
      setLayouts((prev) => ({ ...prev, [tpl.id]: null }));
      api
        .templateLayout(tpl.id)
        .then((l) => setLayouts((prev) => ({ ...prev, [tpl.id]: l })))
        .catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [templates]);

  async function remove(tpl: Template) {
    if (!confirm(t("confirm_delete_template", { name: tpl.name }))) return;
    try {
      await api.deleteTemplate(tpl.id);
      onAfterMutation(t("msg_deleted", { name: tpl.name }));
    } catch (e) {
      onError(String(e));
    }
  }

  async function exportTemplate(tpl: Template) {
    try {
      const safe = tpl.name.replace(/[\\/:*?"<>|]/g, "-");
      const path = await save({
        defaultPath: `${safe}.zip`,
        filters: ZIP_FILTER,
      });
      if (!path) return;
      await api.exportTemplate(tpl.id, path);
      onAfterMutation(t("msg_exported_template", { name: tpl.name, path }));
    } catch (e) {
      onError(String(e));
    }
  }

  async function importTemplate() {
    try {
      const path = await open({ multiple: false, filters: ZIP_FILTER });
      if (typeof path !== "string") return;
      const tpl = await api.importTemplate(path);
      onAfterMutation(t("msg_imported_template", { name: tpl.name }));
    } catch (e) {
      onError(String(e));
    }
  }

  if (templates.length === 0) {
    return (
      <div className="card">
        <h3>{t("shelf_empty_title")}</h3>
        <p className="help">{t("shelf_empty_help")}</p>
        <div className="row" style={{ marginTop: 10 }}>
          <button onClick={importTemplate}>{t("import_zip")}</button>
        </div>
      </div>
    );
  }

  return (
    <>
      <div className="action-bar">
        <div className="spacer" style={{ flex: 1 }} />
        <button onClick={importTemplate}>{t("import_zip")}</button>
      </div>

      <div className="shelf">
        {templates.map((tpl) => (
          <div key={tpl.id} className="pack">
            <div className="pack-inner">
              <div className="pack-kicker">{t("template_kicker")}</div>
              <div className="pack-name">{tpl.name}</div>
              <div className="pack-script">
                {t("from_name", {
                  name: tpl.source_name ?? `#${tpl.source_character_id ?? "?"}`,
                })}
              </div>
              <div className="pack-fine">
                {(tpl.size / 1024).toFixed(1)} KB ·{" "}
                {new Date(tpl.created_at * 1000).toLocaleDateString()}
              </div>
              {layouts[tpl.id] && (
                <div
                  className="pack-wireframe wf-open"
                  role="button"
                  title={t("layout_label")}
                  onClick={() =>
                    setBlueprint({
                      layout: layouts[tpl.id]!,
                      title: tpl.name,
                    })
                  }
                >
                  <LayoutWireframe layout={layouts[tpl.id]!} />
                </div>
              )}
              {!tpl.has_user_data && (
                <div className="pack-warn" title={t("char_file_only_tip")}>
                  {t("char_file_only")}
                </div>
              )}
            </div>
            <div className="pack-actions">
              <button
                className="primary"
                disabled={eveRunning}
                onClick={() => setApplying(tpl)}
              >
                {t("apply_ellipsis")}
              </button>
              <button onClick={() => exportTemplate(tpl)}>
                {t("export_ellipsis")}
              </button>
              <button className="danger" onClick={() => remove(tpl)}>
                {t("delete")}
              </button>
            </div>
          </div>
        ))}
      </div>

      {applying && (
        <ApplyTemplateDialog
          template={applying}
          characters={characters}
          onClose={() => setApplying(null)}
          onConfirm={async (ids, selection, crossServer) => {
            try {
              const r = await api.applyTemplate(
                applying.id,
                ids,
                selection,
                crossServer
              );
              onAfterMutation(
                t("msg_applied_template", {
                  name: applying.name,
                  n: r.written.length,
                }) +
                  (r.skipped.length
                    ? t("msg_skipped_suffix", { n: r.skipped.length })
                    : "")
              );
              if (r.skipped.length) {
                setReport({ title: t("report_apply_title"), ...r });
              }
              setApplying(null);
            } catch (e) {
              onError(String(e));
              setApplying(null);
            }
          }}
        />
      )}

      {blueprint && (
        <BlueprintDialog
          layout={blueprint.layout}
          title={blueprint.title}
          onClose={() => setBlueprint(null)}
        />
      )}

      {report && (
        <ReportDialog
          title={report.title}
          written={report.written}
          skipped={report.skipped}
          onClose={() => setReport(null)}
        />
      )}
    </>
  );
}

interface ApplyProps {
  template: Template;
  characters: CharacterEntry[];
  /** Targets already chosen elsewhere (the order form): shown as a
   *  read-only list instead of the picker. */
  fixedTargets?: CharacterEntry[];
  onClose: () => void;
  onConfirm: (
    ids: number[],
    selection: CopySelection | null,
    crossServer: boolean
  ) => void | Promise<void>;
}

/**
 * The one confirmation every template apply goes through, whether the
 * targets are ticked here or arrived pre-picked from the order form:
 * the group picker, the cross-server switch when a target has a
 * profile on a server the template did not come from, and the note
 * about which account files an apply reaches.
 */
export function ApplyTemplateDialog({
  template,
  characters,
  fixedTargets,
  onClose,
  onConfirm,
}: ApplyProps) {
  const { t } = useI18n();
  const [picked, setPicked] = useState<Set<number>>(
    new Set((fixedTargets ?? []).map((c) => c.id))
  );
  const [selection, setSelection] = useState<CopySelection | null>(null);
  const [crossServer, setCrossServer] = useState(false);
  const [busy, setBusy] = useState(false);
  const emptySelection =
    selection !== null &&
    selection.char_groups.length === 0 &&
    selection.user_groups.length === 0;
  const from = template.source_server;
  const spansServers =
    from !== null &&
    characters.some(
      (c) => picked.has(c.id) && c.servers.some((s) => s !== from)
    );
  const touchesAccounts =
    template.has_user_data &&
    (selection === null || selection.user_groups.length > 0);
  function toggle(id: number) {
    setPicked((p) => {
      const n = new Set(p);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  }
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{t("apply_template_title", { name: template.name })}</h2>
        {fixedTargets ? (
          <>
            <p>{t("confirm_copy_body", { name: template.name })}</p>
            <div className="list" style={{ maxHeight: 280, overflow: "auto" }}>
              {fixedTargets.map((c) => (
                <div key={c.id} className="list-item">
                  <span className="grow">
                    {c.name ?? t("character_n", { id: c.id })}
                    <span className="meta"> · #{c.id}</span>
                  </span>
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <p className="help">{t("apply_template_help")}</p>
            <div
              className="list"
              style={{ maxHeight: "55vh", overflow: "auto" }}
            >
              {characters.map((c) => (
                <label
                  key={c.id}
                  className="list-item"
                  style={{ cursor: "pointer" }}
                >
                  <input
                    type="checkbox"
                    className="checkbox"
                    checked={picked.has(c.id)}
                    onChange={() => toggle(c.id)}
                  />
                  <span className="grow">
                    {c.name ?? t("character_n", { id: c.id })}
                    <span className="meta"> · #{c.id}</span>
                  </span>
                </label>
              ))}
            </div>
          </>
        )}
        <GroupPicker
          load={() => api.templateGroups(template.id)}
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

        {touchesAccounts && (
          <p className="help" style={{ marginTop: 10 }}>
            {t("copy_account_note")}
          </p>
        )}
        {fixedTargets && (
          <p className="help" style={{ marginTop: 10 }}>
            {t("copy_snapshot_note")}
          </p>
        )}

        <div className="actions">
          <button onClick={onClose}>{t("cancel")}</button>
          <button
            className="primary"
            disabled={busy || picked.size === 0 || emptySelection}
            title={emptySelection ? t("nothing_selected") : ""}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm(Array.from(picked), selection, crossServer);
              } finally {
                setBusy(false);
              }
            }}
          >
            {t("apply_to_n", { n: picked.size })}
          </button>
        </div>
      </div>
    </div>
  );
}

import { useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type {
  CommitEntry,
  CopySelection,
  FileDiff,
  ImportPreview,
  MirrorStatus,
} from "../types";
import { DiffLines } from "./DiffLines";
import { GroupPicker } from "./GroupPicker";
import { ReportDialog } from "./ReportDialog";
import { useI18n } from "../i18n";

const ZIP_FILTER = [{ name: "Zip archive", extensions: ["zip"] }];

interface Props {
  status: MirrorStatus | null;
  history: CommitEntry[];
  patPresent: boolean;
  eveRunning: boolean;
  onAfterMutation: (msg: string) => void;
  onError: (e: string) => void;
}

/**
 * The passbook. History on the left as a ledger; the account panel on
 * the right holds the mirror's facts and every action.
 */
export function GitView({
  status,
  history,
  patPresent,
  eveRunning,
  onAfterMutation,
  onError,
}: Props) {
  const { t } = useI18n();
  const [snapshotMsg, setSnapshotMsg] = useState("");
  const [confirming, setConfirming] = useState<CommitEntry | null>(null);
  const [report, setReport] = useState<{
    title: string;
    restored: number;
    skipped: string[];
  } | null>(null);
  const [editingRemote, setEditingRemote] = useState(false);
  const [remote, setRemote] = useState(status?.remote ?? "");
  const [pushing, setPushing] = useState(false);
  const [pulling, setPulling] = useState(false);
  const [cloneUrl, setCloneUrl] = useState("");
  const [cloning, setCloning] = useState(false);
  const [importPreview, setImportPreview] = useState<{
    path: string;
    p: ImportPreview;
  } | null>(null);
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [diffs, setDiffs] = useState<Record<string, FileDiff[] | "loading">>(
    {}
  );
  const [restoreSel, setRestoreSel] = useState<CopySelection | null>(null);

  const initialized = status?.initialized ?? false;
  const emptyRestoreSel =
    restoreSel !== null &&
    restoreSel.char_groups.length === 0 &&
    restoreSel.user_groups.length === 0;

  function toggleDiff(oid: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(oid)) {
        next.delete(oid);
        return next;
      }
      next.add(oid);
      return next;
    });
    if (!diffs[oid]) {
      setDiffs((d) => ({ ...d, [oid]: "loading" }));
      api
        .gitDiff(oid)
        .then((r) => setDiffs((d) => ({ ...d, [oid]: r })))
        .catch((e) => {
          setDiffs((d) => {
            const next = { ...d };
            delete next[oid];
            return next;
          });
          // Collapse too: an expanded row with no diff renders as a
          // forever-"Loading" with nothing in flight.
          setExpanded((prev) => {
            const next = new Set(prev);
            next.delete(oid);
            return next;
          });
          onError(String(e));
        });
    }
  }

  const [snapshotting, setSnapshotting] = useState(false);
  const [restoring, setRestoring] = useState(false);

  async function snapshot() {
    if (snapshotting) return;
    setSnapshotting(true);
    try {
      const oid = await api.gitSnapshot(snapshotMsg.trim() || undefined);
      if (oid === null) {
        onAfterMutation(t("msg_no_changes"));
      } else {
        onAfterMutation(t("msg_snapshot_created", { short: oid.substring(0, 7) }));
      }
      setSnapshotMsg("");
    } catch (e) {
      onError(String(e));
    } finally {
      setSnapshotting(false);
    }
  }

  async function push() {
    if (!patPresent) {
      onError(t("set_pat_first_error"));
      return;
    }
    setPushing(true);
    try {
      await api.gitPush();
      onAfterMutation(t("msg_pushed"));
    } catch (e) {
      onError(String(e));
    } finally {
      setPushing(false);
    }
  }

  async function pull() {
    setPulling(true);
    try {
      const r = await api.gitPull();
      onAfterMutation(r.updated ? t("msg_pulled") : t("msg_up_to_date"));
    } catch (e) {
      onError(String(e));
    } finally {
      setPulling(false);
    }
  }

  async function cloneRemote() {
    if (cloning) return;
    setCloning(true);
    try {
      await api.gitClone(cloneUrl.trim());
      onAfterMutation(t("msg_cloned"));
      setCloneUrl("");
    } catch (e) {
      onError(String(e));
    } finally {
      setCloning(false);
    }
  }

  async function saveRemote() {
    try {
      await api.gitSetRemote(remote.trim());
      onAfterMutation(t("msg_remote_saved"));
      setEditingRemote(false);
    } catch (e) {
      onError(String(e));
    }
  }

  async function exportZip() {
    if (exporting) return;
    // Set before the native dialog opens: a double-click must not
    // spawn two save dialogs.
    setExporting(true);
    try {
      const stamp = new Date().toISOString().slice(0, 10);
      const path = await save({
        defaultPath: `replicator-setup-${stamp}.zip`,
        filters: ZIP_FILTER,
      });
      if (!path) return;
      const s = await api.exportSetup(path);
      onAfterMutation(t("msg_exported", { n: s.files, path }));
    } catch (e) {
      onError(String(e));
    } finally {
      setExporting(false);
    }
  }

  async function pickImport() {
    try {
      const path = await open({ multiple: false, filters: ZIP_FILTER });
      if (typeof path !== "string") return;
      const p = await api.previewImport(path);
      if (p.kind === "template") {
        // A template archive only adds to the shelf - no live file is
        // touched, so no confirmation stands between click and done.
        const tpl = await api.importTemplate(path);
        onAfterMutation(t("msg_imported_template", { name: tpl.name }));
        return;
      }
      setImportPreview({ path, p });
    } catch (e) {
      onError(String(e));
    }
  }

  async function doImport() {
    if (!importPreview) return;
    setImporting(true);
    try {
      const r = await api.importSetup(importPreview.path);
      onAfterMutation(
        t("msg_imported", { n: r.written.length }) +
          (r.skipped.length
            ? t("msg_skipped_suffix", { n: r.skipped.length })
            : "")
      );
      if (r.skipped.length) {
        setReport({
          title: t("import_report_title"),
          restored: r.written.length,
          skipped: r.skipped,
        });
      }
    } catch (e) {
      onError(String(e));
    } finally {
      setImporting(false);
      setImportPreview(null);
    }
  }

  async function restore(c: CommitEntry) {
    if (restoring) return;
    setRestoring(true);
    try {
      const r = await api.gitRestore(c.oid, restoreSel);
      onAfterMutation(
        t("msg_restored", { n: r.restored, short: c.short }) +
          (r.skipped.length
            ? t("msg_skipped_suffix", { n: r.skipped.length })
            : "")
      );
      if (r.skipped.length) {
        setReport({
          title: t("restore_report_title", { short: c.short }),
          restored: r.restored,
          skipped: r.skipped,
        });
      }
      setConfirming(null);
    } catch (e) {
      onError(String(e));
      setConfirming(null);
    } finally {
      setRestoring(false);
    }
  }

  return (
    <div className="git-layout">
      <div className="ledger-col">
        {history.length === 0 ? (
          <div className="card">
            <h3>{t("ledger_empty_title")}</h3>
            <p className="help">{t("ledger_empty_help")}</p>
          </div>
        ) : (
          <div className="ledger">
            <div className="ledger-head">
              <span />
              <span>{t("entry")}</span>
              <span className="ledger-head-right">{t("recorded")}</span>
              <span />
            </div>
            {history.map((c) => {
              const d = new Date(c.timestamp * 1000);
              const diff = diffs[c.oid];
              return (
                <div key={c.oid} className="entry">
                  <span className="entry-mark" aria-hidden />
                  <div
                    className="entry-body clickable"
                    title={t("tip_show_changes")}
                    onClick={() => toggleDiff(c.oid)}
                  >
                    <div className="entry-summary">
                      {c.summary || t("no_message")}
                    </div>
                    <span className="oid">{c.short}</span>
                  </div>
                  <div className="entry-date">
                    <div>{d.toLocaleDateString()}</div>
                    <div>{d.toLocaleTimeString()}</div>
                  </div>
                  <button
                    className="danger entry-restore"
                    disabled={eveRunning}
                    title={eveRunning ? t("close_eve_first") : ""}
                    onClick={() => {
                      setRestoreSel(null);
                      setConfirming(c);
                    }}
                  >
                    {t("restore")}
                  </button>
                  {expanded.has(c.oid) && (
                    <div className="entry-diff">
                      {!diff || diff === "loading" ? (
                        <span className="fine">{t("loading")}</span>
                      ) : diff.length === 0 ? (
                        <span className="fine">{t("diff_none_commit")}</span>
                      ) : (
                        diff.map((f) => (
                          <div
                            key={f.kind + f.id + f.profile}
                            className="ediff-file"
                          >
                            <span className="ediff-who">
                              {f.kind === "user"
                                ? t("account_n", { id: f.id })
                                : f.label}
                            </span>
                            {f.status === "changed" ? (
                              <DiffLines groups={f.groups} />
                            ) : (
                              <span className="fine">
                                {f.status === "opaque"
                                  ? t("diff_opaque")
                                  : f.status === "added"
                                  ? t("diff_added")
                                  : t("diff_removed")}
                              </span>
                            )}
                          </div>
                        ))
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <aside className="order">
        <div className="order-label">{t("mirror_label")}</div>
        <div className="facts">
          <div className="fact">
            <span className="fact-label">{t("status_label")}</span>
            <span className="fact-leader" />
            <span className="fact-value">
              {initialized
                ? status?.dirty
                  ? t("status_dirty")
                  : t("status_clean")
                : t("status_uninit")}
            </span>
          </div>
          <div className="fact">
            <span className="fact-label">{t("head_label")}</span>
            <span className="fact-leader" />
            <span className="fact-value">
              {status?.head
                ? `${status.head.substring(0, 7)} (${status.branch})`
                : "-"}
            </span>
          </div>
          <div className="fact">
            <span className="fact-label">{t("remote_label")}</span>
            <span className="fact-leader" />
            <span className="fact-value" title={status?.remote ?? undefined}>
              {status?.remote ?? t("none_label")}
            </span>
          </div>
        </div>
        {editingRemote ? (
          <div className="order-row" style={{ marginTop: 10 }}>
            <input
              type="text"
              value={remote}
              placeholder="https://github.com/you/eve-ui-mirror.git"
              onChange={(e) => setRemote(e.target.value)}
            />
            <button className="primary" onClick={saveRemote}>
              {t("save")}
            </button>
            <button className="ghost" onClick={() => setEditingRemote(false)}>
              {t("cancel")}
            </button>
          </div>
        ) : (
          <button
            className="ghost"
            style={{ marginTop: 6, padding: "2px 0" }}
            onClick={() => {
              setRemote(status?.remote ?? "");
              setEditingRemote(true);
            }}
          >
            {t("edit_remote")}
          </button>
        )}

        {!initialized && (
          <div className="order-extra">
            <div className="order-label">{t("clone_title")}</div>
            <p className="panel-fine">{t("clone_help")}</p>
            <input
              type="text"
              value={cloneUrl}
              placeholder="https://github.com/you/eve-ui-mirror.git"
              onChange={(e) => setCloneUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && cloneUrl.trim() && !cloning) {
                  cloneRemote();
                }
              }}
            />
            <button
              className="primary order-cta"
              disabled={cloning || !cloneUrl.trim()}
              onClick={cloneRemote}
            >
              {cloning ? (
                <>
                  <span className="spinner" />
                  {t("cloning")}
                </>
              ) : (
                t("clone")
              )}
            </button>
          </div>
        )}

        <div className="order-extra">
          <div className="order-label">{t("snapshot_label")}</div>
          <input
            type="text"
            placeholder={t("snapshot_placeholder")}
            value={snapshotMsg}
            onChange={(e) => setSnapshotMsg(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !snapshotting) snapshot();
            }}
          />
          <button
            className="primary order-cta"
            disabled={snapshotting}
            onClick={snapshot}
          >
            {t("take_snapshot")}
          </button>
          <p className="panel-fine">{t("snapshot_fine")}</p>
        </div>

        <div className="order-extra">
          <div className="order-label">{t("sync_label")}</div>
          <div className="order-row">
            <button
              className="order-half"
              onClick={push}
              disabled={pushing || !patPresent || !initialized || !status?.remote}
              title={
                !initialized
                  ? t("take_snapshot_first")
                  : !status?.remote
                  ? t("set_remote_first")
                  : !patPresent
                  ? t("set_pat_first")
                  : ""
              }
            >
              {pushing ? (
                <>
                  <span className="spinner" />
                  {t("pushing")}
                </>
              ) : (
                t("push")
              )}
            </button>
            <button
              className="order-half"
              onClick={pull}
              disabled={pulling || !initialized || !status?.remote}
              title={
                !initialized
                  ? t("clone_or_snapshot_first")
                  : !status?.remote
                  ? t("set_remote_first")
                  : ""
              }
            >
              {pulling ? (
                <>
                  <span className="spinner" />
                  {t("pulling")}
                </>
              ) : (
                t("pull")
              )}
            </button>
          </div>
          {!patPresent && <p className="panel-fine">{t("pat_fine")}</p>}
        </div>

        <div className="order-extra">
          <div className="order-label">{t("portable_label")}</div>
          <div className="order-row">
            <button
              className="order-half"
              onClick={exportZip}
              disabled={exporting}
            >
              {t("export_zip")}
            </button>
            <button
              className="order-half"
              onClick={pickImport}
              disabled={eveRunning}
              title={eveRunning ? t("close_eve_first") : ""}
            >
              {t("import_zip")}
            </button>
          </div>
          <p className="panel-fine">{t("portable_fine")}</p>
        </div>
      </aside>

      {confirming && (
        <div className="modal-backdrop" onClick={() => setConfirming(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>{t("restore_confirm_title")}</h2>
            <p>{t("restore_confirm_body", { short: confirming.short })}</p>
            <p>
              <em>"{confirming.summary || t("no_message")}"</em>
            </p>
            <GroupPicker
              load={() => api.commitGroups(confirming.oid)}
              onChange={setRestoreSel}
              titleKey="choose_what_restore"
              noteKey="picker_note_restore"
            />
            <p className="help">{t("restore_note")}</p>
            <div className="actions">
              <button onClick={() => setConfirming(null)}>{t("cancel")}</button>
              <button
                className="primary"
                disabled={emptyRestoreSel || restoring}
                title={emptyRestoreSel ? t("nothing_selected") : ""}
                onClick={() => restore(confirming)}
              >
                {t("restore")}
              </button>
            </div>
          </div>
        </div>
      )}

      {importPreview && (
        <div className="modal-backdrop" onClick={() => setImportPreview(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>{t("import_confirm_title")}</h2>
            <p className="help">
              {t("import_meta_line", {
                v: importPreview.p.app_version,
                date: new Date(
                  importPreview.p.created_at * 1000
                ).toLocaleDateString(),
              })}
            </p>
            {importPreview.p.characters.length > 0 && (
              <p>
                {t("import_chars_label")}{" "}
                {importPreview.p.characters
                  .map((c) => c.name ?? `#${c.id}`)
                  .join(", ")}
              </p>
            )}
            {importPreview.p.would_write.length === 0 ? (
              <p className="help">{t("import_none_match")}</p>
            ) : (
              <>
                <p>
                  {t("import_would_write", {
                    n: importPreview.p.would_write.length,
                  })}
                </p>
                <div
                  className="list"
                  style={{ maxHeight: "30vh", overflow: "auto" }}
                >
                  {importPreview.p.would_write.map((w) => (
                    <div key={w} className="list-item">
                      <code
                        className="grow"
                        style={{ overflowWrap: "anywhere" }}
                      >
                        {w}
                      </code>
                    </div>
                  ))}
                </div>
              </>
            )}
            {importPreview.p.skipped.length > 0 && (
              <p className="help">
                {t("import_skipped_note", {
                  n: importPreview.p.skipped.length,
                })}
              </p>
            )}
            <p className="help">{t("copy_snapshot_note")}</p>
            <div className="actions">
              <button onClick={() => setImportPreview(null)}>
                {t("cancel")}
              </button>
              <button
                className="primary"
                disabled={
                  importing || importPreview.p.would_write.length === 0
                }
                onClick={doImport}
              >
                {importing && <span className="spinner" />}
                {t("import_word")}
              </button>
            </div>
          </div>
        </div>
      )}

      {report && (
        <ReportDialog
          title={report.title}
          written={[]}
          writtenCount={report.restored}
          skipped={report.skipped}
          onClose={() => setReport(null)}
        />
      )}
    </div>
  );
}

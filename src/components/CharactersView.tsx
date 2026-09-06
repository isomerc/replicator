import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type {
  CharacterEntry,
  CharactersResponse,
  Group,
  Template,
  WindowLayout,
} from "../types";
import { BlueprintDialog } from "./BlueprintDialog";
import { LayoutWireframe } from "./LayoutWireframe";
import { CompareDialog } from "./CompareDialog";
import { CopyDialog } from "./CopyDialog";
import { ReportDialog } from "./ReportDialog";
import { SaveTemplateDialog } from "./SaveTemplateDialog";
import { ApplyTemplateDialog } from "./TemplatesView";
import { useI18n, type T } from "../i18n";

interface Props {
  chars: CharactersResponse | null;
  groups: Group[];
  templates: Template[];
  portraits: Record<string, string>;
  eveRunning: boolean;
  onOpenSettings: () => void;
  onAfterMutation: (msg: string) => void;
  onError: (e: string) => void;
}

/**
 * The album and the order form. Cards are picked on the left; the
 * ticket on the right accumulates the order and holds every action, so
 * nothing lives below the fold of a long roster.
 */
export function CharactersView({
  chars,
  groups,
  templates,
  portraits,
  eveRunning,
  onOpenSettings,
  onAfterMutation,
  onError,
}: Props) {
  const { t } = useI18n();
  const [sourceId, setSourceId] = useState<number | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [showCopy, setShowCopy] = useState(false);
  const [showTemplate, setShowTemplate] = useState(false);
  const [showCompare, setShowCompare] = useState(false);
  const [filter, setFilter] = useState("");
  const [templatePick, setTemplatePick] = useState<number | "">("");
  const [report, setReport] = useState<{
    title: string;
    written: string[];
    skipped: string[];
  } | null>(null);
  const [sourceLayout, setSourceLayout] = useState<WindowLayout | null>(null);
  const [showBlueprint, setShowBlueprint] = useState(false);
  const [layoutNonce, setLayoutNonce] = useState(0);
  const layoutCache = useRef(new Map<number, WindowLayout | null>());

  // Every completed mutation refreshes `chars`, and copies blanket
  // account files broadly - so any refresh invalidates every cached
  // sketch, and selections must not keep ids that left the disk.
  useEffect(() => {
    layoutCache.current.clear();
    setLayoutNonce((n) => n + 1);
    const ids = new Set((chars?.characters ?? []).map((c) => c.id));
    setSelected((prev) => {
      const next = new Set([...prev].filter((id) => ids.has(id)));
      return next.size === prev.size ? prev : next;
    });
    setSourceId((prev) => (prev !== null && !ids.has(prev) ? null : prev));
  }, [chars]);

  // The order panel sketches the source's window layout - decoded from
  // the settings file itself, so picking through the album flips
  // through the actual arrangements.
  useEffect(() => {
    if (sourceId === null) {
      setSourceLayout(null);
      return;
    }
    const cached = layoutCache.current.get(sourceId);
    if (cached !== undefined) {
      setSourceLayout(cached);
      return;
    }
    setSourceLayout(null);
    let alive = true;
    api
      .characterLayout(sourceId)
      .then((l) => {
        layoutCache.current.set(sourceId, l);
        if (alive) setSourceLayout(l);
      })
      .catch(() => {
        layoutCache.current.set(sourceId, null);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourceId, layoutNonce]);

  const list = chars?.characters ?? [];
  const filtered = filter
    ? list.filter(
        (c) =>
          (c.name ?? "").toLowerCase().includes(filter.toLowerCase()) ||
          String(c.id).includes(filter) ||
          c.servers.some((s) => s.includes(filter.toLowerCase()))
      )
    : list;

  const source =
    sourceId === null ? null : list.find((c) => c.id === sourceId) ?? null;
  // The source can be ticked as well; it is never a copy target (the
  // backend skips self-writes), so keep it out of the order and the
  // count rather than silently dropping it later.
  const targets = Array.from(selected)
    .filter((id) => id !== sourceId)
    .map((id) => list.find((c) => c.id === id))
    .filter(Boolean) as CharacterEntry[];

  const skippedSuffix = (n: number) =>
    n ? t("msg_skipped_suffix", { n }) : "";

  function toggle(id: number) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function selectAll() {
    // Additive: a filter narrows what gets added, it must not silently
    // drop targets picked before the filter was typed.
    setSelected((prev) => new Set([...prev, ...filtered.map((c) => c.id)]));
  }
  function clearSel() {
    setSelected(new Set());
  }

  // The template Apply and the group chips overwrite files just like
  // the Copy button, so they get the same dialog: a confirmation, the
  // group picker, and the cross-server switch when it applies. One
  // click used to write immediately.
  const [showApplyTpl, setShowApplyTpl] = useState(false);
  const [copyGroup, setCopyGroup] = useState<Group | null>(null);
  const templateToApply =
    templatePick === ""
      ? null
      : templates.find((x) => x.id === templatePick) ?? null;
  const pickedEntries = Array.from(selected)
    .map((id) => list.find((c) => c.id === id))
    .filter(Boolean) as CharacterEntry[];
  const groupMembers = (g: Group) =>
    g.member_ids
      .filter((id) => id !== sourceId)
      .map((id) => list.find((c) => c.id === id))
      .filter(Boolean) as CharacterEntry[];

  if (!chars) {
    return <div className="muted">{t("loading")}</div>;
  }
  if (chars.characters.length === 0) {
    return (
      <div className="card">
        <h3>{t("no_chars_title")}</h3>
        <p className="help">{t("no_chars_help")}</p>
        <button onClick={onOpenSettings}>{t("open_settings")}</button>
      </div>
    );
  }

  return (
    <div className="chars-layout">
      <div className="roster">
        <div className="roster-tools">
          <input
            type="text"
            placeholder={t("filter_placeholder")}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <span className="fine">{t("n_shown", { n: filtered.length })}</span>
          <div className="spacer" />
          <button className="ghost" onClick={selectAll}>
            {t("select_all")}
          </button>
          <button
            className="ghost"
            onClick={clearSel}
            disabled={selected.size === 0}
          >
            {t("clear")}
          </button>
        </div>

        <div className="album">
          {filtered.map((c) => (
            <CharCard
              key={c.id}
              c={c}
              t={t}
              portrait={portraits[String(c.id)]}
              isSource={c.id === sourceId}
              isSelected={selected.has(c.id)}
              onClick={(e) => {
                if (e.shiftKey) toggle(c.id);
                else setSourceId(c.id);
              }}
              onToggle={() => toggle(c.id)}
            />
          ))}
        </div>
      </div>

      <aside className="order">
        <div className="order-label">{t("source_label")}</div>
        {source ? (
          <>
            <div className="order-source">
              <span className="order-source-name">
                {source.name ?? t("character_n", { id: source.id })}
              </span>
              <span className="order-source-id">#{source.id}</span>
            </div>
            {sourceLayout && (
              <div
                className="order-wireframe wf-open"
                role="button"
                title={t("layout_label")}
                onClick={() => setShowBlueprint(true)}
              >
                <LayoutWireframe layout={sourceLayout} />
              </div>
            )}
          </>
        ) : (
          <div className="order-hint">{t("hint_source")}</div>
        )}

        <div className="order-label">
          {t("targets_label")} <span className="order-count">{targets.length}</span>
        </div>
        {targets.length === 0 ? (
          <div className="order-hint">{t("hint_targets")}</div>
        ) : (
          <ul className="order-targets">
            {targets.map((tc) => (
              <li key={tc.id}>
                <span className="grow">
                  {tc.name ?? t("character_n", { id: tc.id })}
                </span>
                <button
                  className="order-remove"
                  onClick={() => toggle(tc.id)}
                  title={t("remove")}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}

        <button
          className="primary order-cta"
          disabled={!source || targets.length === 0 || eveRunning}
          onClick={() => setShowCopy(true)}
          title={eveRunning ? t("close_eve_first") : ""}
        >
          {t("copy_settings")}
        </button>
        <button
          className="order-alt"
          disabled={!source}
          onClick={() => setShowTemplate(true)}
        >
          {t("save_as_template")}
        </button>
        {source && targets.length === 1 && targets[0].id !== source.id && (
          <button className="order-alt" onClick={() => setShowCompare(true)}>
            {t("compare")}
          </button>
        )}

        {templates.length > 0 && (
          <div className="order-extra">
            <div className="order-label">{t("or_apply_template")}</div>
            <div className="order-row">
              <select
                value={templatePick}
                onChange={(e) =>
                  setTemplatePick(
                    e.target.value === "" ? "" : Number(e.target.value)
                  )
                }
              >
                <option value="">{t("pick_template")}</option>
                {templates.map((tpl) => (
                  <option key={tpl.id} value={tpl.id}>
                    {tpl.name}
                  </option>
                ))}
              </select>
              <button
                disabled={
                  templateToApply === null ||
                  pickedEntries.length === 0 ||
                  eveRunning
                }
                onClick={() => setShowApplyTpl(true)}
              >
                {t("apply")}
              </button>
            </div>
          </div>
        )}

        {groups.length > 0 && (
          <div className="order-extra">
            <div className="order-label">{t("or_copy_group")}</div>
            <div className="order-chips">
              {groups.map((g) => (
                <button
                  key={g.id}
                  disabled={
                    !source || eveRunning || g.member_ids.length === 0
                  }
                  onClick={() => setCopyGroup(g)}
                  title={
                    g.member_ids.length === 1
                      ? t("one_member")
                      : t("n_members", { n: g.member_ids.length })
                  }
                >
                  {g.name} ({g.member_ids.length})
                </button>
              ))}
            </div>
          </div>
        )}
      </aside>

      {showCopy && sourceId !== null && (
        <CopyDialog
          source={list.find((c) => c.id === sourceId)!}
          targets={targets}
          onClose={() => setShowCopy(false)}
          onConfirm={async (selection, crossServer) => {
            try {
              const r = await api.copyCharacter(
                sourceId,
                targets.map((c) => c.id),
                selection,
                crossServer
              );
              onAfterMutation(
                t("msg_copied", { n: r.written.length }) +
                  skippedSuffix(r.skipped.length)
              );
              if (r.skipped.length) {
                setReport({ title: t("report_copy_title"), ...r });
              }
              setShowCopy(false);
              clearSel();
            } catch (e) {
              onError(String(e));
              setShowCopy(false);
            }
          }}
        />
      )}

      {copyGroup && sourceId !== null && (
        <CopyDialog
          source={list.find((c) => c.id === sourceId)!}
          targets={groupMembers(copyGroup)}
          onClose={() => setCopyGroup(null)}
          onConfirm={async (selection, crossServer) => {
            const g = copyGroup;
            try {
              const r = await api.copyToGroup(
                sourceId,
                g.id,
                selection,
                crossServer
              );
              onAfterMutation(
                t("msg_copied_group", { name: g.name, n: r.written.length }) +
                  skippedSuffix(r.skipped.length)
              );
              if (r.skipped.length) {
                setReport({
                  title: t("report_group_title", { name: g.name }),
                  ...r,
                });
              }
            } catch (e) {
              onError(String(e));
            }
            setCopyGroup(null);
          }}
        />
      )}

      {showApplyTpl && templateToApply && (
        <ApplyTemplateDialog
          template={templateToApply}
          characters={list}
          fixedTargets={pickedEntries}
          onClose={() => setShowApplyTpl(false)}
          onConfirm={async (ids, selection, crossServer) => {
            try {
              const r = await api.applyTemplate(
                templateToApply.id,
                ids,
                selection,
                crossServer
              );
              onAfterMutation(
                t("msg_applied_template", {
                  name: templateToApply.name,
                  n: r.written.length,
                }) + skippedSuffix(r.skipped.length)
              );
              if (r.skipped.length) {
                setReport({ title: t("report_apply_title"), ...r });
              }
              clearSel();
            } catch (e) {
              onError(String(e));
            }
            setShowApplyTpl(false);
          }}
        />
      )}

      {showTemplate && sourceId !== null && (
        <SaveTemplateDialog
          source={list.find((c) => c.id === sourceId)!}
          onClose={() => setShowTemplate(false)}
          onConfirm={async (name) => {
            try {
              const tpl = await api.saveTemplate(sourceId, name);
              onAfterMutation(
                tpl.has_user_data
                  ? t("msg_saved_template", { name })
                  : t("msg_saved_template_partial", { name })
              );
              setShowTemplate(false);
            } catch (e) {
              onError(String(e));
            }
          }}
        />
      )}

      {showBlueprint && source && sourceLayout && (
        <BlueprintDialog
          layout={sourceLayout}
          title={source.name ?? t("character_n", { id: source.id })}
          onClose={() => setShowBlueprint(false)}
          design={{
            characterId: source.id,
            eveRunning,
            onDone: (msg) => {
              // The write changed the file; drop the cached sketch so
              // the panel redraws the new truth.
              layoutCache.current.delete(source.id);
              setLayoutNonce((n) => n + 1);
              setShowBlueprint(false);
              onAfterMutation(msg);
            },
            onError,
          }}
        />
      )}

      {showCompare &&
        source &&
        targets.length === 1 &&
        targets[0].id !== source.id && (
          <CompareDialog
            a={source}
            b={targets[0]}
            onClose={() => setShowCompare(false)}
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
    </div>
  );
}

interface CardProps {
  c: CharacterEntry;
  t: T;
  portrait?: string;
  isSource: boolean;
  isSelected: boolean;
  onClick: (e: React.MouseEvent) => void;
  onToggle: () => void;
}

function CharCard({
  c,
  t,
  portrait,
  isSource,
  isSelected,
  onClick,
  onToggle,
}: CardProps) {
  const name = c.name ?? t("character_n", { id: c.id });
  const modified = c.modified
    ? new Date(c.modified * 1000).toLocaleDateString()
    : "-";
  return (
    <div
      className={
        "ccard" + (isSelected ? " selected" : "") + (isSource ? " source" : "")
      }
      onClick={onClick}
    >
      {portrait ? (
        <img className="ccard-portrait" src={portrait} alt="" />
      ) : (
        <div className="ccard-monogram" aria-hidden>
          {name.charAt(0).toUpperCase()}
        </div>
      )}
      <div className="ccard-body">
        <div className="ccard-name">{name}</div>
        <div className="ccard-fine">#{c.id}</div>
        <div className="ccard-fine">
          {modified} · {(c.size / 1024).toFixed(1)} KB
          {c.settings_dirs.length > 1
            ? ` · ${t("n_profiles", { n: c.settings_dirs.length })}`
            : ""}
          {c.servers
            .filter((s) => s !== "tranquility")
            .map((s) => (
              <span key={s} className="ccard-server">
                {" "}
                · {s}
              </span>
            ))}
          {c.account_alias && (
            <span className="ccard-account"> · {c.account_alias}</span>
          )}
        </div>
      </div>
      <input
        type="checkbox"
        className="checkbox ccard-check"
        checked={isSelected}
        onClick={(e) => e.stopPropagation()}
        onChange={onToggle}
      />
      {isSource && <span className="stamp">{t("source_label")}</span>}
    </div>
  );
}

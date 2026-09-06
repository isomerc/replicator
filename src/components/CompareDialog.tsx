import { useEffect, useState } from "react";
import { api } from "../api";
import type { CharacterEntry, CompareReport, WindowLayout } from "../types";
import { BlueprintDialog } from "./BlueprintDialog";
import { DiffLines } from "./DiffLines";
import { LayoutWireframe } from "./LayoutWireframe";
import { useI18n } from "../i18n";

interface Props {
  a: CharacterEntry;
  b: CharacterEntry;
  onClose: () => void;
}

/**
 * Group-by-group answer to "what's actually different between these
 * two characters?" - the question every multiboxer has and no file
 * copier can answer.
 */
export function CompareDialog({ a, b, onClose }: Props) {
  const { t } = useI18n();
  const [report, setReport] = useState<CompareReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [layouts, setLayouts] = useState<
    [WindowLayout | null, WindowLayout | null] | null
  >(null);
  const [blueprint, setBlueprint] = useState<{
    layout: WindowLayout;
    title: string;
  } | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .diffCharacters(a.id, b.id)
      .then((r) => alive && setReport(r))
      .catch((e) => alive && setError(String(e)));
    Promise.all([
      api.characterLayout(a.id).catch(() => null),
      api.characterLayout(b.id).catch(() => null),
    ]).then(([la, lb]) => alive && setLayouts([la, lb]));
    return () => {
      alive = false;
    };
  }, [a.id, b.id]);

  const nameOf = (c: CharacterEntry) =>
    c.name ?? t("character_n", { id: c.id });
  const noteText = (code: string) =>
    code === "same_account"
      ? t("note_same_account")
      : code === "undecodable"
      ? t("note_undecodable")
      : t("note_user_unavailable");

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{t("compare_title", { a: nameOf(a), b: nameOf(b) })}</h2>

        {error && <p className="help">{error}</p>}
        {!error && !report && <p className="help">{t("loading")}</p>}
        {layouts && (layouts[0] || layouts[1]) && (
          <>
            <div className="picker-head">{t("layout_label")}</div>
            <div className="compare-wireframes">
              {([0, 1] as const).map((i) => (
                <div key={i} className="compare-wf">
                  {layouts[i] ? (
                    <div
                      className="wf-open"
                      role="button"
                      title={t("layout_label")}
                      onClick={() =>
                        setBlueprint({
                          layout: layouts[i]!,
                          title: nameOf(i === 0 ? a : b),
                        })
                      }
                    >
                      <LayoutWireframe layout={layouts[i]!} />
                    </div>
                  ) : (
                    <div className="fine">-</div>
                  )}
                  <div className="compare-wf-name">
                    {nameOf(i === 0 ? a : b)}
                  </div>
                </div>
              ))}
            </div>
          </>
        )}

        {report && (
          <>
            <div className="picker-head">{t("char_settings")}</div>
            {report.char_note === "undecodable" ? (
              <p className="help">
                {t("note_undecodable")}{" "}
                {report.char_bytes_differ
                  ? t("diff_bytes_differ")
                  : t("diff_bytes_same")}
              </p>
            ) : report.char_groups.length === 0 ? (
              <p className="help">{t("diff_identical")}</p>
            ) : (
              <DiffLines groups={report.char_groups} />
            )}

            <div className="picker-head" style={{ marginTop: 16 }}>
              {t("account_settings")}
            </div>
            {report.user_note ? (
              <p className="help">{noteText(report.user_note)}</p>
            ) : report.user_groups.length === 0 ? (
              <p className="help">{t("diff_identical")}</p>
            ) : (
              <DiffLines groups={report.user_groups} />
            )}
          </>
        )}

        <div className="actions">
          <button className="primary" onClick={onClose}>
            {t("close")}
          </button>
        </div>
      </div>

      {blueprint && (
        <BlueprintDialog
          layout={blueprint.layout}
          title={blueprint.title}
          onClose={() => setBlueprint(null)}
        />
      )}
    </div>
  );
}

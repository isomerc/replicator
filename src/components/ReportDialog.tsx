import { useI18n } from "../i18n";

interface Props {
  title: string;
  /** Paths written. Empty for reports that only carry a count. */
  written: string[];
  /** Overrides the written count when the report has no paths (restore). */
  writtenCount?: number;
  skipped: string[];
  onClose: () => void;
}

/**
 * Full outcome of a copy / apply / restore. The banners only carry
 * counts; this is where the skip reasons - the part that tells the
 * user how to fix their problem - actually get read.
 */
export function ReportDialog({
  title,
  written,
  writtenCount,
  skipped,
  onClose,
}: Props) {
  const { t } = useI18n();
  const count = writtenCount ?? written.length;
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        <p>
          {t("n_written", { n: count })}
          {skipped.length > 0 && t("n_skipped_below", { n: skipped.length })}.
        </p>
        {skipped.length > 0 && (
          <div className="list" style={{ maxHeight: "35vh", overflow: "auto" }}>
            {skipped.map((s, i) => (
              <div key={i} className="list-item">
                <span className="grow">{s}</span>
              </div>
            ))}
          </div>
        )}
        {written.length > 0 && (
          <details style={{ marginTop: 10 }}>
            <summary className="help" style={{ cursor: "pointer" }}>
              {t("written_files_n", { n: written.length })}
            </summary>
            <div
              className="list"
              style={{ maxHeight: "30vh", overflow: "auto", marginTop: 6 }}
            >
              {written.map((w) => (
                <div key={w} className="list-item">
                  <code className="grow" style={{ overflowWrap: "anywhere" }}>
                    {w}
                  </code>
                </div>
              ))}
            </div>
          </details>
        )}
        <div className="actions">
          <button className="primary" onClick={onClose}>
            {t("close")}
          </button>
        </div>
      </div>
    </div>
  );
}

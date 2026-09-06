import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { LayoutChange, WindowLayout } from "../types";
import { categorize, WF_CATEGORIES } from "../wfCategories";
import { useI18n } from "../i18n";

interface DesignTarget {
  characterId: number;
  eveRunning: boolean;
  /** Called with the success banner text; the dialog closes itself. */
  onDone: (msg: string) => void;
  onError: (e: string) => void;
}

interface Props {
  layout: WindowLayout;
  title: string;
  onClose: () => void;
  /** When present, the blueprint can be edited and written back. */
  design?: DesignTarget;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Drag {
  name: string;
  mode: "move" | "resize";
  startX: number;
  startY: number;
  orig: Rect;
  scale: number;
}

const MIN = 0.02;

/**
 * The blueprint, unfolded: a large color-coded rendering of a window
 * layout. View mode is a lens - legend toggles families, clicking a
 * window hides it. Design mode turns the lens into a drafting table:
 * drag to move, corner handle to resize, then freeze the result as a
 * template or write it back to the character (gated and snapshotted
 * like every mutation).
 */
export function BlueprintDialog({ layout, title, onClose, design }: Props) {
  const { t } = useI18n();
  const [hiddenCats, setHiddenCats] = useState<Set<string>>(new Set());
  const [hiddenWindows, setHiddenWindows] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState(false);
  const [edits, setEdits] = useState<Record<string, Rect>>({});
  const [drag, setDrag] = useState<Drag | null>(null);
  const [tplName, setTplName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [floors, setFloors] = useState<Record<
    string,
    [number, number]
  > | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);

  const W = 960;
  const H = Math.max(
    300,
    Math.min(720, Math.round((W * layout.screen_h) / layout.screen_w))
  );
  const clamp = (v: number, lo: number, hi: number) =>
    Math.max(lo, Math.min(hi, v));

  const catCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const w of layout.windows) {
      const c = categorize(w.name);
      counts.set(c.key, (counts.get(c.key) ?? 0) + 1);
    }
    return counts;
  }, [layout]);

  const effective = (name: string, base: Rect): Rect => edits[name] ?? base;
  const dirty = Object.keys(edits).length > 0;

  /// The smallest size EVE has ever saved for this box - a stack can't
  /// shrink below its most demanding member. Normalized against the
  /// window's OWN saved viewport: files carry mixed resolutions, and
  /// the write path denormalizes per window too.
  function floorFor(name: string): [number, number] {
    if (!floors) return [MIN, MIN];
    const r = layout.windows.find((w) => w.name === name);
    const members = r ? [r.name, ...r.stacked] : [name];
    let fw = 0;
    let fh = 0;
    for (const m of members) {
      const f = floors[m];
      if (f) {
        fw = Math.max(fw, f[0]);
        fh = Math.max(fh, f[1]);
      }
    }
    const vw = r?.vw ?? layout.screen_w;
    const vh = r?.vh ?? layout.screen_h;
    return [Math.max(MIN, fw / vw), Math.max(MIN, fh / vh)];
  }

  // Floors load as soon as a designable blueprint opens, and the
  // Design button stays disarmed until they arrive: editing without
  // limits would let the canvas promise sizes the game will refuse.
  // A failed fetch is a real error, not a shrug.
  useEffect(() => {
    if (!design) return;
    let alive = true;
    api
      .layoutFloors()
      .then((f) => alive && setFloors(f))
      .catch((e) => alive && design.onError(String(e)));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function toggleCat(key: string) {
    setHiddenCats((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  function beginDrag(
    e: React.PointerEvent,
    name: string,
    mode: "move" | "resize",
    orig: Rect
  ) {
    if (!editing) return;
    e.stopPropagation();
    // Claim the gesture: no native selection, and every subsequent
    // pointer event routes to the svg even outside its bounds.
    e.preventDefault();
    const svg = svgRef.current;
    const bcr = svg?.getBoundingClientRect();
    if (!svg || !bcr || bcr.width === 0) return;
    try {
      svg.setPointerCapture(e.pointerId);
    } catch {
      // Capture is best-effort; the drag works without it.
    }
    setDrag({
      name,
      mode,
      startX: e.clientX,
      startY: e.clientY,
      orig,
      scale: W / bcr.width,
    });
  }

  function onMove(e: React.PointerEvent) {
    if (!drag) return;
    // scale converts client px to viewBox units (aspect is preserved,
    // so one factor serves both axes); dividing by W/H normalizes.
    // Read the bounding box fresh: pointer capture keeps a drag alive
    // through a window resize, which would stale the start-time scale.
    const bcr = svgRef.current?.getBoundingClientRect();
    const scale = bcr && bcr.width > 0 ? W / bcr.width : drag.scale;
    const dx = ((e.clientX - drag.startX) * scale) / W;
    const dy = ((e.clientY - drag.startY) * scale) / H;
    const o = drag.orig;
    const [minW, minH] = floorFor(drag.name);
    const next: Rect =
      drag.mode === "move"
        ? {
            x: clamp(o.x + dx, 0, 1 - o.w),
            y: clamp(o.y + dy, 0, 1 - o.h),
            w: o.w,
            h: o.h,
          }
        : {
            x: o.x,
            y: o.y,
            w: clamp(o.w + dx, Math.min(minW, o.w), 1 - o.x),
            h: clamp(o.h + dy, Math.min(minH, o.h), 1 - o.y),
          };
    setEdits((prev) => ({ ...prev, [drag.name]: next }));
  }

  function changes(): LayoutChange[] {
    const out: LayoutChange[] = [];
    for (const r of layout.windows) {
      const e = edits[r.name];
      if (!e) continue;
      for (const member of [r.name, ...r.stacked]) {
        out.push({ name: member, ...e });
      }
    }
    return out;
  }

  async function saveTemplate(name: string) {
    if (!design || busy) return;
    setBusy(true);
    try {
      const tpl = await api.designSaveTemplate(
        design.characterId,
        changes(),
        name
      );
      design.onDone(t("msg_saved_template", { name: tpl.name }));
      onClose();
    } catch (e) {
      design.onError(String(e));
      setBusy(false);
    }
  }

  async function writeBack() {
    if (!design) return;
    setBusy(true);
    try {
      await api.designApply(design.characterId, changes());
      design.onDone(t("msg_layout_written", { name: title }));
      onClose();
    } catch (e) {
      design.onError(String(e));
      setBusy(false);
    }
  }

  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        // This dialog can sit inside another modal's backdrop (the
        // compare view); the click must not bubble out and close both.
        e.stopPropagation();
        onClose();
      }}
    >
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        <p className="help">
          {editing ? t("design_hint") : t("wf_click_hint")}
        </p>

        <div className="wf-legend">
          {WF_CATEGORIES.filter((c) => catCounts.has(c.key)).map((c) => (
            <label key={c.key} className="wf-legend-item">
              <input
                type="checkbox"
                className="checkbox"
                checked={!hiddenCats.has(c.key)}
                onChange={() => toggleCat(c.key)}
              />
              <span
                className="wf-swatch"
                style={{ background: c.color }}
                aria-hidden
              />
              <span>{t(c.labelKey)}</span>
              <span className="wf-legend-count">{catCounts.get(c.key)}</span>
            </label>
          ))}
          {hiddenWindows.size > 0 && (
            <button
              className="ghost"
              onClick={() => setHiddenWindows(new Set())}
            >
              {t("wf_show_all")}
            </button>
          )}
        </div>

        <svg
          ref={svgRef}
          className={"wireframe blueprint" + (editing ? " editing" : "")}
          viewBox={`0 0 ${W} ${H}`}
          preserveAspectRatio="xMidYMid meet"
          // The CSS aspect-ratio makes height caps shrink the width in
          // step, so the element box always equals the drawing - no
          // letterboxing, which would silently break the drag math's
          // width-based scale.
          style={{ aspectRatio: `${W} / ${H}` }}
          role="img"
          onPointerMove={onMove}
          onPointerUp={() => setDrag(null)}
          onLostPointerCapture={() => setDrag(null)}
        >
          <rect
            className="wf-screen"
            x="1"
            y="1"
            width={W - 2}
            height={H - 2}
          />
          {layout.windows.map((r, i) => {
            const cat = categorize(r.name);
            if (hiddenCats.has(cat.key) || hiddenWindows.has(r.name)) {
              return null;
            }
            const rect = effective(r.name, r);
            const x0 = clamp(rect.x, 0, 1);
            const y0 = clamp(rect.y, 0, 1);
            const x1 = clamp(rect.x + rect.w, 0, 1);
            const y1 = clamp(rect.y + rect.h, 0, 1);
            if (x1 - x0 < 0.004 || y1 - y0 < 0.004) return null;
            const px = x0 * W;
            const py = y0 * H;
            const pw = (x1 - x0) * W;
            const ph = (y1 - y0) * H;
            const label = pw > 88 && ph > 26;
            const edited = !!edits[r.name];
            return (
              <g
                key={r.name + i}
                className={"wf-g" + (editing ? " draggable" : "")}
                onPointerDown={(e) => beginDrag(e, r.name, "move", rect)}
                onClick={() => {
                  if (!editing) {
                    setHiddenWindows((prev) => new Set(prev).add(r.name));
                  }
                }}
              >
                <rect
                  className={"wf-win colored" + (edited ? " edited" : "")}
                  x={px.toFixed(1)}
                  y={py.toFixed(1)}
                  width={pw.toFixed(1)}
                  height={ph.toFixed(1)}
                  style={{ stroke: cat.color, fill: cat.color + "38" }}
                >
                  <title>
                    {[r.name, ...r.stacked].join(", ")}
                  </title>
                </rect>
                {label && (
                  <text
                    className="wf-label"
                    x={(px + 7).toFixed(1)}
                    y={(py + 17).toFixed(1)}
                  >
                    {r.name.length > Math.floor(pw / 7)
                      ? r.name.slice(0, Math.floor(pw / 7)) + "…"
                      : r.name}
                  </text>
                )}
                {editing && (
                  <rect
                    className="wf-handle"
                    x={(px + pw - 9).toFixed(1)}
                    y={(py + ph - 9).toFixed(1)}
                    width="8"
                    height="8"
                    style={{ fill: cat.color }}
                    onPointerDown={(e) =>
                      beginDrag(e, r.name, "resize", rect)
                    }
                  />
                )}
              </g>
            );
          })}
        </svg>

        {tplName !== null ? (
          <div className="design-bar">
            <input
              type="text"
              value={tplName}
              autoFocus
              placeholder={t("template_name_placeholder")}
              onChange={(e) => setTplName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && tplName.trim() && !busy) {
                  saveTemplate(tplName.trim());
                }
              }}
            />
            <button
              className="primary"
              disabled={busy || !tplName.trim()}
              onClick={() => saveTemplate(tplName.trim())}
            >
              {t("save")}
            </button>
            <button className="ghost" onClick={() => setTplName(null)}>
              {t("cancel")}
            </button>
          </div>
        ) : (
          <div className="actions">
            {design && !editing && (
              <button disabled={!floors} onClick={() => setEditing(true)}>
                {t("design_edit")}
              </button>
            )}
            {editing && (
              <>
                <button
                  className="ghost"
                  disabled={!dirty || busy}
                  onClick={() => setEdits({})}
                >
                  {t("design_reset")}
                </button>
                <button
                  disabled={!dirty || busy}
                  onClick={() => setTplName("")}
                >
                  {t("design_save_tpl")}
                </button>
                <button
                  className="danger"
                  disabled={!dirty || busy || design!.eveRunning}
                  title={design!.eveRunning ? t("close_eve_first") : ""}
                  onClick={writeBack}
                >
                  {t("design_write_to", { name: title })}
                </button>
              </>
            )}
            <button className="primary" onClick={onClose}>
              {t("close")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

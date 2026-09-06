import type { WindowLayout } from "../types";

/**
 * A window layout drawn as a blueprint: the client viewport as a ruled
 * frame, every open window as an ink rectangle where it actually sits.
 * Rendered from decoded settings - this is the file's truth, not a
 * screenshot.
 */
export function LayoutWireframe({ layout }: { layout: WindowLayout }) {
  const W = 320;
  const H = Math.max(
    100,
    Math.min(280, Math.round((W * layout.screen_h) / layout.screen_w))
  );
  const clamp = (v: number) => Math.max(0, Math.min(1, v));
  return (
    <svg
      className="wireframe"
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="xMidYMid meet"
      role="img"
    >
      <rect
        className="wf-screen"
        x="0.75"
        y="0.75"
        width={W - 1.5}
        height={H - 1.5}
      />
      {layout.windows.map((r, i) => {
        const x0 = clamp(r.x);
        const y0 = clamp(r.y);
        const x1 = clamp(r.x + r.w);
        const y1 = clamp(r.y + r.h);
        if (x1 - x0 < 0.004 || y1 - y0 < 0.004) return null;
        return (
          <rect
            key={r.name + i}
            className="wf-win"
            x={(x0 * W).toFixed(1)}
            y={(y0 * H).toFixed(1)}
            width={((x1 - x0) * W).toFixed(1)}
            height={((y1 - y0) * H).toFixed(1)}
          >
            <title>{r.name}</title>
          </rect>
        );
      })}
    </svg>
  );
}

import type { ReactNode } from "react";

interface Props {
  kind: "good" | "warn" | "bad";
  children: ReactNode;
  onDismiss?: () => void;
}

export function Banner({ kind, children, onDismiss }: Props) {
  return (
    <div className={`banner ${kind}`}>
      <div style={{ flex: 1 }}>{children}</div>
      {onDismiss && (
        <button className="ghost" onClick={onDismiss}>
          ×
        </button>
      )}
    </div>
  );
}

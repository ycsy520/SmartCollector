import type { FragmentStatus } from "../types/ipc";
import { STATUS_LABEL } from "../types/enums";

// M3 tonal 状态徽章：容器色 + 前导点，随明暗翻转。
const WRAP: Record<FragmentStatus, string> = {
  pending: "bg-surface-high text-on-surface-variant",
  running: "bg-info-container text-on-info-container",
  done: "bg-success-container text-on-success-container",
  failed: "bg-error-container text-on-error-container",
  skipped: "bg-surface-container text-on-surface-variant",
};

const DOT: Record<FragmentStatus, string> = {
  pending: "bg-outline",
  running: "bg-info animate-pulse",
  done: "bg-success",
  failed: "bg-error",
  skipped: "bg-outline-variant",
};

export function StatusBadge({
  status,
  bare,
}: {
  status: FragmentStatus;
  /** 去 tonal 底，只留点+文字：详情页头部用它——那里紧邻按钮，pill 底色会谎报可点。 */
  bare?: boolean;
}) {
  return (
    <span
      className={`inline-flex shrink-0 items-center gap-1.5 text-xs font-semibold transition-colors duration-instant ${
        bare ? "text-on-surface-variant" : `rounded-full px-2.5 py-0.5 ${WRAP[status]}`
      }`}
    >
      <span className={`h-1.5 w-1.5 rounded-full ${DOT[status]}`} />
      {STATUS_LABEL[status]}
    </span>
  );
}

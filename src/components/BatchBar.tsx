// 批量裁决条：多选模式下浮在页面底部，承载「已选 N + 各动作 + 清除选择」。
// 纯展示——动作语义/撤销由页面决定，本件只负责排布与点击透传。
import { Button } from "./ui/button";

export type BatchAction = {
  key: string;
  label: string;
  onClick: () => void;
  danger?: boolean;
};

export function BatchBar({
  count,
  actions,
  onClear,
}: {
  count: number;
  actions: BatchAction[];
  onClear: () => void;
}) {
  return (
    <div className="md-elev sticky bottom-4 z-20 mx-auto flex w-fit items-center gap-3 rounded-full border border-outline-variant bg-surface-high px-4 py-2 text-sm shadow-e2">
      <span className="font-semibold text-on-surface">已选 {count}</span>
      <span className="h-4 w-px bg-outline-variant" />
      <div className="flex items-center gap-1.5">
        {actions.map((a) => (
          <Button
            key={a.key}
            variant={a.danger ? "danger" : "primary"}
            onClick={a.onClick}
            className="px-3 py-1 text-xs"
          >
            {a.label}
          </Button>
        ))}
        <Button variant="ghost" onClick={onClear} className="px-3 py-1 text-xs">
          清除选择
        </Button>
      </div>
    </div>
  );
}

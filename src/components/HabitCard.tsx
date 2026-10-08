// Habit 分拣习惯候选卡：AI 从人工分拣动作中归纳的规则，只建议、须用户启用。
// 类型权威定义见 types/ipc.ts / 02 §7.3；数据源在 stores/habits（双模：Tauri 真 IPC / 浏览器 mock）。
import { useEffect, useState } from "react";
import type { HabitRule } from "../types/ipc";
import { useHabits } from "../stores/habits";
import { Button } from "./ui/button";
import { toast } from "./ui/toast";

const Dot = ({ color }: { color: string }) => (
  <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${color}`} />
);

function HabitCardView({
  rule,
  onEnable,
  onPause,
  onDismiss,
}: {
  rule: HabitRule;
  onEnable: (id: string) => void;
  onPause: (id: string) => void;
  onDismiss: (id: string) => void;
}) {
  const [openSamples, setOpenSamples] = useState(false);
  const ratio = `${rule.hits}/${rule.total}`;

  if (rule.state === "active")
    return (
      <div className="md-elev rounded-2xl bg-card p-4">
        <div className="flex items-center gap-2 text-xs text-hint">
          <Dot color="bg-success" /> 自动处理中
        </div>
        <p className="mt-1 text-sm text-on-surface">
          「{rule.pattern}」→ <b>{rule.action}</b>
        </p>
        <div className="mt-2 flex items-center justify-between">
          <span className="text-xs text-hint">
            近 7 天替你处理了 {rule.recentAuto ?? 0} 条 · 周摘要里可整批撤销
          </span>
          <div className="flex gap-2">
            <Button
              variant="ghost"
              className="px-2.5 py-1 text-xs"
              onClick={() => toast("最近 3 条：均为「生活」类购物链接", "info")}
            >
              查看痕迹
            </Button>
            <Button
              variant="ghost"
              className="px-2.5 py-1 text-xs"
              onClick={() => onPause(rule.id)}
            >
              暂停
            </Button>
          </div>
        </div>
      </div>
    );

  if (rule.state === "paused")
    return (
      <div className="rounded-xl border border-dashed border-outline-variant p-4">
        <div className="flex items-center justify-between">
          <span className="flex items-center gap-2 text-sm text-on-surface-variant">
            <Dot color="bg-outline" /> 「{rule.pattern}」→ {rule.action} ·
            已暂停
          </span>
          <Button
            variant="ghost"
            className="px-2.5 py-1 text-xs"
            onClick={() => onEnable(rule.id)}
          >
            重新启用
          </Button>
        </div>
      </div>
    );

  return (
    <div className="rounded-xl border border-warn/30 bg-warn-container/40 p-4">
      <div className="flex items-center gap-2 text-xs text-on-warn-container">
        <Dot color="bg-warn" /> AI 注意到了
      </div>
      <p className="mt-1.5 text-sm leading-relaxed text-on-surface">
        你最近 {ratio} 次把「{rule.pattern}」的信息处理成了
        <b>{rule.action}</b>。以后遇到同类，要我照做吗？
      </p>
      <p className="mt-1 text-xs leading-relaxed text-on-surface-variant">
        启用后同类碎片将直接{rule.action.startsWith("丢弃") ? "进垃圾站（30 天可找回）" : "完成归档（标记「替你收的」，可补审）"}；每次自动动作都在周摘要留痕，可整批撤销。
      </p>
      <div className="mt-3 flex items-center justify-between">
        <button
          className="text-xs text-on-surface-variant underline-offset-2 hover:text-primary hover:underline"
          onClick={() => setOpenSamples((v) => !v)}
        >
          {openSamples ? "收起样本" : `查看 ${rule.hits} 个样本`}
        </button>
        <div className="flex gap-2">
          <Button
            variant="ghost"
            className="px-2.5 py-1 text-xs"
            onClick={() => onDismiss(rule.id)}
          >
            以后再说
          </Button>
          <Button
            className="px-3 py-1 text-xs"
            onClick={() => {
              onEnable(rule.id);
              toast("已启用：同类碎片将自动处理，周摘要可撤销", "info");
            }}
          >
            启用
          </Button>
        </div>
      </div>
      {openSamples && (
        <ul className="mt-3 space-y-1.5 border-t border-warn/20 pt-3">
          {rule.samples.map((s) => (
            <li key={s} className="text-xs leading-relaxed text-on-surface-variant">
              · {s}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function HabitSection() {
  const { rules, load, setState } = useHabits();
  const [dismissed, setDismissed] = useState<string[]>([]);

  useEffect(() => {
    void load();
  }, [load]);

  const visible = rules.filter((r) => !dismissed.includes(r.id));
  const candidates = visible.filter((r) => r.state === "candidate");
  const managed = visible.filter((r) => r.state !== "candidate");

  const enable = (id: string) => void setState(id, "active");
  const pause = (id: string) => void setState(id, "paused");
  const dismiss = (id: string) => setDismissed((d) => [...d, id]);

  return (
    <section className="flex flex-col gap-2.5">
      <div className="flex items-baseline justify-between">
        <h2 className="text-xs font-semibold text-hint">分拣习惯</h2>
        <span className="text-xs text-hint">
          你的每次放行与丢弃，都在这里被记住
        </span>
      </div>
      {candidates.map((r) => (
        <HabitCardView
          key={r.id}
          rule={r}
          onEnable={enable}
          onPause={pause}
          onDismiss={dismiss}
        />
      ))}
      {managed.map((r) => (
        <HabitCardView
          key={r.id}
          rule={r}
          onEnable={enable}
          onPause={pause}
          onDismiss={dismiss}
        />
      ))}
      {candidates.length === 0 && managed.length === 0 && (
        <p className="text-xs text-hint">
          还没有观察到稳定的分拣习惯。多分拣一些信息后，我会再提议。
        </p>
      )}
      {dismissed.length > 0 && (
        <button
          className="self-start text-xs text-hint underline-offset-2 hover:underline"
          onClick={() => setDismissed([])}
        >
          恢复 {dismissed.length} 条「以后再说」的建议
        </button>
      )}
    </section>
  );
}

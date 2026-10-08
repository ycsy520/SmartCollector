import { create } from "zustand";
import { useEffect, useState } from "react";
import { Icon } from "./icon";

// 单一通知面（批次31 第1片）：右下角 toast 升格为顶部「灵动岛」。
// 一次只显示最前一条，其余排队；每条停留按时长基线 + 排队积压递减（队列越长越仓促，保证 backlog 能排空）。
// 带撤销动作的一律给足 6s（要读完再点）。pending 态（"处理中…"）不自动消失，等 resolve/reject 落定才计时。
type Kind = "info" | "error" | "pending";
interface ToastAction {
  label: string;
  onClick: () => void;
}
interface ToastItem {
  id: number;
  text: string;
  kind: Kind;
  action?: ToastAction;
  pending: boolean; // "处理中…"——不自动超时，等 resolve/reject
  leaving: boolean; // 正在缩回顶部：先播完退场 transition 再真删
}
interface ToastState {
  items: ToastItem[];
  push: (t: ToastItem) => void;
  patch: (id: number, p: Partial<Omit<ToastItem, "id">>) => void;
  leave: (id: number) => void;
  pop: (id: number) => void;
}

// 进/退时长：从容但不拖沓（比旧 toast 的 200/160 明显更缓）。退场必须与 index.css
// .island[data-leaving] 的 --dur-island-out 对齐，否则节点在动画跑完前就被卸载。
const EXIT_MS = 260;

const useToastStore = create<ToastState>((set) => ({
  items: [],
  push: (t) => set((s) => ({ items: [...s.items, t] })),
  patch: (id, p) =>
    set((s) => ({ items: s.items.map((x) => (x.id === id ? { ...x, ...p } : x)) })),
  leave: (id) =>
    set((s) => ({ items: s.items.map((x) => (x.id === id ? { ...x, leaving: true } : x)) })),
  pop: (id) => set((s) => ({ items: s.items.filter((x) => x.id !== id) })),
}));

let nextId = 0;

// 统一退场：手动/超时都先标记 leaving 播完 transition，再 pop。直接 pop 会让退场根本没机会跑。
function retire(id: number) {
  const s = useToastStore.getState();
  if (s.items.some((x) => x.id === id && !x.leaving)) s.leave(id);
  setTimeout(() => useToastStore.getState().pop(id), EXIT_MS);
}

/** 落一条即时提示（成功/信息/失败）。排队显示，返回其 id。 */
export function toast(text: string, kind: "info" | "error" = "info", action?: ToastAction) {
  const id = ++nextId;
  useToastStore.getState().push({ id, text, kind, action, pending: false, leaving: false });
  return id;
}

export interface NoticeHandle {
  id: number;
  /** 落定结果：翻成非 pending 并开始计时消失。 */
  settle: (text: string, kind?: "info" | "error") => void;
  /** 进度文案替换（仍 pending，不计时）。 */
  update: (text: string) => void;
}

/**
 * 起一条"处理中…"pending 提示，返回句柄；动作拿到后端 callback 后调 `settle` 落定文案。
 * 这是第2片接线用的 API——第1片先把通知面与队列/pending 机制建起来。
 */
export function startToast(text = "处理中…", action?: ToastAction): NoticeHandle {
  const id = ++nextId;
  useToastStore.getState().push({ id, text, kind: "pending", action, pending: true, leaving: false });
  return {
    id,
    settle: (t, kind = "info") =>
      useToastStore.getState().patch(id, { text: t, kind, pending: false }),
    update: (t) => useToastStore.getState().patch(id, { text: t }),
  };
}

function IslandRow({ t }: { t: ToastItem }) {
  // 挂载帧先停在"顶栏之上 + 透明 + 略缩"，下一帧落到原位——transition 才有插值起点。
  // setTimeout 兜底不可省：窗口隐藏/后台时 rAF 被暂停，只靠它岛会永久停在 opacity:0（旧 toast LIVE 实测复现）。
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setMounted(true));
    const timer = setTimeout(() => setMounted(true), 120);
    return () => {
      cancelAnimationFrame(raf);
      clearTimeout(timer);
    };
  }, []);
  const icon = t.kind === "error" ? "TriangleAlert" : t.kind === "pending" ? "Loader2" : "Check";
  const tone =
    t.kind === "error"
      ? "bg-error text-on-error"
      : "bg-inverse-surface text-inverse-on-surface";
  return (
    <div
      data-mounted={mounted}
      data-leaving={t.leaving === true}
      className={`island md-elev flex items-center gap-2.5 rounded-full px-4 py-2.5 text-sm font-medium ${tone}`}
    >
      <Icon name={icon} size={16} className={t.kind === "pending" ? "animate-spin" : ""} />
      {/* 文案在容器落定后稍迟淡入——避免整块"啪"地出现，也贴合灵动岛"先形变再揭示"的观感。 */}
      <span className="island-body min-w-0 break-words text-left">{t.text}</span>
      {t.action && (
        <button
          // 容器 pointer-events-none（不挡下方交互），动作按钮单独可点。
          className="island-body pointer-events-auto ml-1 shrink-0 rounded-full px-2 py-0.5 font-semibold underline decoration-current/50 underline-offset-2 hover:bg-inverse-on-surface/10"
          onClick={() => {
            t.action?.onClick();
            retire(t.id);
          }}
        >
          {t.action.label}
        </button>
      )}
    </div>
  );
}

export function Toaster() {
  const items = useToastStore((s) => s.items);
  const front = items[0];
  // 只调度"当前最前且非 pending/非退场"这条的超时；front 一变（换条/落定/退场）就重置计时。
  const frontId = front?.id;
  const pending = front?.pending;
  const leaving = front?.leaving;
  const hasAction = !!front?.action;
  const backlog = items.length - 1;
  useEffect(() => {
    if (frontId == null || pending || leaving) return;
    // 与 dwellMs 同规则：撤销类 6s，否则按积压递减。这里只需 hasAction/backlog 两个入参。
    const ms = hasAction ? 6000 : backlog > 10 ? 1000 : backlog > 5 ? 2000 : 3000;
    const timer = setTimeout(() => retire(frontId), ms);
    return () => clearTimeout(timer);
  }, [frontId, pending, leaving, hasAction, backlog]);

  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-none fixed left-1/2 top-3 z-50 flex w-max max-w-[min(92vw,34rem)] -translate-x-1/2 justify-center"
    >
      {front && <IslandRow t={front} />}
    </div>
  );
}

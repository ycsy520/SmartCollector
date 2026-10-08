import { useEffect, useRef, useState } from "react";

/**
 * #80 键盘流分拣（设计方案 §2.4 死角 B/C）。
 * 键位：`j`/`k`（含 ↑/↓）移动焦点、`Enter` 打开、`Space` 切勾选、多选态下 `Shift`+移动连选区间、
 * `Ctrl/Cmd+A` 全选当前可见、`Delete` 丢弃焦点行（调用方弹二次确认）。
 * 并置态（列表与详情同屏）下焦点移动会带着详情一起走，即"选中即预览"。
 * 焦点行由调用方渲染成 `data-frag="{id}"`，本钩子据此 scrollIntoView，不自管滚动位置。
 */
type Options = {
  /** 当前可见列表的有序 id（筛选后顺序，与渲染一致）。 */
  ids: string[];
  /** 多选态：true 时移动只挪焦点，不让右栏跟随。 */
  mode: boolean;
  /** Enter 与（并置态下的）焦点移动都走它：打开一条。 */
  open: (id: string) => void;
  /** Space：切一条勾选（调用方负责非多选态时先进多选）。 */
  toggle: (id: string) => void;
  /** 常态下焦点移动是否让详情跟随。仅在"列表与详情已并置"时为真——窄档一打开就整页替换、
   *  列表卸载，键盘流当场退化成跳转；调用方用 `activeId != null` 判定（窄档下本列表根本不挂载）。 */
  follow: boolean;
  /** Shift 连选：把 anchor..cursor 这一段并入勾选集（是并入，不是替换）。 */
  selectRange: (ids: string[]) => void;
  /** Ctrl/Cmd+A：全选"当前筛选结果"（范围==看到范围，与页面上的全选按钮同口径）。 */
  selectAll: (ids: string[]) => void;
  /** Delete：请求丢弃焦点行。调用方负责二次确认——键盘误触的代价比鼠标点击高。 */
  requestDelete: (id: string) => void;
  /** 右栏当前条目：鼠标点选后把焦点同步过去，避免键盘与鼠标两条路径状态分叉。 */
  activeId?: string | null;
};

const EDITABLE = new Set(["INPUT", "TEXTAREA", "SELECT"]);

export function useKeyboardReview({
  ids,
  mode,
  open,
  toggle,
  follow,
  selectAll,
  selectRange,
  requestDelete,
  activeId,
}: Options) {
  const [cursor, setCursor] = useState(0);
  // 焦点环在用户**第一次按下键位之前不显示**：否则首屏第一条永远顶着一条朱砂线，
  // 看起来像"系统选中了它"，而实际上没人碰过键盘。
  const [armed, setArmed] = useState(false);
  const cursorRef = useRef(0);
  const anchorRef = useRef(0);
  // 回调与最新列表放进 ref：避免把 ids 写进依赖数组后每次数据变化都要重挂 window 监听器。
  const live = useRef({ ids, mode, open, toggle, follow, selectAll, selectRange, requestDelete });
  live.current = { ids, mode, open, toggle, follow, selectAll, selectRange, requestDelete };

  const setCursorBoth = (i: number) => {
    cursorRef.current = i;
    setCursor(i);
  };
  const arm = () => setArmed(true);

  // 列表变化（筛选/增删）后夹回合法范围；越界不夹会让焦点指到 undefined，键位全部落空。
  const limit = Math.max(0, ids.length - 1);
  if (cursorRef.current > limit) setCursorBoth(limit);

  // 鼠标点了某条 → 焦点同步过去（同一条不重复 setState）。
  const syncKey = `${activeId ?? ""}|${ids.length}`;
  useEffect(() => {
    if (!activeId) return;
    const idx = live.current.ids.indexOf(activeId);
    if (idx >= 0 && idx !== cursorRef.current) setCursorBoth(idx);
  }, [syncKey]);

  useEffect(() => {
    const move = (delta: number, shift: boolean) => {
      const { ids: list, mode: multi, open: onOpen, follow: sync, selectRange: onRange } =
        live.current;
      if (!list.length) return;
      arm();
      const from = cursorRef.current;
      const next = Math.min(list.length - 1, Math.max(0, from + delta));
      if (next === from && !shift) return;
      setCursorBoth(next);
      document
        .querySelector(`[data-frag="${CSS.escape(list[next])}"]`)
        ?.scrollIntoView({ block: "nearest" });
      if (shift && multi) {
        const [a, b] = [Math.min(anchorRef.current, next), Math.max(anchorRef.current, next)];
        onRange(list.slice(a, b + 1));
      } else if (multi) {
        anchorRef.current = next;
      } else if (sync) {
        onOpen(list[next]); // 并置态：右栏就地跟随，列表与滚动位置都不动
      }
    };

    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.altKey) return;
      const t = e.target as HTMLElement | null;
      // 正在写字（草稿/附言/笔记/检索框）或弹层开着时，键盘不能抢走按键。
      if (t && (EDITABLE.has(t.tagName) || t.isContentEditable)) return;
      if (document.querySelector('[role="dialog"]')) return;

      const at = () => live.current.ids[cursorRef.current];
      if (e.ctrlKey || e.metaKey) {
        // 只认 Ctrl/Cmd+A（其余组合键交回给系统与浏览器）。
        if ((e.key === "a" || e.key === "A") && live.current.ids.length) {
          e.preventDefault();
          live.current.selectAll(live.current.ids);
        }
        return;
      }
      if (e.key === "Delete" || e.key === "Backspace") {
        const id = at();
        if (!id) return;
        e.preventDefault();
        arm();
        live.current.requestDelete(id); // 不直接删：调用方弹二次确认
      } else if (e.key === "j" || e.key === "ArrowDown") {
        e.preventDefault();
        move(1, e.shiftKey);
      } else if (e.key === "k" || e.key === "ArrowUp") {
        e.preventDefault();
        move(-1, e.shiftKey);
      } else if (e.key === "Enter") {
        if (t?.closest("button,a,[role='button']")) return; // 焦点在控件上：Enter 归那个控件
        const id = at();
        if (id) {
          e.preventDefault();
          arm();
          live.current.open(id);
        }
      } else if (e.key === " ") {
        // 焦点在按钮/链接上时空格＝按下该控件（与 Enter 分支同理），不劫持。
        if (t?.closest("button,a,[role='button'],[role='checkbox']")) return;
        const id = at();
        if (!id) return;
        e.preventDefault(); // 空格默认滚一页，会把焦点甩出视口
        arm();
        if (e.shiftKey && live.current.mode) {
          const [a, b] = [
            Math.min(anchorRef.current, cursorRef.current),
            Math.max(anchorRef.current, cursorRef.current),
          ];
          live.current.selectRange(live.current.ids.slice(a, b + 1));
        } else {
          anchorRef.current = cursorRef.current;
          live.current.toggle(id);
        }
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return {
    cursorId: armed ? (ids[Math.min(cursor, Math.max(0, ids.length - 1))] ?? null) : null,
  };
}

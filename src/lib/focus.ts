// 弹窗焦点陷阱（桌面 UX 原则：任何弹窗必须支持 Tab/Shift+Tab 循环，焦点不得跑出模态）。
// 做成纯函数而非 hook：详情页的「片段对照」「一次性指令」都是渲染期条件 JSX（IIFE / &&），
// 在里面调 hook 会违反 hooks 规则；用 onKeyDown 挂到容器上最省事，也无需为每个弹层抽组件。
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

// 只列真正可聚焦且启用态的选择器；:disabled 与 tabindex="-1" 排除在外。
const FOCUSABLE =
  'a[href],button:not(:disabled),textarea:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex]:not([tabindex="-1"])';

/**
 * 把 Tab / Shift+Tab 限制在 container 内循环。到达首/尾元素时折返到另一端；
 * 焦点万一落在容器外（如刚挂载）也拉回容器内。不拦截其它按键。
 */
export function trapTab(
  e: ReactKeyboardEvent,
  container: HTMLElement | null
): void {
  if (e.key !== "Tab" || !container) return;
  const nodes = Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE));
  if (nodes.length === 0) return;
  const first = nodes[0];
  const last = nodes[nodes.length - 1];
  const active = document.activeElement;
  const inside = active !== null && container.contains(active);
  if (e.shiftKey && (!inside || active === first)) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (!inside || active === last)) {
    e.preventDefault();
    first.focus();
  }
}

import { useEffect, useState, type RefObject } from "react";
import { Icon } from "./ui/icon";

/**
 * 滚动区「回到顶部」浮标：滚动超过阈值才浮现，点击平滑滚回顶部。
 * 用在"外层不滚、内层 overflow-y-auto"的布局（如详情页）——把本组件绝对定位在
 * 滚动容器的父级（需 relative），ref 指向真正滚动的元素。
 * 批次31：右下角此前被"刻意留空"是因为旧 toast 落在此处；通知已升格为顶部灵动岛，此位已空出。
 */
export function BackToTop({ scrollRef }: { scrollRef: RefObject<HTMLElement | null> }) {
  const [show, setShow] = useState(false);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => setShow(el.scrollTop > el.clientHeight * 0.75);
    onScroll(); // 挂载即判一次：切片段/回退时可能已停在滚动位置
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [scrollRef]);

  return (
    <button
      type="button"
      aria-label="回到顶部"
      title="回到顶部"
      tabIndex={show ? 0 : -1}
      onClick={() => {
        const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
        scrollRef.current?.scrollTo({ top: 0, behavior: reduce ? "auto" : "smooth" });
      }}
      className={`md-press absolute bottom-6 right-6 z-30 inline-flex items-center justify-center rounded-full border border-outline-variant bg-card p-2.5 text-on-surface-variant shadow-e2 transition-[opacity,transform] duration-200 ease-emph hover:text-primary ${
        show ? "translate-y-0 opacity-100" : "pointer-events-none translate-y-2 opacity-0"
      }`}
    >
      <Icon name="ArrowUp" size={18} />
    </button>
  );
}

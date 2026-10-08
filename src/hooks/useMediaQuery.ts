import { useEffect, useState } from "react";

/**
 * 断点判定用 matchMedia 而非纯 CSS：宽档要把详情"就地并置"、窄档仍是整页替换，
 * 两者对同一个 FragmentDetail 只能挂载一次（它会自相关碎片 IPC），CSS 隐藏会重复挂载。
 */
export function useMediaQuery(query: string): boolean {
  const [matched, setMatched] = useState(
    () => window.matchMedia(query).matches,
  );
  useEffect(() => {
    const mql = window.matchMedia(query);
    const onChange = () => setMatched(mql.matches);
    mql.addEventListener("change", onChange);
    onChange(); // query 变化时（如热更新/条件切换）视口并未变，change 不会触发——补一次同步
    return () => mql.removeEventListener("change", onChange);
  }, [query]);
  return matched;
}

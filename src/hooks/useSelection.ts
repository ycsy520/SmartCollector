import { useCallback, useState } from "react";

// 批量点选的选中集合：按页各自实例化（各页 own 自己的 Set），页面切换/进详情即卸载复位。
export function useSelection() {
  const [mode, setMode] = useState(false); // 多选模式开关
  const [sel, setSel] = useState<Set<string>>(() => new Set());

  const toggle = useCallback((id: string) => {
    setSel((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  // 全选=用给定的可见 id 集替换当前选择（各页传入"当前筛选结果"的 id，故全选范围==看到范围）。
  const selectAll = useCallback((ids: string[]) => setSel(new Set(ids)), []);
  // Shift 连选：并入而不是替换——先前手动勾选的不能被一次连选抹掉。
  const addMany = useCallback(
    (ids: string[]) =>
      setSel((prev) => {
        const next = new Set(prev);
        for (const id of ids) next.add(id);
        return next;
      }),
    []
  );
  const clear = useCallback(() => setSel(new Set()), []);
  // 退出多选模式连带清空选择：回到普通浏览不该残留上一次的勾选。
  const exit = useCallback(() => {
    setMode(false);
    setSel(new Set());
  }, []);

  return {
    mode,
    setMode,
    exit,
    has: (id: string) => sel.has(id),
    ids: [...sel],
    count: sel.size,
    toggle,
    selectAll,
    addMany,
    clear,
  };
}

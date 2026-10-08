// 相对时间显示（UI 工具，非契约层）。
export function timeAgo(iso: string): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "时间未知";
  const diff = Date.now() - t;
  if (diff < 0) return "刚刚"; // 未来/时钟漂移按刚刚处理
  const m = Math.floor(diff / 60_000);
  if (m < 1) return "刚刚";
  if (m < 60) return `${m} 分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d} 天前`;
  return new Date(iso).toLocaleDateString("zh-CN");
}

/**
 * 本机日历日 key（`YYYY-MM-DD`），与后端 `date(called_at,'localtime')` 同口径。
 * 不用 `toISOString()`：那是 UTC，东八区的清晨会被写成"昨天"。
 */
export function localDayKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

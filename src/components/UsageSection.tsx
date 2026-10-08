// 设置页「大模型使用统计」：这台机器上 AI 到底被调了多少次、烧了多少 token。
// 数据面是 02 §7.5 的 `get_usage_stats`（后端 `llm_calls` 表，批次17）；浏览器演示环境读 mock。
// 三条硬口径在本组件里说清楚，不靠用户猜：
//   ① token 由上游自报，没报的记为"未知"→ 合计只是**下限**；
//   ② **金额不在后端算**，单价由用户按自己配的端点填，所以这里是"估算"且明示非账单；
//   ③ 连接测试的 ping 与检索期的查询编码**不计入**（它们不是在处理用户收集的内容）。
import { useEffect, useState } from "react";
import { Button } from "./ui/button";
import { Chip } from "./ui/chip";
import { Icon } from "./ui/icon";
import { isTauri, getUsageStats } from "../lib/invoke";
import { mockUsageStats } from "../lib/mock";
import { localDayKey } from "../lib/time";
import type { UsageBucket, UsageStats } from "../types/ipc";

// 设备级偏好（与主题偏好同一惯例）：只存本机 localStorage，不进配置数据库、更不外发。
const PRICE_KEY = "sc.usagePrices";
type Price = { in: number; out: number };
type Prices = Record<string, Price>;

const RANGES = [
  { days: 7, label: "近 7 天" },
  { days: 30, label: "近 30 天" },
  { days: 0, label: "全部" },
];

// 后端的 task 是计量归属名（02 §7.5），界面说人话；没见过的原样显示，不编中文名。
const TASK_LABEL: Record<string, string> = {
  classify: "分类",
  tag: "标签",
  summarize: "摘要",
  extract_links: "链接提取",
  skill_run: "再加工",
  embed: "向量化",
};
const taskLabel = (key: string) => TASK_LABEL[key] ?? key;

const nf = new Intl.NumberFormat();
const int = (n: number) => nf.format(n);
const lat = (ms: number) => (ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`);
const yuan = (n: number) => `¥${n.toFixed(2)}`;

function readPrices(): Prices {
  try {
    const raw = localStorage.getItem(PRICE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Prices;
    // 手改过/别的版本写过的 localStorage 属外部输入：只认有限正数，其余丢掉。
    const out: Prices = {};
    for (const [model, p] of Object.entries(parsed ?? {})) {
      const fin = Number.isFinite(p?.in) && p.in >= 0 ? p.in : 0;
      const fout = Number.isFinite(p?.out) && p.out >= 0 ? p.out : 0;
      if (fin || fout) out[model] = { in: fin, out: fout };
    }
    return out;
  } catch {
    return {};
  }
}

// 全 0 等于没填：不能算出"估算 ¥0.00"这种假账，也得和 readPrices() 丢掉零价的行为一致。
const priceOf = (p: Price | undefined): Price | null =>
  p && (p.in > 0 || p.out > 0) ? p : null;

const bucketTokens = (b: UsageBucket) => b.tokensIn + b.tokensOut;
const fullyBlind = (b: UsageBucket) => b.calls > 0 && b.tokensUnknown === b.calls;

/** 按天槽位上限＝13 整周。短窗口（7/30）用柱子，「全部」用活跃格；取整周是为了让格子图的列数刚好收齐，不留半列。 */
const DAY_SLOTS_CAP = 13 * 7;

/**
 * 把按天桶**补齐成连续日历日**。后端 `GROUP BY` 只返回有调用的那天，所以"只有今天有数据"
 * 会被画成一整块满宽的柱子、两端轴标还印着同一个日期——看着像坏了，也读不出"其余几天是零"。
 * 窗口口径与后端一致：`rangeDays > 0` = 含今天在内的 N 个整日；`0` = 从最早记录到今天。
 */
function daySeries(stats: UsageStats | null): {
  slots: { key: string; b: UsageBucket | null }[];
  clipped: number;
} {
  if (!stats) return { slots: [], clipped: 0 };
  const map = new Map(stats.byDay.map((b) => [b.key, b]));
  const today = new Date();
  today.setHours(12, 0, 0, 0); // 正午：躲开夏令时切换日不存在的 00:00
  const start = new Date(today);
  if (stats.rangeDays > 0) {
    start.setDate(start.getDate() - (stats.rangeDays - 1));
  } else {
    const first = stats.firstRecordedAt ? new Date(stats.firstRecordedAt) : null;
    if (first && !Number.isNaN(first.getTime())) {
      first.setHours(12, 0, 0, 0);
      if (first < start) start.setTime(first.getTime());
    }
  }
  const span = Math.round((today.getTime() - start.getTime()) / 86_400_000) + 1;
  const total = Math.max(1, Math.min(span, DAY_SLOTS_CAP));
  const from = new Date(today);
  from.setDate(from.getDate() - (total - 1));
  const slots = Array.from({ length: total }, (_, i) => {
    const d = new Date(from);
    d.setDate(d.getDate() + i);
    const key = localDayKey(d);
    return { key, b: map.get(key) ?? null };
  });
  return { slots, clipped: Math.max(0, span - total) };
}

/** 活跃格的五档底色：0＝那天没有调用（中性色，不冒充"用量很低"），1~4＝当日 token 占最忙那天的比例分档。 */
const LEVEL_BG = ["bg-surface-high", "bg-primary/25", "bg-primary/45", "bg-primary/65", "bg-primary/90"];
const levelOf = (tokens: number, max: number) => {
  if (max <= 0 || tokens <= 0) return 0;
  const r = tokens / max;
  return r > 0.75 ? 4 : r > 0.5 ? 3 : r > 0.25 ? 2 : 1;
};

/**
 * 「全部」档的活跃格：列＝周（周日为首）、行＝星期。
 * 长跨度下柱子会挤成一堵墙（91 根柱子在 600px 里每根不到 7px，读不出任何东西），
 * 而格子图把"哪几天在用、用到什么强度"变成形状本身要说的话。
 */
function ActivityGrid({ slots, max }: { slots: { key: string; b: UsageBucket | null }[]; max: number }) {
  // 首列前补空白格，让每一行真的对应同一个星期几（补位格与"没有调用"的格子必须能区分）。
  const lead = new Date(`${slots[0]?.key ?? ""}T12:00:00`).getDay() || 0;
  const cells: ({ key: string; b: UsageBucket | null } | null)[] = [
    ...Array.from({ length: lead }, () => null),
    ...slots,
  ];
  const weeks = Math.ceil(cells.length / 7);
  while (cells.length < weeks * 7) cells.push(null);
  // 月标：某列的首个真实日期换月时才标一次，标签允许溢出到右侧空白（与 GitHub 同一读法）。
  const monthLabels: string[] = [];
  let lastMonth = -1;
  for (let w = 0; w < weeks; w++) {
    const first = cells.slice(w * 7, w * 7 + 7).find((c) => c) as { key: string } | undefined;
    const m = first ? Number(first.key.slice(5, 7)) : -1;
    monthLabels.push(m !== -1 && m !== lastMonth ? `${m}月` : "");
    if (m !== -1) lastMonth = m;
  }
  return (
    <div>
      {/* 月标行与格子行必须同列宽、同间距，否则标签会漂到隔壁列上去——所以两行都用
          `w-2.5` + `gap-[3px]`，而不是把标签塞进同一个 grid（那会被列优先流式布局错位）。 */}
      <div className="mb-1 flex gap-1.5">
        <div className="w-4 shrink-0" />
        <div className="min-w-0 overflow-x-auto">
          <div className="flex gap-[3px]">
            {monthLabels.map((m, i) => (
              <span key={`m${i}`} className="w-2.5 shrink-0 text-[9px] leading-none text-hint">
                {m}
              </span>
            ))}
          </div>
        </div>
      </div>
      <div className="flex gap-1.5">
        <div className="grid w-4 shrink-0 grid-rows-7 gap-[3px] text-[9px] text-hint">
          {["日", "一", "二", "三", "四", "五", "六"].map((d, i) => (
            <span key={d} className={`h-2.5 leading-[10px] ${i % 2 ? "invisible" : ""}`}>
              {d}
            </span>
          ))}
        </div>
        <div className="min-w-0 overflow-x-auto">
          <div className="grid grid-flow-col gap-[3px]" style={{ gridTemplateRows: "repeat(7, 10px)" }}>
            {cells.map((c, i) =>
              !c ? (
                <span key={`p${i}`} className="w-2.5" />
              ) : (
                <span
                  key={c.key}
                  title={
                    !c.b
                      ? `${c.key} · 没有调用`
                      : fullyBlind(c.b)
                        ? `${c.key} · ${int(c.b.calls)} 次 · 上游未上报 token`
                        : `${c.key} · ${int(c.b.calls)} 次 · ${int(bucketTokens(c.b))} tokens${
                            c.b.failures ? ` · 失败 ${c.b.failures}` : ""
                          }`
                  }
                  className={`w-2.5 rounded-[2px] ${
                    c.b && fullyBlind(c.b)
                      ? "bg-surface-high ring-1 ring-inset ring-outline"
                      : LEVEL_BG[levelOf(c.b ? bucketTokens(c.b) : 0, max)]
                  }`}
                />
              ),
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// 后端按调用次数排序（那是它的计量主键），但这里的条长画的是 token——排序必须跟条长同维，
// 否则"最长的条排在中间"看起来像坏了。
const byTokens = (list: UsageBucket[]) =>
  [...list].sort((a, b) => bucketTokens(b) - bucketTokens(a));

/** 一行的横向条：条长用 token（用户真正关心的量），次数与延迟写在右侧文字里。 */
function BarRow({
  label,
  sub,
  value,
  max,
  right,
  reveal,
}: {
  label: string;
  sub?: string;
  value: number;
  max: number;
  right: string;
  /** 仅面板首次出数时为真：只有那时才播放"从 0 生长"，此后只补间长度。 */
  reveal: boolean;
}) {
  const pct = max > 0 ? Math.max(2, Math.round((value / max) * 100)) : 0;
  return (
    <div className="flex items-center gap-3 py-1">
      <div className="w-24 shrink-0 truncate text-xs font-medium text-on-surface" title={label}>
        {label}
        {sub && <span className="ml-1 font-normal text-hint">{sub}</span>}
      </div>
      <div className="min-w-0 flex-1">
        <div className="h-2.5 overflow-hidden rounded-full bg-surface-high">
          <div
            className={`bar-x ${reveal ? "bar-in" : ""} h-full rounded-full bg-primary/70`}
            style={{ width: `${pct}%` }}
          />
        </div>
      </div>
      <div className="w-28 shrink-0 text-right font-mono text-xs text-on-surface-variant">
        {right}
      </div>
    </div>
  );
}

export function UsageSection() {
  const live = isTauri();
  const [range, setRange] = useState(30);
  const [nonce, setNonce] = useState(0);
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [prices, setPrices] = useState<Prices>(readPrices);
  // 条形只在**首次出数**那一次播放生长；之后切区间靠高度补间表达"数值在变"。
  // 420ms 后摘掉 .bar-in（动画 0.32s 已结束，摘掉不会打断）；若用户在这 0.4 秒内又切了
  // 区间，柱子会直接落到终值——那是可接受的边缘，不是回到闪动。
  const [reveal, setReveal] = useState(true);

  useEffect(() => {
    if (!stats || !reveal) return;
    const t = setTimeout(() => setReveal(false), 420);
    return () => clearTimeout(t);
  }, [stats, reveal]);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    const done = (s: UsageStats) => {
      if (!alive) return;
      setStats(s);
      setLoading(false);
    };
    const fail = (e: unknown) => {
      if (!alive) return;
      // 不假装成 0：读不到就是要说出来，否则空面板会被读成"这个月很省"。
      setError(e instanceof Error ? e.message : String(e));
      setStats(null);
      setLoading(false);
    };
    if (live) getUsageStats(range).then(done, fail);
    else done(mockUsageStats(range));
    return () => {
      alive = false;
    };
  }, [live, range, nonce]);

  const setPrice = (model: string, side: "in" | "out", raw: string) => {
    const n = Number(raw);
    const next: Prices = { ...prices };
    const cur = next[model] ?? { in: 0, out: 0 };
    const value = Number.isFinite(n) && n >= 0 ? n : 0;
    next[model] = { ...cur, [side]: value };
    setPrices(next);
    try {
      localStorage.setItem(PRICE_KEY, JSON.stringify(next));
    } catch {
      /* 隐私模式写不进去：本次会话内仍然生效，不为此打扰用户 */
    }
  };

  const maxDay = stats ? Math.max(1, ...stats.byDay.map(bucketTokens)) : 1;
  const { slots: days, clipped } = daySeries(stats);
  const maxTask = stats ? Math.max(1, ...stats.byTask.map(bucketTokens)) : 1;
  const maxModel = stats ? Math.max(1, ...stats.byModel.map(bucketTokens)) : 1;

  const estimated =
    stats?.byModel.reduce((acc, b) => {
      const p = priceOf(prices[b.key]);
      if (!p) return acc;
      return acc + (b.tokensIn / 1000) * p.in + (b.tokensOut / 1000) * p.out;
    }, 0) ?? 0;
  const priced = stats?.byModel.some((b) => priceOf(prices[b.key])) ?? false;

  const rangeLabel = RANGES.find((r) => r.days === range)?.label ?? "近 30 天";
  const costOf = (b: UsageBucket) => {
    const p = priceOf(prices[b.key]);
    if (!p) return "未填单价";
    return yuan((b.tokensIn / 1000) * p.in + (b.tokensOut / 1000) * p.out);
  };
  const empty = !loading && !error && (stats?.totalCalls ?? 0) === 0;
  const firstDay = stats?.firstRecordedAt
    ? new Date(stats.firstRecordedAt).toLocaleDateString(undefined, { hour12: false })
    : null;

  return (
    <section className="md-elev rounded-2xl bg-card p-5">
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <h2 className="text-xs font-semibold text-hint">大模型使用统计</h2>
        {!live && (
          <span className="rounded-full border border-outline-variant px-2 py-0.5 text-[11px] text-hint">
            浏览器演示数据
          </span>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          {RANGES.map((r) => (
            <Chip
              key={r.days}
              label={r.label}
              active={range === r.days}
              onClick={() => setRange(r.days)}
            />
          ))}
          <Button
            variant="text"
            icon="RefreshCw"
            // 文案必须恒定：原先"刷新"↔"读取中…"来回切会让按钮变宽 ~24px，而这一行是
            // ml-auto 右对齐的——三枚区间 Chip 被整排推开又弹回，就是用户报的"点一下闪一下"。
            // 加载态改由图标转动 + disabled 表达，宽度不再参与状态表达。
            className={`ml-1 h-7 px-2 ${loading ? "[&>svg]:animate-spin" : ""}`}
            onClick={() => setNonce((n) => n + 1)}
            disabled={loading}
            aria-label="重新读取统计"
            title={loading ? "正在读取…" : "重新读取统计"}
          >
            刷新
          </Button>
        </div>
      </div>

      {error && (
        <p className="caption-in flex items-start gap-1.5 text-xs leading-relaxed text-error">
          <Icon name="TriangleAlert" size={13} className="mt-0.5 shrink-0" />
          <span>统计读取失败：{error}</span>
        </p>
      )}

      {loading && !stats && (
        <p className="text-xs text-hint">正在读取本机的调用记录…</p>
      )}

      {empty && (
        <p className="text-xs leading-relaxed text-on-surface-variant">
          {firstDay
            ? `${rangeLabel}内没有调用记录；库里最早的记录是 ${firstDay}。`
            : "这台机器上还没有过一次模型调用。统计从后台处理第一条内容开始记录。"}
        </p>
      )}

      {stats && stats.totalCalls > 0 && (
        <>
          <div className="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-4">
            {[
              {
                head: "调用",
                big: int(stats.totalCalls),
                sub: `chat ${int(stats.chatCalls)} · embedding ${int(stats.embeddingCalls)}`,
              },
              {
                head: "token",
                big: int(stats.tokensIn + stats.tokensOut),
                sub: `输入 ${int(stats.tokensIn)} · 输出 ${int(stats.tokensOut)}`,
              },
              {
                head: "失败",
                big: int(stats.totalFailures),
                sub: `${((stats.totalFailures / stats.totalCalls) * 100).toFixed(1)}% 的请求`,
              },
              {
                head: "平均耗时",
                big: lat(stats.avgLatencyMs),
                sub: "含退避重试的整次调用",
              },
            ].map((c) => (
              <div key={c.head}>
                <div className="text-[11px] text-hint">{c.head}</div>
                <div className="font-mono text-lg leading-tight text-on-surface">{c.big}</div>
                <div className="text-[11px] leading-snug text-on-surface-variant">{c.sub}</div>
              </div>
            ))}
          </div>

          {stats.tokensUnknown > 0 && (
            <p className="caption-in mt-2 flex items-start gap-1.5 text-xs leading-relaxed text-hint">
              <Icon name="Info" size={13} className="mt-0.5 shrink-0" />
              <span>
                其中 {int(stats.tokensUnknown)} 次调用上游没回 token 数，没算进合计——
                所以下面的 token 总量与金额都只是
                <span className="font-semibold text-on-surface">下限</span>，不是账单。
              </span>
            </p>
          )}

          <div className="mt-3">
            <div className="mb-1 text-[11px] text-hint">
              {stats.rangeDays === 0
                ? "按天活跃（色深＝当日 token 占最忙那天的比例）"
                : "按天 token（柱高＝当日输入+输出）"}
            </div>
            {stats.rangeDays === 0 ? (
              <div className="caption-in">
                <ActivityGrid slots={days} max={maxDay} />
              </div>
            ) : (
              <>
                <div className="flex h-24 items-end gap-[2px]">
                  {days.map(({ key, b }) => {
                    if (!b)
                      return (
                        <div
                          key={key}
                          title={`${key} · 没有调用`}
                          className="h-[3px] flex-1 rounded-t bg-outline-variant/70"
                        />
                      );
                    const blind = fullyBlind(b);
                    // 盲天没有 token 数，任何百分比都是编的——画成贴底的一格短横，并在下面说明它不表示用量。
                    const pct = blind ? 0 : Math.max(3, Math.round((bucketTokens(b) / maxDay) * 100));
                    return (
                      <div
                        key={key}
                        title={`${key} · ${int(b.calls)} 次 · ${
                          blind ? "上游未上报 token" : `${int(bucketTokens(b))} tokens`
                        }${b.failures ? ` · 失败 ${b.failures}` : ""}`}
                        className={`bar-y ${reveal ? "bar-in" : ""} flex-1 rounded-t ${blind ? "bg-outline/80" : "bg-primary/70"}`}
                        style={{ height: blind ? "4px" : `${pct}%` }}
                      />
                    );
                  })}
                </div>
                <div className="mt-1 flex justify-between text-[11px] text-hint">
                  <span>{days[0]?.key ?? ""}</span>
                  <span>{days[days.length - 1]?.key ?? ""}</span>
                </div>
              </>
            )}
            <p className="mt-1 text-[11px] leading-relaxed text-hint">
              {clipped > 0 && `图只画最近 ${DAY_SLOTS_CAP} 天（更早的 ${clipped} 天仍算进上面的合计）。`}
              {stats.rangeDays === 0 ? (
                <>
                  深浅四档分别＝当日 token 占最忙那天的 25% 以内 / 25~50% / 50~75% / 75% 以上；
                  与底色相同的空格＝那天一次调用都没有。
                  {stats.byDay.some(fullyBlind) &&
                    " 带细边框的空心格＝那天有调用、但上游没回 token 数（有些端点确实不回），它不表示用量。"}
                  {"　"}悬停任一格可看确切次数与 token。
                </>
              ) : (
                <>
                  贴底的淡横＝那天一次调用都没有，不是"用量为零"的误读——它只是把窗口里的空位留在原地，
                  否则只有今天有数据时会被拉成一整块满宽的柱子。
                  {stats.byDay.some(fullyBlind) &&
                    " 颜色略深的贴底短横＝那天有调用、但上游没回 token 数（有些端点确实不回），它不表示用量、也没有高度。"}
                </>
              )}
            </p>
          </div>

          <div className="mt-3 border-t border-outline-variant/60 pt-2">
            <div className="mb-1 text-[11px] text-hint">按任务</div>
            {byTokens(stats.byTask).map((b) => (
              <BarRow
                key={b.key}
                label={taskLabel(b.key)}
                sub={TASK_LABEL[b.key] ? undefined : "(未知任务)"}
                value={bucketTokens(b)}
                max={maxTask}
                right={`${int(b.calls)} 次 · ${lat(b.avgLatencyMs)}`}
                reveal={reveal}
              />
            ))}
          </div>

          <div className="mt-3 border-t border-outline-variant/60 pt-2">
            <div className="mb-1 flex flex-wrap items-baseline gap-x-2 text-[11px] text-hint">
              <span>按模型与估算金额</span>
              <span>
                单价你填（元 / 千 token，含税与否按你供应商的口径），本应用不联网取价、不替你猜
              </span>
            </div>
            {byTokens(stats.byModel).map((b) => (
              <div key={b.key}>
                <BarRow
                  label={b.key}
                  value={bucketTokens(b)}
                  max={maxModel}
                  right={`${int(b.calls)} 次 · ${costOf(b)}`}
                  reveal={reveal}
                />
                <div className="mb-1 ml-24 flex flex-wrap items-center gap-2 text-[11px] text-on-surface-variant">
                  <label className="flex items-center gap-1">
                    输入
                    <input
                      type="number"
                      inputMode="decimal"
                      min={0}
                      step={0.001}
                      value={prices[b.key]?.in ?? ""}
                      placeholder="0"
                      onChange={(e) => setPrice(b.key, "in", e.target.value)}
                      className="h-6 w-20 rounded-md border border-outline-variant bg-surface-high px-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary/60"
                    />
                  </label>
                  <label className="flex items-center gap-1">
                    输出
                    <input
                      type="number"
                      inputMode="decimal"
                      min={0}
                      step={0.001}
                      value={prices[b.key]?.out ?? ""}
                      placeholder="0"
                      onChange={(e) => setPrice(b.key, "out", e.target.value)}
                      className="h-6 w-20 rounded-md border border-outline-variant bg-surface-high px-1.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary/60"
                    />
                  </label>
                  {b.tokensUnknown > 0 && (
                    <span className="text-hint">
                      另有 {int(b.tokensUnknown)} 次未上报 token，本行合计为下限
                    </span>
                  )}
                </div>
              </div>
            ))}
            <p className="mt-1 text-xs leading-relaxed text-on-surface-variant">
              {priced ? (
                <>
                  <span className="font-semibold text-on-surface">{rangeLabel}估算 {yuan(estimated)}</span>
                  {"　"}按你填的单价推算，不等于供应商账单；改单价只影响这一栏，不会重算历史。
                </>
              ) : (
                <>还没填单价，所以没有金额。填了才会算——本应用不知道你的合同价，也不猜。</>
              )}
            </p>
          </div>

          <p className="mt-3 border-t border-outline-variant/60 pt-2 text-[11px] leading-relaxed text-hint">
            口径：只统计后台处理与再加工产生的调用（分类 / 标签 / 摘要 / 链接提取 / 技能 / 落向量）。
            设置页的「连接测试」与检索时把查询词编码的那次调用不在其中。
            记录只存时刻、端点、任务、模型、token 数、成败与耗时——正文、提示词、响应原文一个字节都不落。
          </p>
        </>
      )}
    </section>
  );
}

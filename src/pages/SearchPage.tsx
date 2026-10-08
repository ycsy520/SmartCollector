import { useEffect, useMemo, useRef, useState } from "react";
import type { SearchHit, SearchMode } from "../types/ipc";
import { fakeSearch } from "../lib/mock";
import { useFragments } from "../stores/fragments";
import { searchFragments, isTauri } from "../lib/invoke";
import { FragmentCard } from "../components/FragmentCard";
import { Chip } from "../components/ui/chip";
import { Icon } from "../components/ui/icon";

const LIVE = isTauri();
const MODES: { value: SearchMode; label: string }[] = [
  { value: "hybrid", label: "混合" },
  { value: "keyword", label: "关键词" },
  { value: "semantic", label: "语义" },
];

export function SearchPage({
  onOpen,
  activeId,
}: {
  onOpen: (id: string) => void;
  activeId?: string | null;
}) {
  const items = useFragments((s) => s.items);
  const [q, setQ] = useState("");
  const [mode, setMode] = useState<SearchMode>("hybrid");
  const [liveHits, setLiveHits] = useState<SearchHit[]>([]);
  const [degraded, setDegraded] = useState(false);
  const [searching, setSearching] = useState(false);
  const reqId = useRef(0);

  // 检索只覆盖归档层：缓冲区未分拣内容不进检索，避免污染结果
  const archived = useMemo(
    () => items.filter((f) => f.layer === "archived"),
    [items],
  );

  // 真实模式：输入去抖后经 IPC 混合检索（后端缺向量时降级为纯 keyword + degraded）。
  // 层口径由后端保证（02 §3.1：两路都排除缓冲区/垃圾站），前端不再二次过滤——二次过滤会让分数与名次对不上，且掩盖后端契约漂移。
  useEffect(() => {
    if (!LIVE) return;
    const text = q.trim();
    if (!text) {
      setLiveHits([]);
      setDegraded(false);
      return;
    }
    const id = ++reqId.current;
    setSearching(true);
    const t = setTimeout(() => {
      searchFragments({ text, mode, limit: 50 })
        .then((r) => {
          if (id !== reqId.current) return; // 丢弃过期响应
          setLiveHits(r.items);
          setDegraded(!!r.degraded);
        })
        .catch(() => {
          if (id !== reqId.current) return;
          setLiveHits([]);
        })
        .finally(() => {
          if (id === reqId.current) setSearching(false);
        });
    }, 250);
    return () => clearTimeout(t);
  }, [q, mode]);

  const hits = LIVE ? liveHits : q.trim() ? fakeSearch(archived, q, mode) : [];
  // 作用域条要说的"没参与检索的那部分"= 缓冲区已加载的条目（LIVE 下 load() 每层取 500，口径与首页一致）
  const bufferCount = useMemo(
    () => items.filter((f) => f.layer === "buffer").length,
    [items],
  );

  return (
    <div className="mx-auto flex h-full max-w-3xl flex-col gap-4 overflow-y-auto px-6 py-5">
      <section className="flex items-center gap-2">
        <div className="relative min-w-0 flex-1">
          <Icon
            name="Search"
            size={16}
            className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-hint"
          />
          <input
            autoFocus
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={LIVE ? "搜索已归档的片段" : "搜索已收集的片段（mock：本地匹配；真实版为 FTS5+向量 RRF）"}
            className="w-full rounded-xl border border-outline bg-card py-2.5 pl-9 pr-4 text-sm text-on-surface outline-none transition-colors ease-emph placeholder:text-hint focus:border-primary focus:ring-1 focus:ring-primary/40"
          />
        </div>
        <div className="flex rounded-full border border-outline-variant bg-surface-low p-1">
          {MODES.map((m) => (
            <button
              key={m.value}
              onClick={() => setMode(m.value)}
              className={`md-press rounded-full px-3 py-1 text-xs font-semibold ${
                mode === m.value
                  ? "bg-secondary-container text-on-secondary-container"
                  : "text-on-surface-variant hover:bg-surface-high"
              }`}
            >
              {m.label}
            </button>
          ))}
        </div>
      </section>

      {/* 作用域条（Keep 借鉴 ③）：检索范围与语义路状态**常驻可见**——
          搜不到时用户要能区分"没有相关内容"和"我搜的本来就不包括缓冲区"。 */}
      <section
        aria-label="检索作用域"
        className="-mt-2 flex flex-wrap items-center gap-1.5 text-xs"
      >
        <span className="text-hint">作用域</span>
        <Chip
          label="只搜归档库"
          className="bg-secondary-container text-on-secondary-container"
        />
        <Chip
          label={`模式：${MODES.find((m) => m.value === mode)?.label ?? mode}`}
          className="bg-surface-high text-on-surface-variant"
        />
        {mode !== "keyword" && (
          <Chip
            label={
              !LIVE
                ? "语义路：mock 未接"
                : degraded
                  ? "语义路：降级为关键词"
                  : "语义路：已启用"
            }
            className={
              LIVE && !degraded
                ? "bg-surface-high text-on-surface-variant"
                : "border border-outline-variant bg-transparent text-on-surface-variant"
            }
          />
        )}
        {bufferCount > 0 && (
          <span className="text-hint">
            碎片区 {bufferCount} 条未参与（未分拣不进检索）
          </span>
        )}
      </section>

      {q.trim() === "" ? (
        <p key="hint" className="caption-in py-16 text-center text-sm text-hint">
          输入关键词开始检索
        </p>
      ) : searching ? (
        <p key="busy" className="caption-in py-16 text-center text-sm text-hint">检索中…</p>
      ) : hits.length === 0 ? (
        <div key="none" className="caption-in py-16 text-center text-sm text-hint">
          <p>没有命中「{q}」</p>
          {mode !== "keyword" && (
            <p className="mt-2 text-xs">
              {LIVE
                ? degraded
                  ? "（语义路暂不可用，已按关键词检索）"
                  : "（语义路参与检索，两路都没有相近条目）"
                : "语义检索依赖向量库（未接功能，当前仅 mock 匹配）"}
            </p>
          )}
        </div>
      ) : (
        /* 结果列表刻意不加进入动画也不重新挂载：检索每次 settle 都从 busy 落到 hits，
           keyed 会让整列重建——用户刚展开的卡片被折叠、长列表白重画一次。 */
        <section className="flex flex-col gap-3 pb-6">
          {degraded && LIVE && (
            <p className="text-xs text-on-warn-container">
              语义路暂不可用（未配置 embedding 或调用失败），结果只按关键词匹配。
            </p>
          )}
          {hits.map((h) => (
            <FragmentCard
              key={h.fragment.id}
              item={h.fragment}
              query={q.trim()}
              score={h.score}
              matched={h.matched}
              active={h.fragment.id === activeId}
              onClick={() => onOpen(h.fragment.id)}
            />
          ))}
        </section>
      )}
    </div>
  );
}

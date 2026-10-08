// 归档库：展示 layer==='archived' 的片段，补齐"归档后消失"的查看缺口。
// 过滤用本地状态，不与首页共享 store.filters，避免跨页筛选串味。
import { useEffect, useMemo, useRef, useState } from "react";
import type { FragmentDetail } from "../types/ipc";
import { CATEGORIES, MEDIA_TYPES, MEDIA_TYPE_LABEL } from "../types/enums";
import { toSummary } from "../lib/mock";
import * as api from "../lib/invoke";
import { useFragments } from "../stores/fragments";
import { FragmentCard } from "../components/FragmentCard";
import { FragmentTile } from "../components/FragmentTile";
import { BatchBar } from "../components/BatchBar";
import { useSelection } from "../hooks/useSelection";
import { ConfirmDialog } from "../components/ui/dialog";
import { toast, startToast } from "../components/ui/toast";
import { Chip } from "../components/ui/chip";
import { Icon, type IconName } from "../components/ui/icon";

function dateLabel(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "日期未知";
  const md = `${d.getMonth() + 1}月${d.getDate()}日`;
  const weekday = ["日", "一", "二", "三", "四", "五", "六"][d.getDay()];
  return `${md} 周${weekday}`;
}

const dayKey = (iso: string) => iso.slice(0, 10);
const LIVE = api.isTauri();

// 写文本到剪贴板。真窗口 origin 是 http://tauri.localhost，属非安全上下文，
// navigator.clipboard 可能整个不存在，留一条 execCommand 兜底（不受安全上下文限制）。
async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* 落到兜底路径 */
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}

type View = "list" | "grid";
// 设备级偏好，落 config 表要走 config://changed 与 DTO 扩字段，成本不对等（剃刀）。
const VIEW_KEY = "sc.library.view";
const readView = (): View => {
  try {
    return localStorage.getItem(VIEW_KEY) === "grid" ? "grid" : "list";
  } catch {
    return "list"; // 隐私模式/无 storage：退回默认，不让整页白屏
  }
};

const VIEWS: { value: View; icon: IconName; label: string; tip: string }[] = [
  { value: "list", icon: "List", label: "列表", tip: "按日期分组的完整卡片：能看标签、直接丢弃" },
  { value: "grid", icon: "LayoutGrid", label: "网格", tip: "密排浏览，不分组：只看不分拣（无标签、无丢弃）" },
];

export function LibraryPage({
  onOpen,
  activeId,
}: {
  onOpen: (id: string) => void;
  activeId?: string | null;
}) {
  const items = useFragments((s) => s.items);
  const loading = useFragments((s) => s.loading);
  const trash = useFragments((s) => s.trash);
  const setLayer = useFragments((s) => s.setLayer);
  const [view, setView] = useState<View>(readView);
  const [category, setCategory] = useState<string>("all");
  const [mediaType, setMediaType] = useState<string>("all");
  const [tag, setTag] = useState<string | null>(null);
  const [review, setReview] = useState<"all" | "pending">("all"); // 补审队列：pending=只看超时替你收、未人工审过的
  const [leaving, setLeaving] = useState<Set<string>>(() => new Set());

  // 丢弃的被动确认：与首页一致的退场动效（成功 toast 已全局移除，靠卡片滑走留痕）。
  // leaving 类在 commit 后仍保留，避免 LIVE 回执未到时的闪回；1.6s 兜底让失败卡片回位。
  const withLeave = (id: string) => {
    setLeaving((s) => new Set(s).add(id));
    setTimeout(() => trash(id), 200);
    setTimeout(() => {
      setLeaving((s) => {
        const n = new Set(s);
        n.delete(id);
        return n;
      });
    }, 1600);
  };

  // 批量丢弃（#72）：归档库只能整体丢弃（已归档，无"再放行"）。撤销 best-effort 回原层。
  const sel = useSelection();
  const [batchDiscard, setBatchDiscard] = useState(false);
  const runBatchTrash = () => {
    const targets = items.filter((f) => f.layer === "archived" && sel.has(f.id));
    if (!targets.length) return;
    targets.forEach((f) => setLayer(f.id, "trash"));
    sel.clear();
    toast(`已丢弃 ${targets.length} 条`, "info", {
      label: "撤销",
      onClick: () => targets.forEach((f) => setLayer(f.id, "archived")),
    });
  };

  const archived = useMemo(() => {
    return items
      .filter((f) => f.layer === "archived")
      .filter(
        (f) =>
          (category === "all" || f.result?.category === category) &&
          (mediaType === "all" || f.mediaType === mediaType) &&
          (!tag || (f.result?.tags ?? []).includes(tag)) &&
          (review === "all" || !f.reviewed),
      )
      .sort(
        (a, b) =>
          new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime(),
      );
  }, [items, category, mediaType, tag, review]);

  // 待补审总数：所有已归档但未经人工分拣的（14 天超时替你收的那批）。独立于当前 review 态，
  // 供「待补审」chip 的计数徽标——让用户知道队列里到底还有几条，而非点进去才发现。
  const pendingCount = useMemo(
    () => items.filter((f) => f.layer === "archived" && !f.reviewed).length,
    [items],
  );

  const groups = useMemo(() => {
    const out: { key: string; label: string; items: FragmentDetail[] }[] = [];
    for (const f of archived) {
      const k = dayKey(f.updatedAt);
      const last = out[out.length - 1];
      if (last && last.key === k) last.items.push(f);
      else out.push({ key: k, label: dateLabel(f.updatedAt), items: [f] });
    }
    return out;
  }, [archived]);

  const filterActive = category !== "all" || mediaType !== "all" || tag !== null || review !== "all";
  // 首轮 IPC 在途时列表空≠真的空：说"正在载入"，不把加载中的空帧讲成"你还没有归档内容"
  const emptyText = loading
    ? "正在载入…"
    : filterActive
      ? "没有匹配的归档条目"
      : "还没有归档内容，从碎片区放行后即会出现在这里";

  const switchView = (v: View) => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* 存储写不进去只影响下次记住偏好，本次视图照常切换 */
    }
  };

  // 导出：把**当前筛出的这些条**（含筛选，不含分页外的）落成 Markdown 目录。
  // 用显式 ids 而非把筛选透传给后端，是为了"导出的范围 == 看到的范围"这条不变量在 mock/LIVE 下都成立。
  const [exporting, setExporting] = useState(false);
  const [exportNote, setExportNote] = useState<string | null>(null); // 只常驻落点地址（岛负责处理中/成功/失败）
  // §B 导出指引：把已实现的规则显性化（范围/落点/命名/覆盖/上限），不是新增能力（批次29）。
  const [guideOpen, setGuideOpen] = useState(false);
  const doExport = async () => {
    setExporting(true);
    setExportNote(null);
    // 批次31 第3片：导出进度与结果走顶部岛（处理中→已导出 N 条 / 失败），下方留一行落点地址不重复。
    const h = startToast("导出中…");
    try {
      const out = await api.exportFragments({ ids: archived.map((f) => f.id) });
      h.settle(`已导出 ${out.files.length - 1} 条`);
      setExportNote(out.dir);
    } catch (e) {
      h.settle(`导出失败：${e instanceof Error ? e.message : String(e)}`, "error");
    } finally {
      setExporting(false);
    }
  };

  const guideRef = useRef<HTMLDivElement>(null);
  // 说明浮框无遮罩，靠点击框外收起（比"再点一次说明"更显性）。
  useEffect(() => {
    if (!guideOpen) return;
    const onDown = (e: MouseEvent) => {
      if (guideRef.current && !guideRef.current.contains(e.target as Node)) {
        setGuideOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [guideOpen]);
  const copyPath = async () => {
    if (!exportNote) return;
    const ok = await copyText(exportNote);
    toast(ok ? "落点路径已复制" : "复制失败：本窗口不允许写入剪贴板", ok ? "info" : "error");
  };

  return (
    <div
      className={`mx-auto flex h-full flex-col gap-5 overflow-y-auto px-6 py-6 ${
        view === "grid" ? "max-w-5xl" : "max-w-3xl" // 网格态需要横向余量才谈得上"一屏多条"
      }`}
    >
      <section className="flex items-baseline justify-between gap-3">
        <h2 className="text-lg font-semibold text-on-surface">归档库</h2>
        <div className="flex items-baseline gap-3">
          <span className="text-xs text-hint">共 {archived.length} 条</span>
          <div className="flex rounded-full border border-outline-variant bg-surface-low p-0.5" role="group" aria-label="视图切换">
            {VIEWS.map((v) => (
              <button
                key={v.value}
                title={v.tip}
                aria-pressed={view === v.value}
                onClick={() => switchView(v.value)}
                className={`md-press inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold ${
                  view === v.value
                    ? "bg-secondary-container text-on-secondary-container"
                    : "text-on-surface-variant hover:bg-surface-high"
                }`}
              >
                <Icon name={v.icon} size={13} />
                {v.label}
              </button>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <button
              className="text-xs text-hint underline-offset-2 hover:underline disabled:opacity-40"
              disabled={!LIVE || exporting || archived.length === 0}
              title={
                !LIVE
                  ? "导出需要桌面版（要写文件到应用数据目录）"
                  : archived.length === 0
                    ? "当前筛选没有可导出的条目"
                    : "把当前筛选结果导出为 Markdown 目录"
              }
              onClick={doExport}
            >
              {exporting ? "导出中…" : "导出 Markdown"}
            </button>
            <div ref={guideRef} className="relative">
              <button
                className="inline-flex items-center gap-1 text-xs text-hint underline-offset-2 hover:underline"
                onClick={() => setGuideOpen((v) => !v)}
                aria-expanded={guideOpen}
                title="导出规则说明"
              >
                <Icon name="Info" size={12} /> 说明
              </button>
              {guideOpen && (
                <div className="caption-in absolute right-0 top-full z-20 mt-1.5 w-80 rounded-lg border border-outline-variant bg-card p-3 text-left text-xs leading-relaxed text-on-surface-variant shadow-e2">
                  <div className="mb-1 flex items-center justify-between gap-2">
                    <span className="font-semibold text-on-surface">导出规则</span>
                    <button
                      className="md-press -mr-1 rounded-full p-1 text-hint hover:bg-surface-high hover:text-on-surface"
                      onClick={() => setGuideOpen(false)}
                      title="关闭"
                      aria-label="关闭导出说明"
                    >
                      <Icon name="X" size={14} />
                    </button>
                  </div>
                  <ul className="space-y-0.5">
                    <li>· 导出的是<strong className="font-semibold text-on-surface">当前筛选出的全部条目</strong>（含未归档，所见即所得），一条没筛出会提示空范围。</li>
                    <li>· 每条写成一个 <code className="rounded bg-surface-high px-1">YYYY-MM-DD-名称.md</code>，另有一份 <code className="rounded bg-surface-high px-1">_index.md</code> 总目录。</li>
                    <li>· 落点固定：<strong className="font-semibold text-on-surface">应用数据目录 / exports / 本次时间戳</strong>。每次新建子目录，同名自动加短 id，<strong className="font-semibold text-on-surface">永不覆盖</strong>旧导出。</li>
                    <li>· 不给选目录、不弹保存框；单次无条数上限，大库导出会一次落很多小文件，属正常。</li>
                  </ul>
                </div>
              )}
            </div>
          </div>
        </div>
      </section>
      {exportNote && (
        <div className="caption-in -mt-2 flex items-center gap-2 rounded-lg border border-outline-variant bg-surface-high/40 px-3 py-1.5 text-xs">
          <Icon name="Download" size={13} className="shrink-0 text-hint" />
          <span className="shrink-0 text-hint">落点</span>
          <button
            type="button"
            onClick={copyPath}
            title="点击复制路径"
            className="min-w-0 flex-1 cursor-pointer truncate text-left text-on-surface-variant underline-offset-2 hover:text-primary hover:underline"
          >
            {exportNote}
          </button>
          <Icon name="Copy" size={12} className="-ml-1 shrink-0 text-hint" />
          <button
            className="md-press -mr-1 shrink-0 rounded-full p-1 text-hint hover:bg-surface-high hover:text-on-surface"
            onClick={() => setExportNote(null)}
            title="关闭"
            aria-label="关闭落点提示"
          >
            <Icon name="X" size={14} />
          </button>
        </div>
      )}

      <section className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="w-10 shrink-0 text-xs text-hint">类型</span>
          <Chip
            label="全部"
            active={mediaType === "all"}
            onClick={() => setMediaType("all")}
          />
          {MEDIA_TYPES.map((m) => (
            <Chip
              key={m}
              label={MEDIA_TYPE_LABEL[m]}
              active={mediaType === m}
              onClick={() => setMediaType(mediaType === m ? "all" : m)}
            />
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="w-10 shrink-0 text-xs text-hint">分类</span>
          <Chip
            label="全部"
            active={category === "all"}
            onClick={() => setCategory("all")}
          />
          {CATEGORIES.map((c) => (
            <Chip
              key={c}
              label={c}
              active={category === c}
              onClick={() => setCategory(category === c ? "all" : c)}
            />
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="w-10 shrink-0 text-xs text-hint">分拣</span>
          <Chip
            label="全部"
            active={review === "all"}
            onClick={() => setReview("all")}
          />
          <Chip
            label={pendingCount > 0 ? `待补审 ${pendingCount}` : "待补审"}
            active={review === "pending"}
            onClick={() => setReview(review === "pending" ? "all" : "pending")}
          />
        </div>
        <div className="flex items-center gap-2 text-xs text-hint">
          {tag && (
            <Chip
              label={`标签：${tag} ×`}
              className="bg-primary/10 text-primary"
              onClick={() => setTag(null)}
            />
          )}
          {filterActive && (
            <button
              className="text-xs text-hint underline-offset-2 hover:underline"
              onClick={() => {
                setCategory("all");
                setMediaType("all");
                setTag(null);
                setReview("all");
              }}
            >
              清除筛选
            </button>
          )}
          {archived.length > 0 && (
            <div className="ml-auto flex items-center gap-2">
              {sel.mode && (
                <button
                  className="text-xs text-hint underline-offset-2 hover:underline"
                  onClick={() => sel.selectAll(archived.map((f) => f.id))}
                  title="选中当前筛选结果的全部"
                >
                  全选 {archived.length}
                </button>
              )}
              <button
                className={`text-xs underline-offset-2 hover:underline ${
                  sel.mode ? "font-medium text-primary" : "text-hint"
                }`}
                onClick={() => (sel.mode ? sel.exit() : sel.setMode(true))}
                title={sel.mode ? "退出多选" : "点选多条后批量丢弃"}
              >
                {sel.mode ? "取消多选" : "多选"}
              </button>
            </div>
          )}
        </div>
      </section>

      {view === "grid" ? (
        <section key="grid" className="caption-in grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-4 pb-6">
          {archived.length === 0 && (
            <p className="col-span-full py-16 text-center text-sm text-hint">
              {emptyText}
            </p>
          )}
          {archived.map((f) => (
            <FragmentTile
              key={f.id}
              item={toSummary(f)}
              selectable={sel.mode}
              selected={sel.has(f.id)}
              active={f.id === activeId}
              onClick={sel.mode ? () => sel.toggle(f.id) : () => onOpen(f.id)}
            />
          ))}
        </section>
      ) : (
        <section key="list" className="caption-in flex flex-col gap-5 pb-6">
          {groups.length === 0 && (
            <p className="py-16 text-center text-sm text-hint">{emptyText}</p>
          )}
          {groups.map((g) => (
            <div key={g.key} className="flex flex-col gap-4">
              <div className="flex items-center gap-3">
                <span className="text-xs font-semibold text-on-surface-variant">
                  {g.label}
                </span>
                <span className="h-px flex-1 bg-outline-variant" />
                <span className="text-xs text-on-surface-variant">{g.items.length}</span>
              </div>
              {g.items.map((f) => (
                <div key={f.id} className="border-l-2 border-outline-variant pl-4">
                  <FragmentCard
                    item={toSummary(f)}
                    onClick={sel.mode ? () => sel.toggle(f.id) : () => onOpen(f.id)}
                    selectable={sel.mode}
                    selected={sel.has(f.id)}
                    active={f.id === activeId}
                    leaving={leaving.has(f.id)}
                    onTrash={sel.mode ? undefined : () => withLeave(f.id)}
                  />
                </div>
              ))}
            </div>
          ))}
        </section>
      )}

      {sel.mode && sel.count > 0 && (
        <BatchBar
          count={sel.count}
          onClear={sel.clear}
          actions={[
            { key: "trash", label: "全部丢弃", danger: true, onClick: () => setBatchDiscard(true) },
          ]}
        />
      )}

      <ConfirmDialog
        open={batchDiscard}
        title={`丢弃选中的 ${sel.count} 条？`}
        desc="移入垃圾站，30 天后才彻底清除；点此批撤销可即时回位。"
        confirmText="全部丢弃"
        onCancel={() => setBatchDiscard(false)}
        onConfirm={() => {
          setBatchDiscard(false);
          runBatchTrash();
        }}
      />
    </div>
  );
}

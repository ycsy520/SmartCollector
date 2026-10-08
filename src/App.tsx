// 路由壳：缓冲区 / 检索 / 回收站 / 设置 + 片段详情（无路由库，状态机切换）。
// 宽档（≥1180px）详情在右栏就地并置、列表列保持挂载（滚动位置不丢）；
// 窄档维持"详情整页替换"的原形态——同一 FragmentDetail 两档只挂载一次。
import { useEffect, useState } from "react";
import { HomePage } from "./pages/HomePage";
import { LibraryPage } from "./pages/LibraryPage";
import { FragmentDetail } from "./pages/FragmentDetail";
import { SearchPage } from "./pages/SearchPage";
import { TrashPage } from "./pages/TrashPage";
import { SettingsPage } from "./pages/SettingsPage";
import { Toaster } from "./components/ui/toast";
import { ErrorBoundary } from "./components/ui/error-boundary";
import { AppMark } from "./components/AppMark";
import { TitleBar } from "./components/TitleBar";
import { Icon, type IconName } from "./components/ui/icon";
import { useProcessing } from "./stores/processing";
import { initFragments, useFragments } from "./stores/fragments";
import { onClipboardDraft } from "./lib/events";
import { isTauri } from "./lib/invoke";
import { useMediaQuery } from "./hooks/useMediaQuery";

type Page = "home" | "library" | "search" | "trash" | "settings";

// 1180 = 左竖栏 80 + 列表可读下限 ~520 + 详情下限 ~560 的最小共存宽度；再窄退回单栏跳转。
const WIDE_QUERY = "(min-width: 1180px)";

// 标题栏是真窗口的部件：浏览器 mock 下不渲染（那里标题栏归操作系统）。
const LIVE = isTauri();

const NAV: { page: Page; icon: IconName; label: string }[] = [
  { page: "home", icon: "Inbox", label: "碎片" },
  { page: "library", icon: "Archive", label: "归档" },
  { page: "search", icon: "Search", label: "检索" },
  { page: "trash", icon: "Trash2", label: "回收站" },
  { page: "settings", icon: "Settings", label: "设置" },
];

export default function App() {
  const [page, setPage] = useState<Page>("home");
  const [detailId, setDetailId] = useState<string | null>(null);
  // 加速键注入的剪贴板草稿槽：非 null 即"有一条待消费"，HomePage 取走后置回 null。
  const [paste, setPaste] = useState<{ text: string | null } | null>(null);
  const processing = useProcessing((s) => s.pending + s.running);
  const setFilters = useFragments((s) => s.setFilters);
  const wide = useMediaQuery(WIDE_QUERY);

  // 真实模式：挂载即拉全量 + 订阅事件（浏览器 mock 下 initFragments 空操作）。
  useEffect(() => {
    initFragments();
  }, []);

  // 加速键 = 唤起 + 换到收集页 + 把内容交给输入框（详情一并关掉：粘贴的目标是输入框，
  // 不是眼前这条正在看的片段）。投递延到下一帧：窄档开着详情时列表是卸载的，
  // 同批次 setPaste 会落进"下一帧才存在"的组件里；先收详情、下一帧再投，两种落点走同一条路。
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void onClipboardDraft(({ text }) => {
      if (!alive) return;
      setDetailId(null);
      setPage("home");
      setTimeout(() => {
        if (alive) setPaste({ text });
      }, 0);
    }).then((fn) => {
      if (alive) unlisten = fn;
      else fn();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  const goTag = (tag: string) => {
    setFilters({ tag });
    setPage("home");
    setDetailId(null);
  };

  // Esc 关掉就地详情（键盘用户的"退出"原语，#80 的前哨）。
  // 焦点在输入域内时不劫持：正文/笔记编辑态的草稿是 FragmentDetail 的本地态，
  // 此刻卸载=丢草稿，Esc 在 textarea 里的语义应先留给输入域自己。
  useEffect(() => {
    if (!detailId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "TEXTAREA" || t.tagName === "INPUT" || t.isContentEditable))
        return;
      setDetailId(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [detailId]);

  const list =
    page === "home" ? (
      <HomePage
        onOpen={setDetailId}
        activeId={detailId}
        paste={paste}
        onPasteHandled={() => setPaste(null)}
      />
    ) : page === "library" ? (
      <LibraryPage onOpen={setDetailId} activeId={detailId} />
    ) : page === "search" ? (
      <SearchPage onOpen={setDetailId} activeId={detailId} />
    ) : page === "trash" ? (
      <TrashPage onOpen={setDetailId} activeId={detailId} />
    ) : (
      <SettingsPage />
    );

  // 三态：无详情=单列；宽档有详情=列表｜详情并置（列表不卸载）；窄档有详情=详情整页替换。
  const inline = wide && detailId !== null;
  const showList = detailId === null || inline;

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-surface text-on-surface">
      {LIVE && <TitleBar pageLabel={detailId ? "详情" : NAV.find((n) => n.page === page)!.label} />}
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <nav className="flex w-20 shrink-0 flex-col items-center gap-1.5 border-r border-outline-variant bg-surface-low py-4">
          {/* 标记不再叠 shadow-e1：box-shadow 跟的是 SVG 的方形边框盒，
              不是里面那颗 squircle——四角会露出直角阴影。旧版是可圆角的 div 才配得上它。 */}
          <AppMark size={44} title="Smart Collector" className="mb-3" />
          {NAV.map((n) => {
            // 并置态下列表仍在眼前，一级导航不得因为"开了详情"而失活（窄档才让位）。
            const active = page === n.page && (inline || detailId === null);
            return (
              <button
                key={n.page}
                onClick={() => {
                  setPage(n.page);
                  setDetailId(null);
                }}
                className="md-press relative flex w-full flex-col items-center gap-0.5 py-1 outline-none"
              >
                {/* M3 导航栏标志：active 项的 pill 形指示器 */}
                <span
                  className={`flex h-8 items-center justify-center rounded-full transition-[width,background-color,color] duration-enter ease-emph ${
                    active
                      ? "w-14 bg-secondary-container text-on-secondary-container"
                      : "w-8 text-on-surface-variant hover:bg-surface-high"
                  }`}
                >
                  <Icon name={n.icon} size={20} />
                </span>
                <span
                  className={`text-[11px] transition-colors duration-enter ease-emph ${
                    active ? "font-bold text-on-surface" : "font-medium text-on-surface-variant"
                  }`}
                >
                  {n.label}
                </span>
                {n.page === "home" && processing > 0 && (
                  <span className="absolute right-4 top-0 flex h-4 min-w-4 items-center justify-center rounded-full bg-error px-1 text-[9px] font-bold text-on-error">
                    {processing}
                  </span>
                )}
              </button>
            );
          })}
        </nav>
  
        <main className="flex min-w-0 flex-1 overflow-hidden">
          {showList && (
            // key={page}：切页重挂载以驱动 page-in；开详情时 page 不变，故列表滚动位置不动。
            // 刻意用块级容器而非 flex-col：页面根是 overflow 容器，做 flex item 会按 min-content
            // 定宽（实测 345 栏被撑到 365、横向溢过分隔线），块级则照父宽收缩、与改布局前一致。
            <div
              key={page}
              className={`page-in min-w-0 ${
                inline ? "w-[46%] shrink-0" : "flex-1"
              }`}
            >
              <ErrorBoundary resetKey={page} context={NAV.find((n) => n.page === page)!.label}>
                {list}
              </ErrorBoundary>
            </div>
          )}
          {detailId && (
            <div
              className={`pane-in min-h-0 min-w-0 flex-1 ${
                inline ? "border-l border-outline-variant/70" : ""
              }`}
            >
              <ErrorBoundary resetKey={detailId} context="详情页">
                <FragmentDetail
                  key={detailId}
                  id={detailId}
                  onBack={() => setDetailId(null)}
                  onOpenTag={goTag}
                  onOpen={setDetailId}
                />
              </ErrorBoundary>
            </div>
          )}
        </main>
      </div>

      <Toaster />
    </div>
  );
}

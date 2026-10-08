import { useEffect, useRef, useState } from "react";
import type { Category, FragmentSummary } from "../types/ipc";
import {
  CATEGORIES,
  CATEGORY_COLOR,
  FLAG_LABEL,
  FLAG_TONE,
  FLAG_TONE_STYLE,
  SOURCE_LABEL,
} from "../types/enums";
import { useFragments } from "../stores/fragments";
import { useSkills } from "../stores/skills";
import { relatedFragments } from "../lib/mock";
import { isTauri, listRelated } from "../lib/invoke";
import { Button } from "../components/ui/button";
import { Icon } from "../components/ui/icon";
import { Chip } from "../components/ui/chip";
import { ConfirmDialog } from "../components/ui/dialog";
import { Select } from "../components/ui/form";
import { LinkList } from "../components/LinkList";
import { Markdown } from "../components/Markdown";
import { StatusBadge } from "../components/StatusBadge";
import { MediaThumb } from "../components/MediaThumb";
import { TagInput } from "../components/TagInput";
import { timeAgo } from "../lib/time";
import { safeHref } from "../lib/url";
import { trapTab } from "../lib/focus";
import { imageSrc, stripImageMarker } from "../lib/media";
import { useMediaDir, useLlmDisabled, useLlmReady } from "../stores/settings";
import { BackToTop } from "../components/BackToTop";

const LIVE = isTauri();

// #6 长正文兜底：单条最大 200k 字，全量渲染进详情（尤其 #4 就地详情栏更窄后）会发涩、撑高 DOM。
// 默认只渲染前 PREVIEW_CHARS 字 + 「展开全文」；编辑态始终全量（改内容不该被截断误导）。
const PREVIEW_CHARS = 2000;

// pickedSkills 选择器的空回落常量：每次渲染返回同一引用，避免 zustand 每 tick 生成新数组导致多余重渲染。
const EMPTY_PICKS: string[] = [];

/** 相关碎片的展示形状：两种模式都归一到摘录 + 分类 + 相似度（Detail 有 content，Summary 只有 excerpt）。 */
type RelatedView = Pick<FragmentSummary, "id" | "excerpt" | "category"> & {
  score: number;
};

export function FragmentDetail({
  id,
  onBack,
  onOpenTag,
  onOpen,
}: {
  id: string;
  onBack: () => void;
  onOpenTag: (tag: string) => void;
  onOpen: (id: string) => void;
}) {
  const items = useFragments((s) => s.items);
  const f = items.find((x) => x.id === id);
  // 采集时勾选、尚未加工的「待加工」技能（批次28 C-1：不自动跑，只在这里列出供逐条点）。
  const pickedForThis = useFragments((s) => s.pickedSkills[id] ?? EMPTY_PICKS);
  // 外发在途（批次30-A）：这条片段正有一次技能/指令/重处理在等模型回来时为 true。
  // 期间所有外发触发点一律 disabled + 转圈，杜绝"双击=两次外发=凭空多一版"。
  const running = useFragments((s) => !!s.running[id]);
  const {
    retry,
    patchManual,
    archive,
    trash,
    restore,
    hardDelete,
    setNote,
    setContent,
    runSkill,
    runInstruction,
  } = useFragments.getState();
  const skills = useSkills((s) => s.skills);
  const loadSkills = useSkills((s) => s.load);
  // 快捷再加工按钮需要技能表；进入详情即确保已加载（幂等）。
  useEffect(() => {
    void loadSkills();
  }, [loadSkills]);
  const [noteDraft, setNoteDraft] = useState<string | null>(null); // null=未编辑，回落 store 值
  const [contentDraft, setContentDraft] = useState<string | null>(null); // 原文编辑态（失焦提交）
  const [showAllEdits, setShowAllEdits] = useState(false); // 修订账折叠
  const [contentExpanded, setContentExpanded] = useState(false); // 长正文全文/预览（#6）
  const [confirm, setConfirm] = useState<
    null | "trash" | "delete" | "reprocess" | "skill" | "instruction"
  >(null);
  const [pendingSkill, setPendingSkill] = useState<string | null>(null); // confirm==="skill" 时待跑的技能
  const [instrOpen, setInstrOpen] = useState(false); // 一次性指令输入气泡（批次28 §4）
  const [instrDraft, setInstrDraft] = useState(""); // 一次性指令草稿
  const [pendingInstruction, setPendingInstruction] = useState(""); // confirm==="instruction" 时待发的指令
  const [ver, setVer] = useState(0); // 0=当前，n=历史第 n 旧版本
  const [compareId, setCompareId] = useState<string | null>(null); // dup/conflict 对照
  // 附言/正文/标签的保存反馈统一交顶部灵动岛（批次31 第3片：store 层在回执后承诺"已保存"，
  // 不再就地 blur 即亮一个可能兑现不了的"已保存"——那在 LIVE 下先于后端、文案不诚实）。

  // 火花回路：内容近邻的"相关碎片"。LIVE 走 list_related，浏览器 mock 用本地同算法，
  // 两路归一成同一个展示形状（片段摘要字段不同：Detail 有 content，Summary 只有 excerpt）。
  const [related, setRelated] = useState<RelatedView[]>([]);
  const relatedReq = useRef(0);
  useEffect(() => {
    if (!LIVE || !f) return;
    const seq = ++relatedReq.current;
    listRelated(f.id)
      .then((r) => {
        if (seq !== relatedReq.current) return; // 快速切换片段时丢弃过期响应
        setRelated(
          r.items.map((h) => ({
            id: h.fragment.id,
            excerpt: h.fragment.excerpt.slice(0, 24),
            category: h.fragment.category,
            score: h.score,
          })),
        );
      })
      .catch(() => {
        if (seq === relatedReq.current) setRelated([]); // 相关碎片是锦上添花，失败不打扰
      });
  }, [f?.id, f?.content]);
  useEffect(() => {
    if (LIVE) return;
    setRelated(
      f
        ? relatedFragments(f, items).map(({ frag, score }) => ({
            id: frag.id,
            excerpt: frag.content.slice(0, 24),
            category: frag.result?.category,
            score,
          }))
        : [],
    );
  }, [f?.id, f?.content, items]);

  // 图片资源目录（asset 协议读本机 media 目录）。这是 hook，必须无条件在早退 return 之前调用——
  // 否则删除当前打开的片段使 f 变 undefined、走早退分支时本轮少跑一个 hook，React 抛
  // "Rendered fewer hooks than expected" 直接崩掉整棵树（批次31 黑屏真因）。
  const mediaDir = useMediaDir();
  // 总开关关闭时禁用同步外发的技能/指令（后端仍兜底拒 ConfigMissing）；同属 hook，须在早退之前。
  const llmOff = useLlmDisabled() === true;
  // pending 是否"真的会有加工来"：false=大模型关/未配 key/未配模型，此时不得谎报"AI 正在加工"（承批次28 §5）。
  const llmReady = useLlmReady();

  // 详情正文滚动区引用：交给右下「回到顶部」浮标（滚过约一屏才浮现）。
  const scrollRef = useRef<HTMLDivElement>(null);

  // 头部与正文不重叠（批次8 已把操作条移出滚动流），原先靠一条硬 border-b 分界。
  // 改为滚动后在头部下方浮现柔和阴影、置顶时无线——边缘 cue 只在内容真正滚过时出现。
  // 依赖 f?.id：片段从 null 异步载入或切换时重新挂监听，并借首帧 onScroll() 复位阴影。
  const [headerScrolled, setHeaderScrolled] = useState(false);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => setHeaderScrolled(el.scrollTop > 0);
    onScroll();
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [f?.id]);

  if (!f)
    return (
      <div className="p-10 text-center text-sm text-hint">
        片段不存在（E_NOT_FOUND）
        <button
          className="ml-2 underline transition-colors ease-emph hover:text-primary"
          onClick={onBack}
        >
          返回
        </button>
      </div>
    );

  const history = f.priorResults ?? [];
  const r = ver === 0 ? f.result : history[ver - 1];
  // 技能产物（点评/核查/扩展）与摘要是两种东西，却共用 summary 列——标题必须说实话。
  const skillName = r?.modelUsed?.startsWith("skill:")
    ? r.modelUsed.slice("skill:".length)
    : null;
  const viewingHistory = ver !== 0;
  // 有文件引用就显示图本身（asset 协议读本机 media 目录），取不到时留占位而不是裂图。
  // 批次23：图 + 正文合成一条时 mediaType 记的是 text，所以"是不是图片条目"看 `mediaPath`，
  // 而"正文只是占位文件名"（不经模型的那一类）才看 mediaType==='image'。
  const hasImage = !!f.mediaPath;
  const pureImage = f.mediaType === "image";
  const imgSrc = hasImage ? imageSrc(f.mediaPath, mediaDir) : undefined;
  // 只读态剥掉正文尾部那行占位文件名（存储里留着，它躲开按正文 hash 的活跃去重索引）；
  // **编辑态仍给原文**——改了草稿就照原样落库，不偷偷替用户删掉一行他看得见的正文。
  const bodyText = pureImage ? f.content : stripImageMarker(f.content, f.mediaPath);
  // 图片闸门：后端对 media_type='image' 的 retry/run_skill 一律 E_INPUT_INVALID（02 §2.5/§2.7），
  // 按钮亮着只会换来一次必然失败——索性不亮，并在 title 说实话（图片不经模型）。
  // 合并行不在闸门内：它的正文是用户亲手写的字，本就该被整理，图只是它身上的一张附件。
  const retryable =
    !pureImage &&
    (f.status === "failed" ||
      f.status === "skipped" ||
      (f.status === "done" &&
        (f.result?.degraded || f.flags?.some((fl) => fl.kind === "stale"))));
  // 「重新处理」是外发动作：命中隐私的条目须先二次确认（02 §7.1 / 设计方案 §1.4.2），
  // 因为本地隐私闸门只在收集那一道，编辑后重跑是用户明示覆盖，但不能默认无声外发。
  const isSensitive = f.flags?.some((fl) => fl.kind === "sensitive");
  const retryTitle = pureImage
    ? "图片不经模型，无法重新处理（后端一律拒绝），只能改附言或删掉"
    : "用相同流程重跑分类/标签/摘要/链接：恢复失败/降级结果，或强制整理一条被跳过的记录；想换个整理方向请用摘要下方的「换个方向再加工」。";
  const onRetry = (id: string) => {
    if (isSensitive) setConfirm("reprocess");
    else retry(id, false);
  };
  // 一次性指令弹层提交（批次30-B）：命中隐私 → 转二次确认弹窗（弹层先关，避免两层叠）；
  // 否则当场异步外发一次并关弹层，忙态由 30-A 的 `running` 闸承担（提交即关，摘要区显"正在再加工"）。
  const submitInstruction = () => {
    const text = instrDraft.trim();
    if (!text || running) return; // 在途闸（批次30-A）：连点/连按只发一次
    setInstrOpen(false);
    if (isSensitive) {
      setPendingInstruction(text);
      setConfirm("instruction");
      return;
    }
    runInstruction(f.id, text, false);
    setInstrDraft("");
  };
  // 快捷再加工：命中适用条件（类型×分类）且非自动触发的手动/@技能，对 done 片段一键出新版本。
  const runnable =
    f.result && !viewingHistory
      ? skills.filter(
          (s) =>
            s.enabled &&
            s.trigger !== "auto" &&
            (s.mediaType === "all" || s.mediaType === f.mediaType) &&
            (s.category === "all" || s.category === f.result?.category),
        )
      : [];
  // 「待加工」集合（采集时勾选、尚未跑）：把命中的技能标出来，点它才真的外发（批次28 C-1）。
  const pickedSet = new Set(pickedForThis);
  // 再加工区出现条件：有结果、非历史版本查看态、且非纯图片（后端对图片 retry/run_skill 一律拒）。
  // 一次性指令不依赖是否配有匹配技能，故与 runnable 解耦。
  const canReprocess = !!f.result && !viewingHistory && !pureImage;

  return (
    // 布局：外层不滚动，操作条是它的第一个子（固定头部），滚动区是第二个子。
    // 曾用 sticky（批次6-②），但 Chrome 的 sticky 视口矩形取的是滚动容器 content box——容器 py-5
    // 那 20px 上内边距把吸顶条整体压离顶部，正文从条上方的缝里透出来（用户 2026-09-27 再报），
    // 抵消用的 -mt-5 + pt-5 实测无效（margin 生效、条仍钉在 top:20）。移出滚动流两者同时消失：
    // 正文永远从条下方开始，既不被压也不留缝。右下浮标（回到顶部）绝对定位在本 relative 外层，
    // 不随正文滚动；旧"刻意留空右下"的取舍已随通知升格顶部灵动岛而失效（toast 不再落 bottom-right）。
    <div className="relative mx-auto flex h-full max-w-3xl flex-col">
      {/* 头部只放"能做的动作"与必要的例外提示，不放层级：
          右侧动词组（放行归档 / 移回缓冲区 / 恢复+彻底删除）本身已把层级说出来，再挂一个
          带底色的圆角"缓冲区"徽标既是重复表达，又在「← 返回」旁边谎报可点（形状是按钮的语法、
          实际不可点）；同层浏览时它还恒定不变，纯噪声。状态同理降为点+文字。 */}
      <div className={`flex shrink-0 flex-wrap items-center justify-between gap-x-3 gap-y-2 bg-surface px-6 py-3 transition-shadow ease-emph ${headerScrolled ? "shadow-e1" : ""}`}>
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <button
            className="md-press inline-flex shrink-0 items-center gap-1 text-sm text-on-surface-variant hover:text-primary"
            onClick={onBack}
          >
            <Icon name="ArrowLeft" size={15} />
            返回
          </button>
          {f.archivedBy === "auto" && (
            <span
              className="shrink-0 text-xs text-hint"
              title="超 14 天未分拣，系统替你归档，可补审"
            >
              替你收的
            </span>
          )}
          <StatusBadge status={f.status} bare />
        </div>
        <div className="relative flex flex-wrap items-center justify-end gap-2">
          {f.layer === "buffer" && (
            <>
              <Button
                variant="ghost"
                icon={running ? "Loader2" : "RefreshCw"}
                className={running ? "[&>svg]:animate-spin" : ""}
                onClick={() => onRetry(f.id)}
                disabled={!retryable || running}
                title={running ? "正在重新处理…" : retryTitle}
              >
                重新处理
              </Button>
              <Button
                variant="ghost"
                icon="Trash2"
                onClick={() => setConfirm("trash")}
                className="text-on-surface-variant"
              >
                丢弃
              </Button>
              <Button
                icon="Check"
                onClick={() => archive(f.id)}
                title={
                  f.status === "done" || f.status === "skipped"
                    ? "归档这条片段"
                    : "原文归档：这条尚未经大模型加工，归档后摘要为空，启用大模型会自动补"
                }
              >
                放行归档
              </Button>
            </>
          )}
          {f.layer === "archived" && (
            <>
              <Button
                variant="ghost"
                icon={running ? "Loader2" : "RefreshCw"}
                className={running ? "[&>svg]:animate-spin" : ""}
                onClick={() => onRetry(f.id)}
                disabled={!retryable || running}
                title={running ? "正在重新处理…" : retryTitle}
              >
                重新处理
              </Button>
              <Button
                variant="ghost"
                icon="Undo2"
                onClick={() => {
                  // 批次30-C（P5）：restore 是乐观更新，点完这条已翻回 buffer、离开归档库。
                  // 若留在详情里，会看着一条"已不在本列表"的记录、按钮还翻成缓冲区的放行/丢弃——位置语义错乱。
                  // 关掉详情回到归档列表即为反馈（条目从列表消失 = 可见确认，R2 不弹成功 toast）；
                  // restore 是瞬时的乐观本地操作，加 loading 点只是给即时动作套假等待，故不加。
                  restore(f.id);
                  onBack();
                }}
              >
                移回碎片区
              </Button>
            </>
          )}
          {f.layer === "trash" && (
            <>
              <Button variant="ghost" icon="Undo2" onClick={() => restore(f.id)}>
                恢复
              </Button>
              <Button variant="danger" icon="Trash2" onClick={() => setConfirm("delete")}>
                彻底删除
              </Button>
            </>
          )}
        </div>
      </div>

      {/* 滚动区：旗标/原文/摘要/技能/相关碎片都在此上下滚，上方头部不动。
          刻意与子元素同缩进——整块 450 行重新缩进会吞掉真实改动、且多行模板串/预格式文本
          有被缩进污染的风险，故此处只加壳不动子树。 */}
      <div ref={scrollRef} className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-6 py-6">
      <header>
        <div className="flex flex-wrap items-center gap-3 text-xs text-hint">
          <span>来源：{SOURCE_LABEL[f.source]}</span>
          <span>创建于 {timeAgo(f.createdAt)}</span>
          {f.externalUrl &&
            (safeHref(f.externalUrl) ? (
              <a
                href={safeHref(f.externalUrl)}
                target="_blank"
                rel="noopener noreferrer"
                className="truncate text-primary hover:underline"
              >
                原文链接
              </a>
            ) : (
              <span
                className="text-hint"
                title="非 http(s) 协议，已拦截以防 XSS"
              >
                外部链接（已拦截）
              </span>
            ))}
        </div>
      </header>

      {f.flags && f.flags.length > 0 && (
        <section className="flex flex-col gap-1.5">
          {f.flags.map((fl, i) => (
            <div
              key={i}
              className={`flex items-center justify-between gap-3 rounded-lg border px-4 py-2.5 text-sm ${FLAG_TONE_STYLE[FLAG_TONE[fl.kind]]}`}
            >
              <span className="flex min-w-0 items-center gap-2">
                <span className="flex shrink-0 items-center gap-1 font-medium">
                  <Icon name="Flag" size={13} />
                  {FLAG_LABEL[fl.kind]}
                </span>
                <span className="truncate opacity-90">{fl.message}</span>
              </span>
              <span className="flex shrink-0 gap-1.5">
                {fl.kind === "retrash" ? (
                  <>
                    <button
                      onClick={() => trash(f.id)}
                      className="text-xs font-medium text-primary underline-offset-2 transition-colors ease-emph hover:underline"
                    >
                      再次丢弃
                    </button>
                    <span className="px-1 py-0.5 text-xs opacity-60">
                      保留则无需操作
                    </span>
                  </>
                ) : fl.kind === "stale" ? (
                  <button
                    onClick={() => onRetry(f.id)}
                    className="text-xs font-medium text-primary underline-offset-2 transition-colors ease-emph hover:underline"
                  >
                    {fl.action}
                  </button>
                ) : fl.kind === "verify-fail" ? (
                  <button
                    onClick={() => onRetry(f.id)}
                    disabled={!retryable}
                    className="text-xs font-medium text-primary underline-offset-2 transition-colors ease-emph hover:underline disabled:pointer-events-none disabled:opacity-40"
                  >
                    {fl.action}
                  </button>
                ) : (
                  <button
                    onClick={() => fl.refId && setCompareId(fl.refId)}
                    className="text-xs font-medium text-primary underline-offset-2 transition-colors ease-emph hover:underline"
                  >
                    {fl.action}
                  </button>
                )}
              </span>
            </div>
          ))}
        </section>
      )}

      {f.status === "failed" && f.layer === "buffer" && (
        <div className="rounded-lg border border-error/30 bg-error-container px-4 py-2.5 text-sm text-on-error-container">
          处理失败。点「重新处理」会用相同流程再跑一次；多次失败会停在 failed 等你手动重试。
        </div>
      )}
      {r?.degraded && (
        <div className="rounded-lg border border-warn/30 bg-warn-container px-4 py-2.5 text-sm text-on-warn-container">
          降级结果：部分字段由兜底产出。点「重新处理」用相同流程再跑一次，或用下方技能换个整理方向。
        </div>
      )}
      {viewingHistory && (
        <div className="rounded-lg bg-surface-low px-4 py-2.5 text-sm text-on-surface-variant">
          正在查看历史版本 v{history.length - ver + 1}（只读）。
        </div>
      )}

      <section className="md-elev rounded-2xl bg-card p-5">
        <div className="mb-2 flex items-center justify-between">
          <h2 className="text-xs font-semibold text-hint">
            {skillName ? `技能 · ${skillName}` : "摘要"}
          </h2>
          {history.length > 0 && (
            <div className="flex items-center gap-1 text-xs">
              <button
                onClick={() => setVer(0)}
                className={`rounded-full px-2.5 py-0.5 text-xs font-semibold ${ver === 0 ? "bg-primary text-on-primary" : "text-on-surface-variant hover:bg-surface-high"}`}
              >
                当前
              </button>
              {history.map((_, i) => (
                <button
                  key={i}
                  onClick={() => setVer(i + 1)}
                  className={`rounded-full px-2.5 py-0.5 text-xs font-semibold ${ver === i + 1 ? "bg-primary text-on-primary" : "text-on-surface-variant hover:bg-surface-high"}`}
                >
                  v{history.length - i}
                </button>
              ))}
            </div>
          )}
        </div>
        {(f.status === "pending" || f.status === "running") &&
          !(llmReady === false && f.status === "pending") && (
            // 关着大模型的 pending 不亮此条（worker 永不领取，谎报"正在加工"）——降级说明并入下方摘要区，避免两处重复。
            <div className="mb-2 flex items-center gap-2 rounded-lg bg-info-container px-3 py-1.5 text-xs text-on-info-container">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-info" />
              AI 正在加工 · 第 {history.length + 1} 版 · 分类/标签/摘要/链接，通常几秒到几十秒
            </div>
          )}
        {/* 再加工在途（批次30-B）：技能/一次性指令外发时片段状态仍是 done，上面的 pending/running
            提示不会亮——用 30-A 的 running 闸单独给一条"正在再加工"，完成后新版本经 updated 事件自动出现。 */}
        {running && f.status !== "pending" && f.status !== "running" && (
          <div className="caption-in mb-2 flex items-center gap-2 rounded-lg bg-info-container px-3 py-1.5 text-xs text-on-info-container">
            <Icon name="Loader2" size={13} className="animate-spin" />
            AI 正在再加工 · 完成后自动生成新版本，可在上方版本切换查看
          </div>
        )}
        {f.status === "skipped" && (
          <div className="mb-2 flex items-center gap-2 rounded-lg bg-surface-container px-3 py-1.5 text-xs text-on-surface-variant">
            <span className="h-1.5 w-1.5 rounded-full bg-outline-variant" />
            未做 AI 整理 · 系统判定为裸链接 / 简短记录 / 低价值噪声，无需加工；点「重新处理」可强制整理，或直接放行归档
          </div>
        )}
        {skillName && (
          <div className="mb-2 flex items-start gap-2 rounded-lg bg-warn-container px-3 py-1.5 text-xs text-on-warn-container">
            <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-warn" />
            <span>
              上面是「{skillName}」的产出：模型<strong className="font-semibold">生成</strong>的内容，不是检索结果。补出处/核查这类任务它会自信地编错，
              一律按未经核实对待。
            </span>
          </div>
        )}
        <div className="break-words [overflow-wrap:anywhere] text-sm leading-relaxed text-on-surface">
          {!r ? (
            <span className="text-hint">
              {llmReady === false
                ? "尚未加工 · 大模型已关闭或未配置，这条按原文保存；重新启用后会自动补摘要。"
                : "等待处理…"}
            </span>
          ) : r.summary ? (
            // 只渲染模型回来的摘要（当前版/历史版/技能产出都经此）：md 子集 + 全 React 转义，绝不注入 HTML。
            <Markdown text={r.summary} />
          ) : (
            // 空摘要 = 系统判定"这条不该被压缩"（短内容、诗句、路径），不是失败也不是没跑完。
            <span className="text-hint">
              未生成摘要：内容本身即为要点，压缩只会得到一遍复读。
              {runnable.length > 0 && "想换个方向加工，用下面的技能。"}
            </span>
          )}
        </div>
        {canReprocess && (
          <div className="mt-3 flex flex-col gap-2 border-t border-outline-variant pt-3">
            <div className="flex flex-wrap items-center gap-1.5">
              {runnable.length > 0 && (
                <span className="text-xs text-hint">换个方向再加工：</span>
              )}
              {runnable.map((s) => {
                // 采集时勾选、尚未跑的 = 「待加工」：用主色实底 + 小徽标把它和普通可选技能区分开，
                // 让用户看见"当初点的技能还欠着"，但点它才真的外发（批次28 C-1：绝不自动跑）。
                const isPick = pickedSet.has(s.id);
                return (
                  <button
                    key={s.id}
                    disabled={running || llmOff}
                    onClick={() => {
                      if (running) return; // 在途闸（批次30-A）：disabled 已拦，这里是双保险
                      // 技能与「重新处理」是同一类动作：正文整段外发。命中隐私时同样要二次确认，
                      // 否则这条闸门就成了漏口（chip 在批次9 才真正出现在界面上）。
                      if (isSensitive) {
                        setPendingSkill(s.id);
                        setConfirm("skill");
                      } else runSkill(f.id, s, false);
                    }}
                    className={`inline-flex items-center gap-1 rounded-lg border px-2 py-0.5 text-xs transition-colors ease-emph disabled:pointer-events-none disabled:opacity-40 ${
                      isPick
                        ? "border-primary bg-primary/10 text-primary hover:bg-primary/15"
                        : "border-outline text-on-surface-variant hover:border-primary hover:text-primary"
                    }`}
                    title={
                      llmOff
                        ? "大模型已关闭，请先在设置里开启"
                        : running
                        ? "正在加工，请稍候…"
                        : isPick
                          ? `这条收集时勾选了「${s.name}」，点这里才生成新版本`
                          : `用「${s.name}」生成新版本结果`
                    }
                  >
                    <Icon
                      name={running ? "Loader2" : "Sparkles"}
                      size={12}
                      className={running ? "animate-spin" : ""}
                    />
                    {s.name}
                    {isPick && !running && (
                      <span className="rounded bg-primary/15 px-1 text-[10px]">待加工</span>
                    )}
                  </button>
                );
              })}
              {/* §4 一次性指令：不进技能库、用完即走的现场指令，走同一套隐私/图片/done 闸门。
                  批次30-B：由内联细长 input 改为弹层（多行、可从容写），提交即关、异步加工。 */}
              <button
                onClick={() => setInstrOpen(true)}
                disabled={running || llmOff}
                className="inline-flex items-center gap-1 rounded-lg border border-dashed border-outline px-2 py-0.5 text-xs text-on-surface-variant transition-colors ease-emph hover:border-primary hover:text-primary disabled:pointer-events-none disabled:opacity-40"
                title={llmOff ? "大模型已关闭，请先在设置里开启" : "写一条只用在这条片段的一次性指令（不存进技能库，正文会整段发给模型）"}
              >
                <Icon name="Sparkles" size={12} />
                一次性指令
              </button>
            </div>
          </div>
        )}
      </section>

      <section className="md-elev rounded-2xl bg-card p-5">
        <div className="mb-2 flex items-center justify-between">
          <h2 className="text-xs font-semibold text-hint">原文</h2>
          {/* 图片不给「编辑正文」入口：它的正文是 `【图片】<文件名>` 占位，改它既改不到图，
              又会让那行字和真实文件名说谎（能改的描述只有附言）。 */}
          {contentDraft === null && !viewingHistory && f.layer !== "trash" && !pureImage && (
            <button
              onClick={() => setContentDraft(f.content)}
              className="inline-flex items-center gap-1 rounded-lg border border-outline px-2 py-0.5 text-xs text-on-surface-variant transition-colors ease-emph hover:border-primary hover:text-primary"
              title="改正文只在本地保存，不会外发给模型"
            >
              <Icon name="Pencil" size={12} />
              编辑
            </button>
          )}
        </div>
        {/* 编辑态 ↔ 只读态切换：只淡入、不位移（位移会挪动 autoFocus 的落点，让人看见光标在跳）。 */}
        <div className="fade-in" key={contentDraft === null ? "ro" : "edit"}>
        {hasImage && (
          <figure className="mb-3">
            <MediaThumb
              src={imgSrc}
              alt={pureImage ? "" : bodyText.slice(0, 80)}
              iconSize={20}
              imgCls="max-h-[60vh] w-auto max-w-full rounded-lg border border-outline-variant bg-surface-high object-contain"
              boxCls="h-28 w-full bg-surface-high"
              title="图片仍在下面的 media 目录里，只是这台设备取不到预览"
            />
            <figcaption className="mt-1.5 text-xs leading-relaxed text-hint">
              {pureImage
                ? "原样收藏的图片文件，没有被模型看过 · 能搜到的只有下方附言与收集时间"
                : "图片原样存在本机，没有被模型看过 · 下面那段正文照常加工"}
            </figcaption>
          </figure>
        )}
        {contentDraft === null ? (
          (() => {
            const len = bodyText.length;
            const long = len > PREVIEW_CHARS;
            const shown = long && !contentExpanded ? bodyText.slice(0, PREVIEW_CHARS) : bodyText;
            return (
              <>
                <p className="whitespace-pre-wrap break-words [overflow-wrap:anywhere] text-sm leading-[1.8] text-on-surface">
                  {shown}
                  {long && !contentExpanded && <span className="text-hint"> …</span>}
                </p>
                {long && (
                  <button
                    onClick={() => setContentExpanded((v) => !v)}
                    className="mt-1.5 text-xs text-primary underline-offset-2 hover:underline"
                  >
                    {contentExpanded
                      ? "收起"
                      : `展开全文 · 已预览前 ${PREVIEW_CHARS} 字，共 ${len} 字`}
                  </button>
                )}
              </>
            );
          })()
        ) : (
          <>
            <textarea
              value={contentDraft}
              autoFocus
              onChange={(e) => setContentDraft(e.target.value)}
              onBlur={() => {
                if (contentDraft !== f.content) setContent(f.id, contentDraft);
                setContentDraft(null);
              }}
              rows={Math.min(16, Math.max(4, contentDraft.split("\n").length + 1))}
              placeholder="修订正文…（失焦即保存；仅在本地，不发送给模型）"
              className="w-full resize-y rounded-lg border border-outline-variant bg-transparent px-2 py-1.5 text-sm leading-relaxed outline-none focus:border-primary focus:ring-1 focus:ring-primary/40"
            />
            <p className="mt-1 text-xs text-hint">
              保存只改本地正文，不外发；如已有 AI 结果，会标记为「已修订」，需要你主动点「重新处理」才会重整理。
            </p>
          </>
        )}
        </div>
        {f.editLog && f.editLog.length > 0 && (
          <div className="mt-3 border-t border-outline-variant pt-2">
            <div className="mb-1 flex items-center justify-between">
              <h3 className="text-xs font-semibold text-hint">
                修订记录（{f.editLog.length}）
              </h3>
              {f.editLog.length > 3 && (
                <button
                  onClick={() => setShowAllEdits((v) => !v)}
                  className="text-xs text-hint underline-offset-2 hover:text-primary hover:underline"
                >
                  {showAllEdits ? "收起" : `展开全部（${f.editLog.length}）`}
                </button>
              )}
            </div>
            <ul className="flex flex-col gap-1 text-xs text-on-surface-variant">
              {(showAllEdits ? f.editLog : f.editLog.slice(-3)).map((e, i) => (
                <li key={i} className="flex flex-wrap items-baseline gap-x-2">
                  <span className="shrink-0 text-hint">
                    {timeAgo(e.at)}
                  </span>
                  <span className="shrink-0 font-medium text-on-surface-variant">
                    {e.field === "content" ? "改正文" : "改笔记"}
                  </span>
                  <span className="shrink-0">
                    {e.beforeChars}→{e.afterChars} 字
                  </span>
                  {e.excerpt && (
                    <span className="min-w-0 truncate text-hint">
                      改前 {e.excerpt}
                    </span>
                  )}
                </li>
              ))}
            </ul>
          </div>
        )}
      </section>

      <section className="md-elev rounded-2xl bg-card p-5">
        <h2 className="mb-2 text-xs font-semibold text-hint">
          链接（{r?.links.length ?? 0}）
        </h2>
        <LinkList links={r?.links ?? []} />
      </section>

      {r && (
        <section className="md-elev rounded-2xl bg-card p-5">
          <h2 className="mb-2 text-xs font-semibold text-hint">
            分类与标签
          </h2>
          <div className="mb-2 flex items-center gap-2">
            <Select
              value={r.category}
              disabled={viewingHistory}
              onChange={(e) =>
                patchManual(f.id, { category: e.target.value as Category })
              }
              className="w-auto py-1 text-xs"
            >
              {CATEGORIES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </Select>
            <Chip
              label={r.category}
              className={CATEGORY_COLOR[r.category]}
            />
            {r.subcategory && (
              <span className="text-xs text-on-surface-variant">/ {r.subcategory}</span>
            )}
          </div>
          {!viewingHistory && (
            <TagInput
              tags={r.tags}
              onChange={(tags) => patchManual(f.id, { tags })}
            />
          )}
          <div className="mt-2 flex flex-wrap gap-1">
            {r.tags.slice(0, 5).map((t) => (
              <button
                key={t}
                onClick={() => onOpenTag(t)}
                className="text-xs text-hint underline-offset-2 hover:text-primary hover:underline"
              >
                查看「{t}」
              </button>
            ))}
          </div>
        </section>
      )}

      <section className="md-elev rounded-2xl bg-card p-5">
        <h2 className="mb-2 text-xs font-semibold text-hint">
          笔记（人工，参与检索）
        </h2>
        <textarea
          value={noteDraft ?? f.note ?? ""}
          disabled={f.layer === "trash"}
          onChange={(e) => setNoteDraft(e.target.value)}
          onBlur={() => {
            if (noteDraft !== null && noteDraft !== (f.note ?? "")) {
              setNote(f.id, noteDraft);
            }
            setNoteDraft(null);
          }}
          rows={3}
          placeholder="补一句你自己的判断、上下文或待办…（留空即无笔记）"
          className="w-full resize-y rounded-lg border border-outline-variant bg-transparent px-2 py-1.5 text-sm leading-relaxed outline-none focus:border-primary focus:ring-1 focus:ring-primary/40 disabled:bg-surface-high"
        />
      </section>

      {related.length > 0 && (
        <section className="md-elev rounded-2xl bg-card p-5">
          <h2 className="mb-1 text-xs font-semibold text-hint">相关碎片</h2>
          <p className="mb-2 text-xs text-hint">
            内容相近的收集（已配向量时按语义近邻，否则按字面重合度兜底），点开看看能否串起来。
          </p>
          <ul className="flex flex-col gap-1.5">
            {related.map(({ id: rid, excerpt, category, score }) => (
              <li key={rid}>
                <button
                  onClick={() => onOpen(rid)}
                  className="flex w-full items-baseline gap-2 rounded-lg px-3 py-2 text-left transition-colors ease-emph hover:bg-primary/5"
                >
                  <span className="min-w-0 flex-1 truncate text-sm text-on-surface">
                    {excerpt}
                  </span>
                  <span className="shrink-0 text-xs text-hint">
                    {category ?? "—"} · {Math.round(score * 100)}%
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <footer className="pb-6 text-xs leading-relaxed text-hint">
        model_used: {r?.modelUsed ?? "—"} · processed_at:{" "}
        {r ? timeAgo(r.processedAt) : "—"} · updated_at: {timeAgo(f.updatedAt)}
      </footer>

      {compareId &&
        (() => {
          const other = useFragments
            .getState()
            .items.find((x) => x.id === compareId);
          if (!other) return null;
          return (
            <div
              role="dialog"
              aria-modal="true"
              aria-label="片段对照"
              className="scrim-in fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 backdrop-blur-[1px]"
              onClick={() => setCompareId(null)}
              onKeyDown={(e) => {
                trapTab(e, e.currentTarget);
                if (e.key === "Escape") setCompareId(null);
              }}
            >
              <div
                className="modal-in max-h-[80vh] w-full max-w-2xl overflow-y-auto rounded-xl bg-card p-5 shadow-e2"
                onClick={(e) => e.stopPropagation()}
              >
                <h2 className="text-sm font-semibold text-on-surface">
                  片段对照
                </h2>
                <p className="mt-0.5 text-xs text-hint">
                  两条信息高度相似或相互冲突，请择一保留或各自修订。
                </p>
                <div className="mt-3 grid grid-cols-2 gap-3 text-sm">
                  {[
                    { label: "当前", frag: f },
                    { label: "对照", frag: other },
                  ].map(({ label, frag }) => (
                    <div
                      key={label}
                      className="rounded-lg border border-outline-variant p-3"
                    >
                      <div className="mb-1 flex items-center justify-between">
                        <span className="text-xs font-semibold text-on-surface-variant">
                          {label}
                        </span>
                        <span className="text-xs text-hint">
                          {frag.result?.category ?? "—"}
                        </span>
                      </div>
                      <p className="line-clamp-6 min-w-0 whitespace-pre-wrap break-words [overflow-wrap:anywhere] leading-relaxed text-on-surface">
                        {frag.content}
                      </p>
                      {frag.id !== f.id && (
                        <button
                          onClick={() => {
                            trash(frag.id);
                            setCompareId(null);
                          }}
                          className="mt-2 w-full rounded-lg border border-outline py-1 text-xs text-on-surface-variant hover:border-outline"
                        >
                          移入垃圾站
                        </button>
                      )}
                    </div>
                  ))}
                </div>
                <div className="mt-4 flex justify-end">
                  <Button variant="ghost" onClick={() => setCompareId(null)}>
                    关闭
                  </Button>
                </div>
              </div>
            </div>
          );
        })()}
      </div>

      {/* 右下「回到顶部」浮标：正文滚过约一屏才浮现（绝对定位在外层，不随滚动）。 */}
      <BackToTop scrollRef={scrollRef} />

      {/* 一次性指令弹层（批次30-B）：多行输入，Ctrl/Cmd+Enter 发送、Esc 关闭；提交即关、异步加工，
          忙态由 30-A 的 running 闸承担（完成后新版本经 updated 事件自动出现在版本切换里）。
          复用 ConfirmDialog 同款 scrim/modal 动效类，暗色不新增颜色、reduced-motion 已登记降级。 */}
      {instrOpen && (
        <div
          className="scrim-in fixed inset-0 z-40 flex items-center justify-center bg-black/40 backdrop-blur-[1px]"
          onClick={() => setInstrOpen(false)}
          onKeyDown={(e) => {
            trapTab(e, e.currentTarget);
            if (e.key === "Escape") setInstrOpen(false);
          }}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-label="一次性指令"
            className="modal-in flex w-[min(34rem,92vw)] flex-col gap-3 rounded-3xl border border-outline-variant bg-surface-high p-6 shadow-e2"
            onClick={(e) => e.stopPropagation()}
          >
            <div>
              <h3 className="type-headline text-on-surface">一次性指令</h3>
              <p className="mt-1 text-sm text-on-surface-variant">
                只用在当前这条片段，不存进技能库。正文会连同这条指令整段发给模型。
              </p>
            </div>
            <textarea
              autoFocus
              value={instrDraft}
              rows={3}
              onChange={(e) => setInstrDraft(e.target.value)}
              onKeyDown={(e) => {
                if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
                  e.preventDefault();
                  submitInstruction();
                } else if (e.key === "Escape") {
                  setInstrOpen(false);
                }
              }}
              placeholder="例：把它改写成一段给外行看的科普，保留关键数字…"
              className="w-full resize-none rounded-xl border border-outline bg-surface px-3 py-2 text-sm leading-relaxed text-on-surface outline-none placeholder:text-hint focus:border-primary"
            />
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-hint">Ctrl+Enter 发送 · Esc 取消</span>
              <div className="flex gap-2">
                <Button variant="ghost" onClick={() => setInstrOpen(false)}>
                  取消
                </Button>
                <Button
                  icon="Sparkles"
                  disabled={!instrDraft.trim() || running}
                  onClick={submitInstruction}
                >
                  加工
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}

      <ConfirmDialog
        open={confirm !== null}
        title={
          confirm === "delete"
            ? "彻底删除该片段？"
            : confirm === "reprocess"
              ? "重新处理含隐私的碎片？"
              : confirm === "skill"
                ? "再加工含隐私的碎片？"
                : confirm === "instruction"
                  ? "一次性指令含隐私的碎片？"
                  : "丢弃该片段？"
        }
        desc={
          confirm === "delete"
            ? "物理删除，不可恢复。"
            : confirm === "reprocess"
              ? "这条含身份证 / 手机号等隐私特征。重新处理会把正文完整发送给模型，且一经发出不可撤回。确认无误再继续。"
              : confirm === "skill"
                ? `这条含身份证 / 手机号等隐私特征。「${skills.find((s) => s.id === pendingSkill)?.name ?? "该技能"}」会把正文完整发送给模型，且一经发出不可撤回。确认无误再继续。`
                : confirm === "instruction"
                  ? "这条含身份证 / 手机号等隐私特征。一次性指令会把正文连同指令完整发送给模型，且一经发出不可撤回。确认无误再继续。"
                  : `移入垃圾站，${30} 天后自动清除，期间可恢复。`
        }
        confirmText={
          confirm === "delete"
            ? "彻底删除"
            : confirm === "reprocess"
              ? "仍要重新处理"
              : confirm === "skill" || confirm === "instruction"
                ? "仍要发送"
                : "丢弃"
        }
        onCancel={() => setConfirm(null)}
        onConfirm={() => {
          if (confirm === "delete") {
            hardDelete(f.id);
            onBack();
          } else if (confirm === "reprocess") {
            // 用户在这个弹窗里看过警告：把"明示确认"一路带到后端（02 §2.2/§2.3），
            // 后端认这个值才允许把正文重新发给模型；参数本身不外传，只落一次授权位。
            retry(f.id, true);
          } else if (confirm === "skill" && pendingSkill) {
            const s = skills.find((x) => x.id === pendingSkill);
            if (s) runSkill(f.id, s, true); // 同上：明示确认，技能同步外发
          } else if (confirm === "instruction" && pendingInstruction) {
            runInstruction(f.id, pendingInstruction, true); // 一次性指令：确认即当场外发一次
            setInstrDraft("");
            setInstrOpen(false);
          } else {
            trash(f.id);
          }
          setPendingSkill(null);
          setPendingInstruction("");
          setConfirm(null);
        }}
      />
    </div>
  );
}

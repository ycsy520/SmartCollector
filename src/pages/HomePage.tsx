import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { FragmentDetail, Layer, Skill, WeekDigest } from "../types/ipc";
import { CATEGORIES, MEDIA_TYPES, MEDIA_TYPE_LABEL, STATUS_LABEL } from "../types/enums";
import type { FragmentStatus } from "../types/ipc";
import { toSummary } from "../lib/mock";
import { getWeekDigest, isTauri } from "../lib/invoke";
import { useFragments } from "../stores/fragments";
import { useSkills } from "../stores/skills";
import { useSettings, useLlmReady, useLlmDisabled } from "../stores/settings";
import { formatShortcut } from "../lib/shortcut";
import { formatBytes, imageFromDataTransfer, prepareImage } from "../lib/media";
import type { ImagePayload } from "../lib/media";
import { imageUrlsFromHtml } from "../lib/url";
import { FragmentCard } from "../components/FragmentCard";
import { BackToTop } from "../components/BackToTop";
import { BatchBar } from "../components/BatchBar";
import { useSelection } from "../hooks/useSelection";
import { useKeyboardReview } from "../hooks/useKeyboardReview";
import { ConfirmDialog } from "../components/ui/dialog";
import { toast } from "../components/ui/toast";
import { Button } from "../components/ui/button";
import { Chip } from "../components/ui/chip";
import { Icon } from "../components/ui/icon";

const MAX_LEN = 200_000;
const DAY_MS = 86_400_000;
const LIVE = isTauri();
// 缓冲区只暴露"需要再动手"的两种状态：完成/在途是常态，不必占一枚 chip（剃刀）。
// skipped 可筛是批次2 闸门的找回入口——被判低价值的条目混在缓冲区分不出。
const BUFFER_STATUSES: FragmentStatus[] = ["skipped", "failed"];

function dateLabel(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "日期未知";
  const today = new Date();
  const days = Math.floor(
    (new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime() -
      new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()) /
      DAY_MS,
  );
  const md = `${d.getMonth() + 1}月${d.getDate()}日`;
  const weekday = ["日", "一", "二", "三", "四", "五", "六"][d.getDay()];
  const suffix = `周${weekday}`;
  if (days === 0) return `今天 · ${md} ${suffix}`;
  if (days === 1) return `昨天 · ${md} ${suffix}`;
  return `${md} ${suffix}`;
}

const dayKey = (iso: string) => iso.slice(0, 10);

// 本周回顾导语（后端 insight）尾部带一句「待分拣还有 N 条」/「待分拣里攒着 N 条」，与右侧
// 「开始整理」旁的待整理数是同一件事、却两个数据源（后端全库 pending vs 前端 triageCount）。
// 展示时剥掉这句，把待整理数唯一地交给动作区（按钮真正作用的那个数）。措辞源 commands/curation.rs
// ::week_insight；万一后端改词这里没匹配上，最坏退化成旧的双数并列，不崩、不丢信息。
const recapLine = (insight: string) =>
  insight.replace(/[，,]?待分拣(?:还有|里攒着)\s*\d+\s*条。?/g, "").trim();

// 收集框下方的「Tips」轮换提示：一个可扩展的提示池（见组件内 tips），每条有稳定 key。
// 点 x 关闭 = 该条永久不再展示（设备级偏好落 localStorage，与视图/主题偏好同构，不走 config 表）。
// 后续新功能/公告只需往池里加一条；未关闭的多条随机轮换，全部关闭后整行隐藏。
const TIPS_KEY = "sc.home.tips.dismissed";
const readDismissedTips = (): string[] => {
  try {
    const raw = localStorage.getItem(TIPS_KEY);
    const v = raw ? JSON.parse(raw) : [];
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return []; // 隐私模式/坏数据：退回"无已关闭记录"，绝不让首页白屏
  }
};

/** 周回顾卡片的数据形状：done=人工放行+丢弃的"已分拣"总数（两种模式归一到此）。 */
type WeekView = { done: number; auto: number; trashed: number; insight: string };
// 老化：0 天不透明，14 天淡至 0.4（缓冲区不显示倒计时，只用视觉表达停留）
// 老化：0 天不透明，14 天淡至 0.4（缓冲区不显示倒计时，只用视觉表达停留）。
// 量化到 0.001：原始值随 Date.now() 每帧微漂（~5e-8/s），逐字比较会让 memo 化的行每次父渲染都失配；
// 而 0.001 一档≈33 分钟才跳一次，肉眼无差、又能让 dim 作为稳定 prop 命中 memo。
const ageDim = (iso: string) => {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return 1; // 无效日期按新条目处理，不产生 NaN opacity
  const days = (Date.now() - t) / DAY_MS;
  return Math.round(Math.max(0.4, Math.min(1, 1 - (days / 14) * 0.6)) * 1000) / 1000;
};

/**
 * 缓冲区单行（F2 档1-b）：`memo` + 内部 `useMemo(toSummary)`/`useCallback` 把重渲染收敛到
 * "这条自己变了"才发生。三处必须稳定，否则 memo 形同虚设：
 *  - `item`：`toSummary(f)` 每次新建对象 → 挪进本组件 `useMemo([f])`，`f` 引用来自 store
 *    （`setItems` 对未命中条目保持原引用），故只有这条真被更新时才换。
 *  - 三个回调：`onOpen`/`archive`/`trash`/`withLeave`/`onToggle` 全是上游稳定引用（useState setter /
 *    store 一次创建 / useCallback），这里只再 `useCallback` 包一层把 `f.id` 绑进去。
 *  - 每行的标志位：`dim/fresh/leaving/selectable/selected/active/cursor` 都是父层算好的原始值，
 *    同值即相等，memo 直接命中。
 * 行为与原内联渲染逐字对齐：多选态整卡点击=勾选、done/skipped 才给放行、多选态隐藏放行/丢弃。
 */
const BufferRow = memo(function BufferRow({
  f,
  dim,
  fresh,
  leaving,
  selectable,
  selected,
  active,
  cursor,
  onToggle,
  onOpen,
  withLeave,
  archive,
  trash,
}: {
  f: FragmentDetail;
  dim: number;
  fresh: boolean;
  leaving: boolean;
  selectable: boolean;
  selected: boolean;
  active: boolean;
  cursor: boolean;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
  withLeave: (id: string, commit: (id: string) => void) => void;
  archive: (id: string) => void;
  trash: (id: string) => void;
}) {
  const item = useMemo(() => toSummary(f), [f]);
  const onClick = useCallback(
    () => (selectable ? onToggle(f.id) : onOpen(f.id)),
    [selectable, onToggle, onOpen, f.id],
  );
  const onArchive = useCallback(() => withLeave(f.id, archive), [withLeave, archive, f.id]);
  const onTrash = useCallback(() => withLeave(f.id, trash), [withLeave, trash, f.id]);
  return (
    <div
      data-frag={f.id}
      className={`border-l-2 pl-4 ${cursor ? "border-primary" : "border-outline-variant"}`}
    >
      <FragmentCard
        item={item}
        dim={dim}
        fresh={fresh}
        leaving={leaving}
        selectable={selectable}
        selected={selected}
        active={active}
        onClick={onClick}
        onArchive={!selectable ? onArchive : undefined}
        onTrash={!selectable ? onTrash : undefined}
      />
    </div>
  );
});

export function HomePage({
  onOpen,
  activeId,
  paste,
  onPasteHandled,
}: {
  onOpen: (id: string) => void;
  activeId?: string | null; // 并置态：右栏正在看的那条（列表不卸载，靠它高亮落点）
  paste?: { text: string | null } | null; // 加速键注入的剪贴板草稿（见 App）
  onPasteHandled?: () => void; // 消费后清空槽位，避免切页重挂载时二次注入
}) {
  // 逐字段订阅（F2 档1-a）：只有 items/loading/filters 是响应式数据；动作全是 store 里创建一次的
  // 稳定引用，单独 selector 取即可。用整包 `useFragments()` 解构会让 running 在途表、pickedSkills
  // 等任意切片一变就重渲染整个缓冲区列表——而这些切片与本列表的渲染无关，纯浪费。
  const items = useFragments((s) => s.items);
  const loading = useFragments((s) => s.loading);
  const filters = useFragments((s) => s.filters);
  // 「待加工」集合来自会话态 pickedSkills（批次31-A2 的筛选依据）：不落库、重启即清，与详情页同源。
  const pickedSkills = useFragments((s) => s.pickedSkills);
  const setFilters = useFragments((s) => s.setFilters);
  const resetFilters = useFragments((s) => s.resetFilters);
  const submitText = useFragments((s) => s.submitText);
  const submitImage = useFragments((s) => s.submitImage);
  const archive = useFragments((s) => s.archive);
  const trash = useFragments((s) => s.trash);
  const setLayer = useFragments((s) => s.setLayer);
  const keepOriginal = useSettings((s) => s.config.keepOriginalImage);
  const [draft, setDraft] = useState("");
  const [intent, setIntent] = useState("");
  // 批次21-B：粘贴进来的图片先进这个待收槽（缩略图 + 可撤），与「收进来」按钮共用一次提交，
  // 而不是粘贴瞬间就落库——用户得有机会先写一句附言，否则这张图日后只能靠时间找。
  const [pendingImg, setPendingImg] = useState<ImagePayload | null>(null);
  const [picked, setPicked] = useState<Skill | null>(null); // 单选：null = 仅摘要（不加附加技能）
  // 「＋技能」展开开关（改动3-b 渐进披露）：默认收起，采集主流程只留一枚按钮，点开才铺技能 chip。
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [triage, setTriage] = useState(false);
  const [submitMsg, setSubmitMsg] = useState<{ kind: "ok" | "warn" | "err"; text: string } | null>(null);
  const [justId, setJustId] = useState<string | null>(null);
  const [leaving, setLeaving] = useState<Set<string>>(() => new Set());
  const [celebrate, setCelebrate] = useState(false);
  const skills = useSkills((s) => s.skills);
  const loadSkills = useSkills((s) => s.load);

  // @指令补全需要技能表；进入首页即确保已加载（幂等）。
  useEffect(() => {
    void loadSkills();
  }, [loadSkills]);

  // 行内回执条自动淡出（成功 4s、失败 6s 供阅读）；新片段高亮环 1.3s 后解除。
  useEffect(() => {
    if (!submitMsg) return;
    const t = setTimeout(() => setSubmitMsg(null), submitMsg.kind === "ok" ? 4000 : 6000);
    return () => clearTimeout(t);
  }, [submitMsg]);
  useEffect(() => {
    if (!justId) return;
    const t = setTimeout(() => setJustId(null), 1300);
    return () => clearTimeout(t);
  }, [justId]);
  useEffect(() => {
    if (!celebrate) return;
    const t = setTimeout(() => setCelebrate(false), 2600);
    return () => clearTimeout(t);
  }, [celebrate]);

  const atQuery = /(^|\s)@(\S*)$/.exec(intent); // 末尾正在输入的 @token
  const suggestions = atQuery
    ? skills.filter(
        (s) => s.trigger !== "auto" && s.name.includes(atQuery[2]),
      )
    : [];
  // 可手动触发的技能。改动3-b 后展开态一次铺全部（渐进披露面板内不必再二级「更多」）。
  // 排序字段（sort_order）本批不做——需 schema_v9 迁移 + 后端命令，先用当前列表顺序。
  const pickable = skills.filter((s) => s.trigger !== "auto");
  // @ 输入时按名字过滤给补全下拉；这是唯一走 openList 的路径（「更多」二级下拉已随渐进披露移除）。
  const openList = atQuery ? suggestions : [];

  const pickSkill = (s: Skill) => {
    setIntent((v) => v.replace(/(^|\s)@\S*$/, "$1").replace(/\s+$/, " "));
    setPicked((p) => (p?.id === s.id ? null : s)); // 单选：再点已选中的即取消，回到「仅摘要」
    setSkillsOpen(false); // 选完即收起，回到「＋技能 / 已选技能名」的单按钮态
  };

  // 主框键入 @ 时把指令交给附言行，正文与指令物理分离：@token 若混进 draft，
  // 它会进 content、参与 content_hash 判重、进 FTS 索引——同一条原文因为带不带指令
  // 被存成两条，检索命中里还挂着技能名。焦点/光标落点要等 intent 的新值写进 DOM，
  // 所以用布局期 effect + 旗标，而不是 rAF（后台不可见页里 rAF 根本不触发）。
  const intentRef = useRef<HTMLInputElement>(null);
  const caretPending = useRef(false);
  useLayoutEffect(() => {
    if (!caretPending.current) return;
    caretPending.current = false;
    const el = intentRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  });

  const handOffAt = (el: HTMLTextAreaElement) => {
    const before = el.value.slice(0, el.selectionStart ?? el.value.length);
    // 只在行首或空白后接住 @：邮箱、"@某人"夹在句中时不该打断正常输入。
    if (before && !/\s$/.test(before)) return false;
    return true;
  };

  // 加速键（Ctrl/Cmd+Alt+K，键位可在设置→系统 改）把剪贴板文本交到这里：**只填草稿、不提交**。
  // 与托盘「收集剪贴板」是两条路——那条是明示收集、当场落库；这条要把内容留在用户眼前，
  // 由用户决定是否按「收进来」。粘贴后光标落到文末，接着打字即可。
  // 消费完立刻让父级清空这个槽：切走再切回收集页时 HomePage 是重挂载的，
  // 留着旧值会把同一段文本第二次塞进草稿。
  const loadSettings = useSettings((s) => s.load);
  const pasteShortcut = useSettings((s) => s.config.pasteShortcut);
  useEffect(() => {
    // 页脚提示要念出真实键位，所以首页也得有配置（load 幂等；浏览器 mock 下直接 return）。
    void loadSettings();
  }, [loadSettings]);
  // 收集框下方「Tips」轮换提示：关闭状态落 localStorage（见 TIPS_KEY），未关闭的随机挑一条。
  // tipSeed 本次挂载定一次，避免每次渲染重挑导致同一行文案在两次 render 间跳变。
  const [dismissedTips, setDismissedTips] = useState<string[]>(readDismissedTips);
  const [tipSeed] = useState(() => Math.random());
  const tips = useMemo(() => {
    const pool = [
      {
        key: "capture-shortcuts",
        text: LIVE
          ? `Ctrl+Enter 收集 · @ 附加技能 · ${formatShortcut(pasteShortcut)} 粘贴剪贴板`
          : "Ctrl+Enter 收集 · @ 附加技能",
      },
    ];
    const avail = pool.filter((t) => !dismissedTips.includes(t.key));
    return avail.length ? avail[Math.floor(tipSeed * avail.length)] : null;
  }, [pasteShortcut, dismissedTips, tipSeed]);
  const dismissTip = (key: string) => {
    const next = [...dismissedTips, key];
    setDismissedTips(next);
    try {
      localStorage.setItem(TIPS_KEY, JSON.stringify(next));
    } catch {
      /* 存不下（隐私模式）就算了：本次会话内关闭仍生效 */
    }
  };
  const taRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  // 改动5 手感：正文框随内容长高，最少约 3 行（与旧固定态一致，不至于变矮）、最多约 8 行后内部滚动，
  // 省掉「固定 3 行要么太空要么截断」的两难。用布局期 effect 在绘制前定高，避免打字时高度跳一下。
  useLayoutEffect(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(Math.max(el.scrollHeight, 84), 224)}px`;
  }, [draft]);

  const pastePending = useRef(false);
  const pasteTaken = useRef<{ text: string | null } | null>(null);
  useEffect(() => {
    // 按对象身份去重：一次投递只消费一遍（StrictMode 的双跑、切页重挂载都走这里）。
    if (!paste || paste === pasteTaken.current) return;
    pasteTaken.current = paste;
    onPasteHandled?.();
    const t = paste.text;
    if (t === null) {
      setSubmitMsg({
        kind: "warn",
        text: "剪贴板里没有文本；图片请在下面的框里 Ctrl+V 收集",
      });
      return;
    }
    if (!t.trim()) {
      setSubmitMsg({ kind: "warn", text: "剪贴板是空白的" });
      return;
    }
    if (t.length > MAX_LEN) {
      setSubmitMsg({ kind: "err", text: `剪贴板内容 ${t.length} 字符，超过 20 万上限，未填入` });
      return;
    }
    setDraft((d) => (d.trim() ? `${d}\n${t}` : t));
    pastePending.current = true;
  }, [paste, onPasteHandled]);
  useLayoutEffect(() => {
    if (!pastePending.current) return;
    pastePending.current = false;
    const el = taRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  });

  // 粘进来的字节先落到待收槽，不直接入库：附言是图片唯一的可检索描述，
  // 粘贴那一刻用户往往还没想好写什么。
  const attachImage = async (file: File) => {
    try {
      setPendingImg(await prepareImage(file, keepOriginal));
    } catch (e) {
      setSubmitMsg({
        kind: "err",
        text: e instanceof Error ? e.message : "这张图没能读进来，换一张试试",
      });
    }
  };

  // 富文本粘贴（从网页/文档复制带图内容）时，浏览器只把 text/plain 交给 textarea——
  // 图片地址在进框的那一刻就丢了。这里在粘贴这一道口子补读 text/html，把 <img> 的绝对
  // http(s) 地址接到正文末尾：后端既有的链接抽取扫的就是 content，于是这些地址**作为链接留存**
  // （不下载图、不识别图、零新存储零新契约）。地址明晃晃写在框里，用户看得见也删得掉。
  const onPasteRich = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    // 图片字节先走图片这条路（进待收槽，不混进正文）：一张图的 base64 有几十万字符，
    // 让它进 textarea 会把正文、判重哈希、FTS 索引全污染掉，且永远不可能被识别。
    const file = imageFromDataTransfer(e.clipboardData);
    if (file) {
      // 拿到字节就不再接图链：同一张图留两份（本地文件 + 远端地址）是重复，字节那份不会失效。
      // 不 preventDefault —— 同一次粘贴里的文字仍按浏览器默认进正文。
      void attachImage(file);
      return;
    }
    const urls = imageUrlsFromHtml(e.clipboardData.getData("text/html"));
    if (!urls.length) return; // 纯文本粘贴：一个字节都不干预浏览器默认行为
    e.preventDefault();
    const el = e.currentTarget;
    const text = e.clipboardData.getData("text/plain");
    const from = el.selectionStart ?? draft.length;
    const to = el.selectionEnd ?? from;
    const pasted = draft.slice(0, from) + text + draft.slice(to);
    // 原文里本来就印着这些地址（不少文章会写图链）就不重复接
    const extra = urls.filter((u) => !pasted.includes(u));
    if (!extra.length) {
      setDraft(pasted);
      return;
    }
    const block = extra.join("\n");
    const next = `${pasted.trimEnd()}\n\n${block}`;
    if (next.length > MAX_LEN) {
      setDraft(pasted);
      setSubmitMsg({ kind: "warn", text: `接图链会超过 20 万字符上限，只贴进了文字部分` });
      return;
    }
    setDraft(next);
    setSubmitMsg({
      kind: "ok",
      text: `另存了 ${extra.length} 个图片地址（在正文末尾，用不上可删）`,
    });
  };

  const send = async () => {
    const content = draft.trim();
    const img = pendingImg;
    // 就地被动提示，不弹右下角 toast；失败/校验不通过时保留输入原文。
    if (!content && !img) {
      setSubmitMsg({ kind: "err", text: "内容还是空的，先粘贴或输入一点" });
      return;
    }
    if (content.length > MAX_LEN) {
      setSubmitMsg({ kind: "err", text: `内容超长 ${content.length} 字符（上限 20 万）` });
      return;
    }
    const note = intent.trim().replace(/(^|\s)@\S*$/, "$1").trim();
    // 图与文**合成一条**（批次23）：图片字节从不进正文，所以"带一张图的收藏"和"一段可加工的
    // 文字"没必要是两条——一次收集就该是一条。只有图没写文字时才回到批次21-B 那行占位条目
    // （占位正文发给模型只会换来凭空编造的摘要，故那条一律不经模型）。
    let r: { id: string; duplicate: boolean } | null = null;
    const merged = !!img && !!content;
    if (img) {
      const ri = await submitImage(img.data, {
        note: note || undefined,
        content: content || undefined,
        skills: picked ? [picked] : [],
      });
      if (!ri) {
        setSubmitMsg({
          kind: "err",
          text: merged
            ? "没收下，图和文字都还在这里，可以再试一次"
            : "图片收下失败了，缩略图和文字都还在这里，可以再试一次",
        });
        return;
      }
      r = ri;
      setPendingImg(null);
    } else if (content) {
      r = await submitText(content, { note: note || undefined, skills: picked ? [picked] : [] });
      if (!r) {
        setSubmitMsg({ kind: "err", text: "收下失败了，内容已保留，可以再试一次" });
        return;
      }
    }
    setDraft("");
    setIntent("");
    setPicked(null);
    setSkillsOpen(false);
    const parts: string[] = [];
    if (merged)
      parts.push("图与文已合成一条收进碎片区 · 文字部分后台自动加工，图片仍原样存、不经模型");
    else if (img) parts.push("图片已原样收藏 · 不经模型，只按附言与时间查找");
    else if (r)
      parts.push(
        r.duplicate ? "这条文字之前收过了（已定位，未重复新建）" : "文字已收进碎片区 · 后台自动加工中",
      );
    // 只有图却挂了 @技能：技能是提示词再加工，纯图片那条根本不入队，不能假装应用了。
    if (img && !content && picked) parts.push("图片不经模型，再加工技能未应用");
    setSubmitMsg({
      kind: !merged && r?.duplicate ? "warn" : "ok",
      text: parts.join("；"),
    });
    setJustId(r?.id ?? null);
  };

  // 放行/丢弃：先播退场动效（200ms）再落库——动作的被动确认由"卡片滑走 + 计数减少"承载。
  // leaving 类在 commit 后仍保留：LIVE 下 upsert 回执尚未到，此刻解除会让卡片闪回一下。
  // 成功路径随 store 更新即卸载；1.6s 兜底仅用于提交失败时让卡片可见地回位。
  const withLeave = useCallback((id: string, commit: (id: string) => void) => {
    setLeaving((s) => new Set(s).add(id));
    setTimeout(() => commit(id), 200);
    setTimeout(() => {
      setLeaving((s) => {
        const n = new Set(s);
        n.delete(id);
        return n;
      });
    }, 1600);
  }, []);

  // 键盘 Del 比鼠标点「丢弃」更容易误触，故即便点进回收站可恢复，也单独加一道确认。
  const [delId, setDelId] = useState<string | null>(null);

  // 批量裁决（#72）：选中集合本页各自管理，切换页面/进详情即卸载复位。
  const sel = useSelection();
  const [batchDiscard, setBatchDiscard] = useState(false); // 批量丢弃是破坏性动作 → 二次确认

  // 应用到选中的缓冲区条目：逐条改层、清空选择，再发**一条**汇总提示（批次31：批量只占一条队列，
  // 撤销 best-effort 逐条改回 buffer）。单条放行/丢弃的确认由 store 的 archive/trash 各自发岛提示；
  // 这里走 setLayer（静默批量原语）故自己补汇总文案，避免 N 条各弹一次刷屏。
  const runBatch = (to: Layer) => {
    const targets = items.filter((f) => f.layer === "buffer" && sel.has(f.id));
    if (!targets.length) return;
    const ids = targets.map((t) => t.id);
    ids.forEach((id) => setLayer(id, to));
    sel.clear();
    const undo = () => ids.forEach((id) => setLayer(id, "buffer"));
    toast(
      to === "archived" ? `已归档 ${ids.length} 条` : `已移入回收站 ${ids.length} 条`,
      "info",
      { label: "撤销", onClick: undo },
    );
  };

  const buffer = useMemo(() => {
    const list = items
      .filter((f) => f.layer === "buffer")
      .filter(
        (f) =>
          (filters.category === "all" ||
            f.result?.category === filters.category) &&
          (filters.mediaType === "all" || f.mediaType === filters.mediaType) &&
          (filters.status === "all" || f.status === filters.status) &&
          (!filters.tag || (f.result?.tags ?? []).includes(filters.tag)) &&
          (!filters.pendingOnly || (pickedSkills[f.id]?.length ?? 0) > 0) &&
          (!triage || (!f.reviewed && (f.status === "done" || f.status === "skipped"))),
      );
    return list.sort(
      (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime(),
    );
  }, [items, filters, pickedSkills, triage]);

  // 键盘流分拣（#80）：焦点只在"当前看到的"这条列表上移动；并置态下右栏就地跟随，
  // 多选态下移动不抢右栏、Space 勾选、Shift+移动连选区间、Ctrl/Cmd+A 全选、Del 丢弃（弹确认）。
  const visibleIds = useMemo(() => buffer.map((f) => f.id), [buffer]);
  const { cursorId } = useKeyboardReview({
    ids: visibleIds,
    mode: sel.mode,
    open: onOpen,
    toggle: (id) => {
      if (!sel.mode) sel.setMode(true);
      sel.toggle(id);
    },
    selectRange: sel.addMany,
    selectAll: (ids) => {
      sel.setMode(true); // 全选不进多选态就是往一个不显示的集合里写——勾选框根本不渲染
      sel.selectAll(ids);
    },
    requestDelete: (id) => setDelId(id),
    // activeId 非空 ⇒ 列表与详情正并置（窄档下详情会整页替换、本组件根本不挂载）→ 焦点移动可带动右栏。
    follow: activeId != null,
    activeId,
  });

  // 按日期分组（保持组间新→旧）
  const groups = useMemo(() => {
    const out: { key: string; label: string; items: FragmentDetail[] }[] = [];
    for (const f of buffer) {
      const k = dayKey(f.createdAt);
      const last = out[out.length - 1];
      if (last && last.key === k) last.items.push(f);
      else out.push({ key: k, label: dateLabel(f.createdAt), items: [f] });
    }
    return out;
  }, [buffer]);

  // 「碎片区 N 条」= 缓冲区里等你裁决（放行/丢弃）的条目。done 与 skipped 都算：
  // skipped 是被闸门挡下（低价值/含隐私）或图片原样收藏那批，卡片照旧显示放行/丢弃、照旧要你拍板，
  // 把它排除在外会让"碎片区 N 条"这个数字骗人。failed 不算——它只能重试或丢弃，不是"分拣"。
  const triageCount = items.filter(
    (f) =>
      f.layer === "buffer" &&
      !f.reviewed &&
      (f.status === "done" || f.status === "skipped"),
  ).length;
  // 在途：缓冲区里仍在排队/加工（pending/running）的条数——提交后立刻可见的诚实计数。
  // 但未配置模型/密钥时 worker 永不领任务，pending 是"永远不会来的加工"，不能谎报"加工中"（批次28 §5）。
  const llmReady = useLlmReady();
  const llmDisabled = useLlmDisabled();
  const inflight = llmReady === false ? 0 : items.filter(
    (f) => f.layer === "buffer" && (f.status === "pending" || f.status === "running"),
  ).length;
  // 整理模式下理完（碎片区归零）→ 自动退出并给一次轻庆祝。顺带消除"退出按钮消失导致卡死"。
  useEffect(() => {
    if (triage && triageCount === 0) {
      setTriage(false);
      setCelebrate(true);
    }
  }, [triage, triageCount]);
  // 周 digest（火花回路）：本周分拣动作的轻量回顾 + 一句数据推导语。不催办、不打分。
  // LIVE 必须由后端算：items 只是加载进来的当前页，本地数会漏掉未加载的历史动作。
  const [weekDigest, setWeekDigest] = useState<WeekDigest | null>(null);
  const digestReq = useRef(0);
  useEffect(() => {
    if (!LIVE) return;
    const seq = ++digestReq.current;
    getWeekDigest()
      .then((d) => {
        if (seq === digestReq.current) setWeekDigest(d);
      })
      .catch(() => {
        /* 周回顾读不到就不显示，不打扰 */
      });
  }, [items]);

  const mockDigest = useMemo<WeekView | null>(() => {
    if (LIVE) return null; // 真实模式不等本地算，等后端回执
    const weekAgo = Date.now() - 7 * DAY_MS;
    const inWeek = (iso?: string) =>
      iso != null && new Date(iso).getTime() >= weekAgo;
    const archived = items.filter(
      (f) => f.layer === "archived" && inWeek(f.updatedAt),
    );
    const auto = archived.filter((f) => f.archivedBy === "auto").length;
    const manual = archived.length - auto;
    const trashed = items.filter((f) =>
      f.layer === "trash" ? inWeek(f.trashedAt ?? f.updatedAt) : false,
    ).length;
    const counts = new Map<string, number>();
    for (const f of archived) {
      const c = f.result?.category;
      if (c) counts.set(c, (counts.get(c) ?? 0) + 1);
    }
    const top = [...counts.entries()].sort((a, b) => b[1] - a[1])[0];
    const done = manual + trashed;
    let insight: string;
    if (done === 0 && auto === 0)
      insight = "本周还没有分拣记录。碎片区里的东西攒着也无妨，有空再理。";
    else if (top)
      insight = `本周你归档最多的是「${top[0]}」（${top[1]} 条）——这条线要不要开个专档？`;
    else insight = "本周的分拣比较分散，没有明显集中的主题。";
    return { done, auto, trashed, insight };
  }, [items]);

  const digest: WeekView | null =
    LIVE && weekDigest
      ? {
          done: weekDigest.sorted + weekDigest.trashed,
          auto: weekDigest.autoArchived,
          trashed: weekDigest.trashed,
          insight: weekDigest.insight,
        }
      : mockDigest;

  // 理完庆祝用的本周归档数：口径与周回顾一致（LIVE 用后端全库计量，mock 用已加载的 items 估算）。
  const archivedWeek =
    LIVE && weekDigest
      ? weekDigest.sorted + weekDigest.autoArchived
      : items.filter(
          (f) =>
            f.layer === "archived" &&
            Date.now() - new Date(f.updatedAt).getTime() < 7 * DAY_MS,
        ).length;

  // 「待加工」总数：缓冲区里勾了技能、尚未点跑的条数——独立于其它筛选维度，就是"还有几条等你点技能"。
  const pendingCount = items.filter(
    (f) => f.layer === "buffer" && (pickedSkills[f.id]?.length ?? 0) > 0,
  ).length;

  const filterActive =
    filters.category !== "all" || filters.mediaType !== "all" || filters.status !== "all" || filters.tag !== null || filters.pendingOnly;

  return (
    // 外层相对定位列（居中、限宽、填满高度）承载「回到顶部」浮标；内层才是真正滚动的列。
    // 若把浮标放进滚动列内，absolute 会随内容一起滚走——它必须挂在非滚动的相对父上，
    // 才能像网页 top 按钮那样钉在列的右下角（与详情页 BackToTop 同构）。
    <div className="relative mx-auto h-full w-full max-w-3xl">
      <div ref={scrollRef} className="flex h-full flex-col gap-5 overflow-y-auto px-6 py-6">
      <section className="md-elev rounded-2xl bg-card p-5">
        {/* 待收图片槽：粘贴即见缩略图，可随时撤；点「收进来」才落库（先给用户写附言的机会）。 */}
        {pendingImg && (
          <div className="caption-in mb-2 flex items-center gap-2.5 rounded-lg bg-surface-high/40 p-2">
            <img
              src={pendingImg.data}
              alt="待收藏图片的预览"
              className="h-14 w-14 shrink-0 rounded-md object-cover"
            />
            <div className="min-w-0 flex-1 text-xs">
              <p className="truncate font-medium text-on-surface">
                图片 · {formatBytes(pendingImg.bytes)}
                {pendingImg.compressed && " · 已压缩（超过 5MB）"}
              </p>
              <p className="mt-0.5 text-hint">原样收藏，不经模型；不写附言的话，日后只能按时间找它。</p>
            </div>
            <button
              className="md-press shrink-0 rounded-full p-1.5 text-hint hover:text-on-surface"
              onClick={() => setPendingImg(null)}
              title="不要这张图"
              aria-label="移除待收藏的图片"
            >
              <Icon name="X" size={14} />
            </button>
          </div>
        )}
        <textarea
          ref={taRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onPaste={onPasteRich}
          onKeyDown={(e) => {
            if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
              e.preventDefault();
              send();
            }
            if (e.key === "@" && handOffAt(e.currentTarget)) {
              e.preventDefault();
              caretPending.current = true;
              setIntent((v) => (v && !/\s$/.test(v) ? `${v} @` : `${v}@`));
            }
          }}
          rows={3}
          placeholder="粘贴文本、链接或图片，或直接输入想法…"
          className="w-full resize-none overflow-y-auto bg-transparent text-sm leading-relaxed outline-none placeholder:text-hint"
        />

        {/* 动作区（改动1 去「框中框」）：不再套 bg-surface-high/40 子面板——那层 tint 在亮色几乎不可见
            （你此前已点名 surface 浅底无效），既不是清楚容器也不是干净留白。改由上方一条 border-t 分隔线
            把「输入」与「动作」分开（两主题都成立），控件直接坐在卡片留白里。 */}
        <div className="relative mt-2.5 border-t border-outline-variant/60 pt-2.5">
          {/* 附言 + 收进来（改动 A 续：数据仍与正文分离；视觉从"下划线"改为一枚 filled 色块）。
              下划线在浅/深两态都太弱、看不出是输入位（你 LIVE 反馈"没有变化"），改成无边框的填充色块：
              bg-surface-high（浅 239/深 40，满不透明——当初 bg-surface-high/40 在亮色近乎隐形才去掉那层 tint，
              这次是明确要一个可输入的槽，故用满色而非 40%）。无描边、圆角，读作"卡片上的书写槽"而非第二个描边框。
              聚焦不改底色（暗色里加深方向易刺眼），只加一圈极淡主色 ring。标签与输入统一 text-sm，消除字号不一致。
              intent 仍只作 note + @技能载体，绝不并进 content（@token 污染 content_hash 判重与 FTS，见 send 注释）。 */}
          <div className="flex items-center gap-2">
            <div className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg bg-surface-high px-3 py-1.5 transition-shadow ease-emph focus-within:ring-1 focus-within:ring-primary/40">
              <span className="shrink-0 text-sm text-hint">附言</span>
              <input
                ref={intentRef}
                value={intent}
                onChange={(e) => setIntent(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && suggestions.length) {
                    e.preventDefault();
                    pickSkill(suggestions[0]);
                  }
                }}
                placeholder="给这条碎片补一句，或输入 @ 指定再加工技能"
                className="min-w-0 flex-1 bg-transparent text-sm text-on-surface outline-none placeholder:text-hint"
              />
            </div>
            <Button onClick={send} icon="Inbox" className="shrink-0 px-4">
              收进来
            </Button>
          </div>

          {/* 技能入口（改动3-b 渐进披露）：默认只一枚「＋技能」按钮，不占黄金位；点开才铺全部
              可手动触发技能 chip + 「仅摘要」（清空）。技能是采集后的可选预标（C-1：这里选了不触发，
              只标「待加工」），故层级低于「收进来」。已选时按钮直接显示技能名，收起态也看得清当前选择。
              无可手动触发技能时整行不渲染——否则点开只剩一个无意义的「仅摘要」。 */}
          {pickable.length > 0 && (
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <button
              onClick={() => setSkillsOpen((v) => !v)}
              aria-expanded={skillsOpen}
              title={picked ? `已选再加工技能：${picked.name}（点击展开可改）` : "附加再加工技能（可选）"}
              className={`md-press inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs ${
                picked
                  ? "bg-primary/15 font-medium text-primary"
                  : "text-hint hover:bg-surface-high hover:text-on-surface"
              }`}
            >
              <Icon name="Sparkles" size={12} />
              {picked ? picked.name : "＋ 技能"}
              <Icon name="ChevronDown" size={12} className={skillsOpen ? "rotate-180" : ""} />
            </button>
            {skillsOpen && (
              <>
                <button
                  onClick={() => {
                    setPicked(null);
                    setSkillsOpen(false);
                  }}
                  aria-pressed={picked === null}
                  title="不加附加技能：正文按默认流水线自动摘要"
                  className={`md-press inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs ${
                    picked === null
                      ? "bg-primary/15 font-medium text-primary"
                      : "text-hint hover:bg-surface-high hover:text-on-surface"
                  }`}
                >
                  <Icon name="AlignLeft" size={12} /> 仅摘要
                </button>
                {pickable.map((s) => {
                  const on = picked?.id === s.id;
                  return (
                    <button
                      key={s.id}
                      onClick={() => pickSkill(s)}
                      aria-pressed={on}
                      title={`附加再加工技能：${s.name}`}
                      className={`md-press inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs ${
                        on
                          ? "bg-primary/15 font-medium text-primary"
                          : "text-hint hover:bg-surface-high hover:text-on-surface"
                      }`}
                    >
                      <Icon name="Sparkles" size={12} /> {s.name}
                    </button>
                  );
                })}
              </>
            )}
          </div>
          )}

          {openList.length > 0 && (
            <ul className="caption-in absolute left-2 right-2 top-full z-20 mt-1 max-h-60 origin-top overflow-y-auto rounded-lg border border-outline-variant bg-card py-1 text-sm shadow-e2">
              {openList.map((s) => {
                const on = picked?.id === s.id;
                return (
                  <li key={s.id}>
                    <button
                      onClick={() => pickSkill(s)}
                      className="flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left transition-colors ease-emph hover:bg-surface-high"
                    >
                      <span className="flex min-w-0 items-center gap-1.5">
                        <Icon
                          name={on ? "Check" : "Sparkles"}
                          size={13}
                          className={on ? "text-primary" : "text-hint"}
                        />
                        <span className={`truncate ${on ? "text-primary" : "text-on-surface"}`}>
                          {s.name}
                        </span>
                      </span>
                      <span className="text-xs text-hint">
                        {s.trigger === "at" ? "@指令" : "快捷"}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}

        </div>

        {/* 行内回执条：收进来后的成功/失败就地被动提示（不弹右下角窗）。
            只在真有回执时占 h-5，空态不留固定高度——否则卡片底部比顶部多一截，上下 padding 不齐。 */}
        <div className={submitMsg ? "h-5" : ""} aria-live="polite">
          {submitMsg && (
            <p
              key={submitMsg.text}
              className={`caption-in inline-flex max-w-full items-center gap-1 truncate text-xs ${
                submitMsg.kind === "ok"
                  ? "text-success"
                  : submitMsg.kind === "warn"
                    ? "text-on-surface-variant"
                    : "text-error"
              }`}
            >
              <Icon
                name={
                  submitMsg.kind === "err"
                    ? "TriangleAlert"
                    : submitMsg.kind === "warn"
                      ? "Info"
                      : "Check"
                }
                size={13}
              />
              {submitMsg.text}
            </p>
          )}
        </div>
      </section>

      {/* 收集框下方一行：左侧「Tips」轮换提示（可关闭，关闭后不再出现，见 readDismissedTips），
          右侧无 key/在途状态徽标。二者语义不同故分开渲染——徽标是诚实状态（P0 文案即契约），
          不随 Tips 关闭而消失。-mt-2 抵掉父级 gap，把整行拉近收集框、归属清晰。 */}
      <div className="-mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 px-1 text-xs text-hint">
        {tips && (
          <span className="inline-flex items-center gap-1">
            <span className="font-semibold text-on-surface-variant">Tips：</span>
            <span>{tips.text}</span>
            <button
              type="button"
              onClick={() => dismissTip(tips.key)}
              aria-label="关闭此提示"
              title="关闭后不再显示"
              className="md-press -mr-1 inline-flex cursor-pointer items-center rounded-full p-0.5 hover:bg-surface-high hover:text-on-surface"
            >
              <Icon name="X" size={12} />
            </button>
          </span>
        )}
        {llmReady === false ? (
          // 批次28 §5：无 key/未配模型时 worker 永不领任务，pending 不是"在途"而是"不会来的加工"——
          // 不发脉冲忙态，改一条静态琥珀提示把真实后果说清（文案即契约，P0 诚实）。
          // 总开关关闭是另一种"不会来"：配置还在、只是被用户关了，文案须与"未配置"区分。
          <span className="inline-flex items-center gap-1 rounded-full bg-warn-container px-2 py-0.5 font-medium text-on-warn-container">
            <Icon name="Info" size={12} />
            {llmDisabled
              ? "大模型已关闭：内容原样保存，不会自动加工"
              : "未配置模型：内容原样保存，不会自动加工"}
          </span>
        ) : (
          inflight > 0 && (
            <span className="inline-flex items-center gap-1 rounded-full bg-info-container px-2 py-0.5 font-medium text-on-info-container">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-info" />
              后台加工中 {inflight}
            </span>
          )
        )}
      </div>

      {/* 碎片区动作 + 本周回顾：**一个模块、主次两栏**（LIVE 反馈：原两浮块中间空挡过大、
          「碎片区 N 条」重复呈现；上一版把两段文案并排又成了无层级的文字堆——长句最响、主操作反而最弱）。
          重排原则（M3 + 桌面 UX「最重要的东西最显眼」）：
          · 右栏=本页主操作，整块唯一的饱和色（实心「开始整理」）+ 其作用对象的待整理数（中性粗体，
            眼动落点先看到数字再看到动词）；
          · 左栏=本周回顾，降为次要：小字、柔化色、✨ 起头，剥掉与右栏同源的待整理尾句（见 recapLine），
            只留过去一周的回望，与右栏的「此刻待办」在时间与语义上分开；
          · A 批：整块 bg-card+md-elev 浮起、不描边；窄态 flex-wrap 让两栏各自折行不互相挤压。 */}
      <section className="md-elev flex flex-wrap items-center justify-between gap-x-8 gap-y-3 rounded-2xl bg-card px-5 py-3.5">
        <p className="flex min-w-0 flex-1 items-center gap-2 text-xs leading-relaxed text-hint">
          {triage ? (
            <span>整理模式 · 逐条放行或丢弃，理完自动退出</span>
          ) : digest ? (
            <>
              <Icon name="Sparkles" size={13} className="shrink-0 text-primary/70" />
              <span>{recapLine(digest.insight) || "本周还没有分拣动作。"}</span>
            </>
          ) : (
            <span>本周回顾加载中…</span>
          )}
        </p>
        {triageCount > 0 || triage ? (
          <div className="flex shrink-0 items-center gap-3">
            <span className="whitespace-nowrap">
              <span className="text-lg font-semibold tabular-nums text-on-surface">{triageCount}</span>
              <span className="ml-1 text-xs text-hint">条待整理</span>
            </span>
            <Button
              variant={triage ? "primary" : "filled"}
              size="sm"
              onClick={() => {
                // 进入整理模式即清空多维筛选：否则列表被筛选裁窄，「N 条待整理」会大于可见条目，骗人。
                if (!triage) resetFilters();
                setTriage((v) => !v);
              }}
              title={triage ? "退出整理，恢复显示全部条目" : "只显示碎片区条目，逐条整理"}
            >
              {triage ? "退出整理" : "开始整理"}
            </Button>
          </div>
        ) : (
          <span className="inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full bg-success-container px-3 py-1 text-xs font-medium text-on-success-container">
            <Icon name="Check" size={13} strokeWidth={2.5} />
            已全部整理
          </span>
        )}
      </section>


      {/* 理完庆祝：一次性轻反馈（自动淡出），替代弹窗 */}
      {celebrate && (
        <div
          className="celebrate-in flex items-center gap-2 rounded-xl border border-success/30 bg-success-container px-4 py-2.5 text-sm font-medium text-on-success-container"
          aria-live="polite"
        >
          <Icon name="Check" size={15} strokeWidth={2.5} />
          <span>全部理完了 · 本周已替你归档 {archivedWeek} 条</span>
        </div>
      )}

      {/* 三维筛选：类型 × 分类 × 状态 */}
      <section className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="w-10 shrink-0 text-xs text-hint">类型</span>
          <Chip
            label="全部"
            active={filters.mediaType === "all"}
            onClick={() => setFilters({ mediaType: "all" })}
          />
          {MEDIA_TYPES.map((m) => (
            <Chip
              key={m}
              label={MEDIA_TYPE_LABEL[m]}
              active={filters.mediaType === m}
              onClick={() =>
                setFilters({ mediaType: filters.mediaType === m ? "all" : m })
              }
            />
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="w-10 shrink-0 text-xs text-hint">分类</span>
          <Chip
            label="全部"
            active={filters.category === "all"}
            onClick={() => setFilters({ category: "all" })}
          />
          {CATEGORIES.map((c) => (
            <Chip
              key={c}
              label={c}
              active={filters.category === c}
              onClick={() =>
                setFilters({ category: filters.category === c ? "all" : c })
              }
            />
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="w-10 shrink-0 text-xs text-hint">状态</span>
          <Chip
            label="全部"
            active={filters.status === "all"}
            onClick={() => setFilters({ status: "all" })}
          />
          {BUFFER_STATUSES.map((st) => (
            <Chip
              key={st}
              label={STATUS_LABEL[st]}
              active={filters.status === st}
              onClick={() =>
                setFilters({ status: filters.status === st ? "all" : st })
              }
            />
          ))}
        </div>
        {/* 第四维只在"真有待加工"或已开启时出现（常态不占位，与状态维同理）：一键把挂技能的条目捞到眼前。 */}
        {(pendingCount > 0 || filters.pendingOnly) && (
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="w-10 shrink-0 text-xs text-hint">待办</span>
            <Chip
              label={`只看待加工 ${pendingCount}`}
              active={filters.pendingOnly}
              onClick={() => setFilters({ pendingOnly: !filters.pendingOnly })}
            />
          </div>
        )}
        <div className="flex items-center gap-2 text-xs text-hint">
          <span>共 {buffer.length} 条</span>
          {buffer.length > 1 && !sel.mode && (
            <span className="text-on-surface-variant">J/K 移动 · Enter 打开 · Space 勾选 · Del 丢弃</span>
          )}
          {filters.tag && (
            <Chip
              label={`标签：${filters.tag} ×`}
              className="bg-primary/10 text-primary"
              onClick={() => setFilters({ tag: null })}
            />
          )}
          {filterActive && (
            <button
              className="text-xs text-hint underline-offset-2 hover:underline"
              onClick={resetFilters}
            >
              清除筛选
            </button>
          )}
          {buffer.length > 0 && (
            <div className="ml-auto flex items-center gap-2">
              {sel.mode && (
                <button
                  className="text-xs text-hint underline-offset-2 hover:underline"
                  onClick={() => sel.selectAll(buffer.map((f) => f.id))}
                  title="选中当前筛选结果的全部"
                >
                  全选 {buffer.length}
                </button>
              )}
              <button
                className={`text-xs underline-offset-2 hover:underline ${
                  sel.mode ? "font-medium text-primary" : "text-hint"
                }`}
                onClick={() => (sel.mode ? sel.exit() : sel.setMode(true))}
                title={sel.mode ? "退出多选" : "点选多条后批量放行/丢弃（Space 也可直接勾选）"}
              >
                {sel.mode ? "取消多选" : "多选"}
              </button>
            </div>
          )}
        </div>
      </section>

      {/* 竖向日期时间线：切换整理模式时整段交叉淡入，避免列表"跳"一下 */}
      <section key={triage ? "triage" : "all"} className="caption-in flex flex-col gap-5 pb-6">
        {groups.length === 0 && (
          <p className="py-16 text-center text-sm text-hint">
            {loading
              ? "正在载入…"
              : triage
              ? "碎片区没有条目"
              : filters.pendingOnly
                ? "没有待加工的条目——采集时勾的技能都已点跑，或本次会话没勾过"
                : filters.status !== "all"
                  ? `碎片区没有「${STATUS_LABEL[filters.status]}」的条目`
                  : "碎片区没有条目"}
          </p>
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
              <BufferRow
                key={f.id}
                f={f}
                dim={ageDim(f.createdAt)}
                fresh={f.id === justId}
                leaving={leaving.has(f.id)}
                selectable={sel.mode}
                selected={sel.has(f.id)}
                active={f.id === activeId}
                cursor={f.id === cursorId}
                onToggle={sel.toggle}
                onOpen={onOpen}
                withLeave={withLeave}
                archive={archive}
                trash={trash}
              />
            ))}
          </div>
        ))}
      </section>

      {sel.mode && sel.count > 0 && (
        <BatchBar
          count={sel.count}
          onClear={sel.clear}
          actions={[
            { key: "archive", label: "全部放行", onClick: () => runBatch("archived") },
            { key: "trash", label: "全部丢弃", danger: true, onClick: () => setBatchDiscard(true) },
          ]}
        />
      )}

      {/* 键盘 Del 的单条丢弃：鼠标点「丢弃」不弹确认（可恢复、动作定向），键盘误触成本高，多一道闸。 */}
      <ConfirmDialog
        open={delId !== null}
        title="丢弃这一条？"
        desc="移入回收站，30 天后才彻底清除；可在回收站恢复到碎片区。"
        confirmText="丢弃"
        onCancel={() => setDelId(null)}
        onConfirm={() => {
          const id = delId;
          setDelId(null);
          if (id) withLeave(id, trash);
        }}
      />

      <ConfirmDialog
        open={batchDiscard}
        title={`丢弃选中的 ${sel.count} 条？`}
        desc="移入垃圾站，30 天后才彻底清除；可随时在回收站恢复。"
        confirmText="全部丢弃"
        onCancel={() => setBatchDiscard(false)}
        onConfirm={() => {
          setBatchDiscard(false);
          runBatch("trash");
        }}
      />
      </div>

      <BackToTop scrollRef={scrollRef} />
    </div>
  );
}

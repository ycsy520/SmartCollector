// UI 阶段 mock 数据层（用户指示：UI 先行、不接功能）。
// P7 接真实 IPC 时：stores 内的 action 换为调用 lib/invoke.ts，本文件删除。
import { putMockImage } from "./media";
import type {
  Category,
  ConfigDTO,
  ExtractedLink,
  FragmentDetail,
  FragmentFlag,
  FragmentSource,
  FragmentSummary,
  HabitRule,
  LlmTestResult,
  Matched,
  SearchHit,
  SearchMode,
  Skill,
  SkillTrigger,
  UsageBucket,
  UsageStats,
} from "../types/ipc";

// Skill/SkillTrigger 的权威定义已入 02 §7.2（types/ipc.ts 单源），此处再导出供既有引用点无缝迁移。
export type { Skill, SkillTrigger };

const at = (minAgo: number) =>
  new Date(Date.now() - minAgo * 60_000).toISOString();

export const uid = () => crypto.randomUUID();

// —— 假分析：模拟 pipeline 产出的字段（抽取式，够 UI 演示即可） ——
const LINK_RE = /https?:\/\/[^\s<>"')）\]]+/g;

export function fakeLinks(content: string): ExtractedLink[] {
  const urls = [...new Set(content.match(LINK_RE) ?? [])].slice(0, 20);
  return urls.map((url) => ({ url, text: undefined }));
}

function fakeTags(content: string): string[] {
  const tokens = content
    .split(/[\s，。；、！？.,;:!?()（）「」“”#】]+/)
    .filter((t) => t.length >= 2 && t.length <= 8 && !/^https?:/.test(t));
  const freq = new Map<string, number>();
  for (const t of tokens) freq.set(t, (freq.get(t) ?? 0) + 1);
  return [...freq.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 4)
    .map(([t]) => t);
}

function fakeCategory(content: string): Category {
  if (/rust|sql|代码|接口|api|前端|数据库|性能/i.test(content)) return "技术";
  if (/需求|产品|设计|增长/i.test(content)) return "产品";
  if (/融资|市场|营收|财经/i.test(content)) return "商业";
  if (/学习|方法论|读书|考试/i.test(content)) return "学习";
  if (/健康|美食|旅行|家居/i.test(content)) return "生活";
  if (/新闻|动态|发布/i.test(content)) return "资讯";
  if (/灵感|文案|起名|创意/i.test(content)) return "创意";
  return "其他";
}

function fakeSummary(content: string): string {
  const s = content.replace(LINK_RE, " ").replace(/\s+/g, " ").trim();
  // 与后端 task_summarize::MIN_SUMMARIZE_CHARS 同口径：短内容不产摘要（否则只会得到一遍复读）。
  // 空串在详情页渲染成"未生成摘要"，不是失败。
  if (s.length < 100) return "";
  const cut = s.slice(0, 88);
  const p = cut.lastIndexOf(" ");
  return (p > 60 ? cut.slice(0, p) : cut) + "…";
}

// 抽取式假标题：在句读/词边界截断，避免硬切半个 URL 或词
export function autoTitle(content: string): string {
  const line = content.split(/[。\n！!？?]/)[0].trim() || content.trim();
  if (line.length <= 24) return line;
  const cut = line.slice(0, 24);
  const p = cut.lastIndexOf(" ");
  return (p > 12 ? cut.slice(0, p) : cut) + "…";
}

export function fakeResult(
  content: string,
  opts: { degraded?: boolean; model?: string } = {},
) {
  return {
    category: fakeCategory(content),
    subcategory: undefined,
    tags: fakeTags(content),
    summary: fakeSummary(content),
    links: fakeLinks(content),
    degraded: opts.degraded ?? false,
    modelUsed: opts.model ?? "qwen3-max",
    processedAt: at(0),
  };
}

export function toSummary(f: FragmentDetail): FragmentSummary {
  return {
    id: f.id,
    title: f.title,
    excerpt: f.content.slice(0, 120),
    status: f.status,
    category: f.result?.category,
    tags: f.result?.tags ?? [],
    createdAt: f.createdAt,
    layer: f.layer,
    mediaType: f.mediaType,
    archivedBy: f.archivedBy,
    mediaPath: f.mediaPath,
    flags: f.flags,
  };
}

// —— 种子数据：覆盖 3 层 × 4 状态 × 3 来源 × 类型 × 降级/超时归档样本 ——
function seed(
  minAgo: number,
  source: FragmentSource,
  content: string,
  status: FragmentDetail["status"],
  extra: Partial<FragmentDetail> & { degraded?: boolean } = {},
): FragmentDetail {
  const base: FragmentDetail = {
    id: uid(),
    content,
    source,
    status,
    createdAt: at(minAgo),
    updatedAt: at(Math.max(0, minAgo - 3)),
    title: autoTitle(content),
    layer: "buffer",
    mediaType: source === "link" ? "link" : "text",
    reviewed: false,
    result:
      status === "done"
        ? { ...fakeResult(content, { degraded: extra.degraded }), processedAt: at(Math.max(0, minAgo - 3)) }
        : undefined,
  };
  const { degraded: _d, ...rest } = extra;
  return { ...base, ...rest };
}

const ARCH = { layer: "archived", reviewed: true, archivedBy: "manual" } as const;
const AUTO = { layer: "archived", reviewed: false, archivedBy: "auto" } as const;

export const seedFragments: FragmentDetail[] = [
  // —— 缓冲区：待人工分拣（含今日与昨日两组日期）——
  seed(2, "manual",
    "sqlite-vec 让向量检索直接跑在 SQLite 里，不用额外起服务。rowid 与主表对齐后，混合检索=FTS 一路 + 向量一路，RRF 融合排序即可。",
    "running"),
  seed(8, "clipboard",
    "会议记录：周一评审知识库 MVP 范围。结论：v1.0 砍掉悬浮球与图谱可视化，保收集-处理-检索三条链路；剪贴板不做后台监听，只收用户主动提交的内容。",
    "done"),
  // 批次21-B 演示：一条真带文件的图片片段。浏览器预览没有 media 目录，字节由 putMockImage
  // 存在内存 Map 里（刷新即失），只为让缩略图/详情这块样式看得见。
  seed(30, "clipboard",
    "截图 · 竞品增长曲线：Q3 DAU 持续爬坡，注意 9 月的拐点，疑似新功能拉动。",
    "skipped", { mediaType: "image", mediaPath: "demo-growth.png" }),
  seed(47, "manual",
    "灵感：收集类工具的差异化不在存储而在『零摩擦入口』——粘贴即走，剩下的交给后台。所有需要用户先做分类决策的产品都会死于决策疲劳。",
    "done", { flags: [{ kind: "verify-fail", message: "含未证实断言（「都会死于决策疲劳」）", action: "核实真伪" }] }),
  // 批次6-①演示：人工改过正文 → 结果过期（stale）。详情页原文下方可见修订账，点「重新处理」按新正文对齐。
  seed(20, "manual",
    "混合检索的正确姿势：先各取 top-N（FTS 一路 + 向量一路），再用 RRF 融合排序，别在原始分数上直接相加——两路的分值不可比。",
    "done", {
      editLog: [
        { at: at(20), field: "content", beforeChars: 52, afterChars: 58, excerpt: "“混合检索的做法：先各取 top-N…」" },
        { at: at(18), field: "note", beforeChars: 0, afterChars: 6 },
      ],
      flags: [{ kind: "stale", message: "内容已修订 · AI 结果与向量描述的是改前文本", action: "点「重新处理」按新正文重整理（会发送给模型）" }],
    }),
  seed(70, "clipboard",
    "trigram tokenizer 在 SQLite ≥3.34 可用，对中文子串检索效果好于 unicode61，但索引体积增大约 30%，BM25 权重需要重新调。",
    "failed", { flags: [{ kind: "dup", message: "与「sqlite-vec…」高度相似（约 88%）", action: "对照合并" }] }),
  seed(90, "clipboard",
    "✅✅✅ 😂😂 😷",
    "skipped", { flags: [{ kind: "junk", message: "系统判定为低价值信息，已跳过 AI 整理", action: "仍要保留可放行归档，或点「重新处理」强制整理" }] }),
  // 隐私命中演示：本地规则检出手机号/口令 → 不发送模型（02 §1.1、§7.1 sensitive）
  seed(91, "manual",
    "备用机 13800138000，登录密码：Xk9#tR2mLq",
    "skipped", { flags: [{ kind: "sensitive", message: "检测到手机号等隐私信息，已阻止发送给模型", action: "请留意保密；确认无需保留可在回收站「彻底删除」即刻清除" }] }),
  // 批次9 自查补的夹具：**处理之后**才编辑进隐私（后端 set_content 不重新过 triage，状态仍是 done）。
  // 这条是"再加工 chip 会把正文外发"唯一的真实漏口形态，用来验 chip 也走二次确认（02 §7.1 误报出口）。
  seed(91, "manual",
    "备用机清单：营业厅预约要带的号码 13800138000，另外这台设备的解锁口令是 Xk9#tR2mLq，别告诉别人，放好后记得把这条彻底删除。",
    "done", {
      editLog: [{ at: at(1), field: "content", beforeChars: 46, afterChars: 62, excerpt: "（含隐私，摘录已省略）" }],
      flags: [{ kind: "sensitive", message: "检测到手机号等隐私信息，未经 AI 处理的部分保持原样", action: "请留意保密；确认无需保留可在回收站「彻底删除」即刻清除" }],
    }),
  seed(1500, "manual",
    "洪武悬丝的 UI 反馈：维度标签要用古代品评用语，词性统一为名词，不用现代白话。",
    "done"),
  seed(28800, "clipboard", // 20 天前：超过 14 天未分拣，启动时被定时器自动归档
    "旧笔记：某个未读完成的长文摘录，一直躺在缓冲区没人整理，用来演示 14 天自动归档。",
    "done"),
  // —— 归档层：已放行（manual）或超时替你收（auto，可补审）——
  seed(25, "link",
    "转存：Tauri 2 权限模型改为 capability 白名单，ipc 与 core 拆分，升级时要迁移 capabilities/*.json。https://v2.tauri.app/security/capabilities/",
    "done", { ...ARCH, externalUrl: "https://v2.tauri.app/security/capabilities/", flags: [{ kind: "conflict", message: "与另一条转存对 v2 拆分边界的描述不一致", action: "查看冲突项" }] }),
  seed(140, "link",
    "Rust 异步 trait  Stabilized：async fn in trait 与 dyn 兼容终于落地，Arc<dyn Service> 的异步对象不再有天花板。https://blog.rust-lang.org/2023-12-21/async-fn-rpit-in-traits.html",
    "done", { ...ARCH, externalUrl: "https://blog.rust-lang.org/2023-12-21/async-fn-rpit-in-traits.html" }),
  seed(95, "manual",
    "读《观念的水位》摘录：健康的社会需要安全阀，情绪有出口，事实才被看见。",
    "done", { ...AUTO, degraded: true }),
  seed(260, "manual",
    "问 AI 要方案时，先让它列出不自信点；不列的，多半在硬撑。",
    "done", ARCH),
  seed(320, "link",
    "OpenAI text-embedding-3-small 评测：1024 维默认，成本约为 v2 的 1/5，MTEB 中文略弱于 bge-m3。",
    "done", { ...AUTO }),
  seed(330, "manual",
    "混合检索落地：FTS5 一路 + 向量一路用 RRF 融合排序，向量直接用 sqlite-vec 存在 SQLite 里，省掉单独服务；embedding 先用 text-embedding-3-small，1024 维够用。",
    "done", ARCH),
  seed(400, "clipboard",
    "TODO：给知识库加导出（JSON/Markdown），数据必须在本地手里。",
    "done", ARCH),
  // —— 垃圾站：明确丢弃，30 天倒计时 ——
  seed(1600, "clipboard",
    "某电商双十一预售节奏梳理：满减玩法再度前移，定金膨胀规则调整。",
    "done", { layer: "trash", trashedAt: at(120) }),
  seed(1700, "clipboard",
    "「免费领」课程推广：限时三天扫码进群，名额仅剩 12 个。",
    "done", { layer: "trash", trashedAt: at(40) }),
];

// 交叉引用：dup/conflict 回填对照片段 id（种子构造后统一挂接）。
{
  const find = (kw: string) => seedFragments.find((f) => f.content.includes(kw));
  const dupTarget = find("sqlite-vec")?.id;
  const conflictTarget = find("Rust 异步 trait")?.id;
  for (const f of seedFragments)
    for (const fl of f.flags ?? []) {
      if (fl.kind === "dup") fl.refId = dupTarget;
      if (fl.kind === "conflict") fl.refId = conflictTarget;
    }
}

// ============================================================
// 健壮性测试数据（临时·用户指示：对当前 UI 造常规/非常规/异常数据做压力测试）
// 覆盖：超长无空格串、超长 URL、HTML/XSS 串、零宽/BOM、RTL 双向覆盖、
// zalgo 组合音标、大量换行/制表/回车、emoji 组合、40 标签、四类 flag 齐发、
// 空/纯白内容、无效/未来/epoch 日期、超大内容。验证完可整块删除本段 push。
// ============================================================
function stress(
  p: Partial<FragmentDetail> & { content: string },
): FragmentDetail {
  const now = new Date().toISOString();
  return {
    id: uid(),
    source: "manual",
    status: "done",
    createdAt: now,
    updatedAt: now,
    title: autoTitle(p.content),
    layer: "buffer",
    mediaType: "text",
    reviewed: false,
    result: {
      category: "其他",
      tags: [],
      // 与其他种子同口径：由正文长度决定有无摘要，否则 270 字的健壮样本也会显示"内容本身即为要点"。
      summary: fakeSummary(p.content),
      links: [],
      degraded: false,
      processedAt: now,
    },
    ...p,
  };
}

const LONG_WORD = "超长无空格连续串".repeat(30); // 270 CJK 无空格
const LONG_URL = "https://example.com/" + "a".repeat(320) + "/尾";
const ZALGO = "崩" + "\u0301\u0302\u0303\u0304".repeat(20) + "溃";
const BIDI = "\u202E逆向覆盖 .krow gnol si txet";
const XSS = "<script>alert('xss')</script> <img src=x onerror=alert(document.cookie)>";
const ZWSP = "\uFEFF\u200B零宽\u2060空格\u00A0测试";
const NEWLINES = "第一行\n\n\n\n\n\n第八行\t制表\r\n回车";
const EMOJI = "\u{1F468}\u200D\u{1F469}\u200D\u{1F467}\u200D\u{1F466} 家庭 \u{1F1E8}\u{1F1F3} 旗 \u{1F3F3}\uFE0F\u200D\u{1F308} 彩虹";
const MANY_TAGS = Array.from({ length: 40 }, (_, i) => `标签${i}`);
const ALL_FLAGS: FragmentFlag[] = [
  { kind: "retrash", message: "你此前丢弃过这条", action: "仍要保留？" },
  { kind: "dup", message: "与多条高度相似", action: "对照合并" },
  { kind: "conflict", message: "与另一条信息冲突", action: "查看冲突项" },
  { kind: "verify-fail", message: "含未证实断言", action: "核实真伪" },
  {
    kind: "sensitive",
    message: "检测到身份证号等隐私信息，已阻止发送给模型",
    action: "请留意保密；确认无需保留可在回收站「彻底删除」即刻清除",
  },
];

seedFragments.push(
  stress({ title: "健壮·超长无空格串", content: LONG_WORD }),
  stress({
    title: "健壮·超长URL",
    content: "看这个链接 " + LONG_URL + " 结尾",
    mediaType: "link",
    externalUrl: LONG_URL,
    result: {
      category: "技术",
      tags: ["链接"],
      summary: "含超长 URL 的摘要。" + LONG_URL,
      links: [{ url: LONG_URL }, ...Array.from({ length: 20 }, (_, i) => ({ url: `https://e.com/${i}?q=${"x".repeat(80)}` }))],
      degraded: false,
      processedAt: new Date().toISOString(),
    },
  }),
  stress({ title: "健壮·HTML注入串", content: XSS }),
  stress({ title: "健壮·零宽BOM", content: ZWSP }),
  stress({ title: "健壮·RTL双向覆盖", content: BIDI }),
  stress({ title: "健壮·zalgo组合音标", content: ZALGO }),
  stress({ title: "健壮·多换行制表", content: NEWLINES }),
  stress({ title: "健壮·emoji组合", content: EMOJI }),
  stress({
    title: "健壮·40标签",
    content: "这条挂了很多标签，用来验证卡片只显前 4 个 +N、详情页标签区不撑破。",
    result: {
      category: "其他",
      tags: MANY_TAGS,
      summary: "标签爆炸样本。",
      links: [],
      degraded: false,
      processedAt: new Date().toISOString(),
    },
  }),
  stress({
    title: "健壮·五类异常齐发",
    content: "同时命中 retrash/dup/conflict/verify-fail/sensitive 五种标识，验证徽标换行不溢出。",
    status: "failed",
    flags: ALL_FLAGS,
    result: undefined,
  }),
  stress({ title: "健壮·空内容", content: "   ", result: undefined, status: "pending" }),
  stress({ title: "健壮·无效日期", content: "createdAt 是非法字符串，验证 timeAgo 不崩。", createdAt: "not-a-date", updatedAt: "garbage" }),
  stress({ title: "健壮·未来日期", content: "createdAt 在未来，验证 timeAgo 负数不崩。", createdAt: new Date(Date.now() + 999 * 86_400_000).toISOString() }),
  stress({ title: "健壮·epoch日期", content: "1970 年的时间戳，验证降级为绝对日期显示。", createdAt: "1970-01-01T00:00:00.000Z", layer: "archived", reviewed: true, archivedBy: "manual" }),
  stress({ title: "健壮·超大内容", content: "超大内容".repeat(3000), result: undefined, status: "running" }),
);

// —— 图片类型压测：当前 UI 图片仅 `▣ 图片` 标签占位、不渲染 <img>（v1.0）。
// 因此这些用例验证的是：图片分类/筛选/图标渲染，以及图片型载荷当文本时——
// 超长无空格 base64 撑不撑破、data:/javascript: 伪协议会不会被当 HTML 执行、坏链是否安全。
const IMG_DATA_URI =
  "截图 data:image/png;base64," + "A".repeat(4000); // 无空格超长串，专测溢出
const IMG_DATA_SVG =
  "内联图 <svg xmlns='http://www.w3.org/2000/svg' onload='alert(1)'><script>alert(document.cookie)</script></svg>";
const IMG_JS = "javascript:alert(document.cookie)#伪装成图片src的伪协议";

seedFragments.push(
  stress({
    title: "健壮·图片-常规URL",
    content: "远程截图一张，当前仅标签不渲染。https://picsum.photos/seed/smart/800/600",
    mediaType: "image",
    result: {
      category: "资讯",
      tags: ["截图", "竞品"],
      summary: "仪表板增长曲线截图（图片占位，不渲染）。",
      links: [{ url: "https://picsum.photos/seed/smart/800/600" }],
      degraded: false,
      processedAt: new Date().toISOString(),
    },
  }),
  stress({
    title: "健壮·图片-坏链404",
    content: "失效图 https://example.com/definitely-404-not-here.png",
    mediaType: "image",
  }),
  stress({
    title: "健壮·图片-超长base64",
    content: IMG_DATA_URI,
    mediaType: "image",
    result: {
      category: "其他",
      tags: [],
      summary: "摘要里也塞一段 base64。" + "B".repeat(600),
      links: [],
      degraded: false,
      processedAt: new Date().toISOString(),
    },
  }),
  stress({
    title: "健壮·图片-dataSVG含script",
    content: IMG_DATA_SVG,
    mediaType: "image",
  }),
  stress({
    title: "健壮·图片-伪协议js",
    content: IMG_JS,
    mediaType: "image",
    externalUrl: IMG_JS, // 即便被当作原文链接 href，也须是惰性文本（详见 LinkList/详情页 href 处理）
  }),
  stress({
    title: "健壮·图片-空格unicode名",
    content: "图 标 (带 空 格 & 特殊#%+字符) 中文.png?w=800&v=2",
    mediaType: "image",
  }),
);

// 批次21-B：上面那条图片种节的字节。浏览器预览无 asset 协议，图只能内联；
// 真实模式由 services/media.rs 落 PNG/JPG/WebP/GIF 文件，这里这张 svg 永远不会进后端。
putMockImage(
  "demo-growth.png",
  "data:image/svg+xml;utf8," +
    encodeURIComponent(
      "<svg xmlns='http://www.w3.org/2000/svg' width='480' height='300' viewBox='0 0 480 300'>" +
        "<rect width='480' height='300' fill='#f5f0eb'/>" +
        "<polyline points='40,240 120,210 200,190 280,120 360,90 440,60' fill='none' stroke='#b3402a' stroke-width='4'/>" +
        "<circle cx='280' cy='120' r='7' fill='#b3402a'/><text x='292' y='112' font-size='18' fill='#5a4a44'>9 月拐点</text>" +
        "</svg>",
    ),
);

// 收集时异常检测（V9）：retrash 命中垃圾站墓碑；dup 命中现存高相似片段。
// conflict/verify-fail 依赖 verify skill × top-k，P5 后接真实逻辑，此处仅种子演示。
const norm = (s: string) => s.replace(/\s+/g, "").trim();

export function detectCollectFlags(
  content: string,
  items: FragmentDetail[],
): FragmentFlag[] {
  const flags: FragmentFlag[] = [];
  const n = norm(content);
  const tomb = items.find(
    (f) => f.layer === "trash" && norm(f.content) === n,
  );
  if (tomb) {
    // 唯一允许"收集即打断"的标识——用户此前明确丢弃过同内容
    flags.push({
      kind: "retrash",
      message: "这条你此前丢弃过",
      action: "仍要保留？",
      refId: tomb.id,
    });
    return flags;
  }
  const qb = bigrams(content);
  let best: { id: string; title?: string; score: number } | undefined;
  for (const f of items) {
    if (f.layer === "trash" || f.content === content) continue;
    const s = simScore(qb, bigrams(f.content));
    if (s >= 0.6 && (!best || s > best.score))
      best = { id: f.id, title: f.title, score: s };
  }
  if (best) {
    flags.push({
      kind: "dup",
      message: `与「${(best.title ?? "片段").slice(0, 12)}…」高度相似（${Math.round(
        best.score * 100,
      )}%）`,
      action: "对照合并",
      refId: best.id,
    });
  }
  return flags;
}

// —— 模拟 pipeline：pending → running → done/failed ——
export const FAILURE_RATE = 0.18;

const timers = new Map<string, number[]>();

export function cancelFakeProcess(id: string) {
  const ts = timers.get(id);
  if (ts) {
    for (const t of ts) clearTimeout(t);
    timers.delete(id);
  }
}

export function scheduleFakeProcess(
  id: string,
  onPatch: (id: string, patch: Partial<FragmentDetail>) => void,
  onSettle: (id: string, ok: boolean) => void,
) {
  cancelFakeProcess(id);
  timers.set(id, [
    setTimeout(() => onPatch(id, { status: "running" }), 900),
    setTimeout(() => {
      timers.delete(id);
      const content = currentContent(id);
      if (content === undefined) return; // 片段已删除，不写回、不弹 toast
      if (Math.random() < FAILURE_RATE) {
        onPatch(id, { status: "failed" });
        onSettle(id, false);
      } else {
        onPatch(id, { status: "done", result: fakeResult(content) });
        onSettle(id, true);
      }
    }, 2400 + Math.random() * 1600),
  ]);
}

// 由 fragments store 注入内容查询，避免 mock 层持有状态单例
let currentContent: (id: string) => string | undefined = () => undefined;
export function bindContentLookup(fn: (id: string) => string | undefined) {
  currentContent = fn;
}

// —— 再加工技能（Skill）：类型见 types/ipc.ts / 02 §7.2；此处仅为 mock 数据。——
export const mockSkills: Skill[] = [
  {
    id: "s-verify",
    name: "验证真伪",
    builtin: true,
    trigger: "at",
    mediaType: "all",
    // 与后端 ensure_builtin 的种子一致（category='all'）：库中 84% 是「其他」，
    // 分类作用域一收窄，详情页的再加工入口在绝大多数片段上就是空的。
    category: "all",
    prompt:
      "你是事实核查助手。阅读 {{原文}}，列出：①核心可核查断言；②每条的支持/反驳证据；③总体可信度（高/中/低）与理由。不确定处一律标注「存疑」，不得编造来源。",
    enabled: true,
  },
  {
    id: "s-extend",
    name: "创意扩展",
    builtin: true,
    trigger: "manual",
    mediaType: "all",
    category: "all",
    prompt:
      "基于 {{原文}} 做发散：给出 5 个延伸方向，每个一句话说明切入点与潜在用途，避免复述原文。",
    enabled: true,
  },
  {
    id: "s-source",
    name: "补全出处",
    builtin: true,
    trigger: "manual",
    mediaType: "all",
    // 与 db/skills.rs::BUILTINS 第三条同口径（批次9-P1，用户裁定 e「接受 AI 推断未经核实标注」）
    category: "all",
    prompt:
      "片段若含诗词、古文、歌词、引文等出处性质的文字，补出其作者/朝代/篇名、所引的完整原文、以及白话大意；半句也要给出全篇。不确定出处时一律写「存疑」并说明理由，禁止编造篇名、作者或原文；开头必须写一行「AI 推断，未经核实」。",
    enabled: true,
  },
  {
    id: "s-tldr",
    name: "三句话摘要",
    builtin: false,
    trigger: "auto",
    mediaType: "link",
    category: "技术",
    prompt: "把 {{原文}} 压缩成三句话：它是什么、为什么重要、我能拿来做什么。",
    enabled: false,
  },
];

// —— 分拣习惯（Habit）：类型见 types/ipc.ts / 02 §7.3；此处仅为 mock 数据。——
export const mockHabits: HabitRule[] = [
  {
    id: "h1",
    pattern: "资讯类 × 链接来源",
    action: "丢弃",
    hits: 7,
    total: 8,
    state: "candidate",
    samples: [
      "某电商双十一预售节奏梳理，满减玩法再度前移……",
      "某厂春季发布会汇总：三款新品、两个订阅服务……",
      "「免费领」课程推广邮件：限时三天，扫码进群……",
    ],
  },
  {
    id: "h2",
    pattern: "含拼团 / 优惠券 / 免费领",
    action: "丢弃",
    hits: 5,
    total: 5,
    state: "candidate",
    samples: [
      "限时拼团：家居收纳三件套 49.9 元……",
      "优惠券提醒：你有一张 5 元运费券今晚过期……",
    ],
  },
  {
    id: "h3",
    pattern: "技术类 × 剪贴板来源 × 短于 50 字",
    action: "快速归档（跳过人工审）",
    hits: 6,
    total: 7,
    state: "candidate",
    samples: [
      "PRAGMA busy_timeout 默认 0，多写必设。",
      "r2d2 的 connection_timeout ≠ SQLite 的 busy_timeout。",
    ],
  },
  {
    id: "h4",
    pattern: "生活类 × 购物链接",
    action: "丢弃",
    hits: 9,
    total: 10,
    state: "active",
    recentAuto: 3,
    samples: ["比价插件推荐帖……", "历史低价查询工具……"],
  },
];

// —— 假配置（对应 03 文档 §4 默认键） ——
export const mockConfig: ConfigDTO = {
  llmPrimary: {
    provider: "openai_compat",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    model: "qwen3-max",
    timeoutS: 60,
    maxTokens: 2048,
  },
  llmFallback: null,
  pasteShortcut: "CommandOrControl+Alt+K",
  pasteShortcutError: null,
  llmEmbedding: {
    provider: "openai_compat",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    model: "text-embedding-v3",
    dim: 1024,
  },
  agentRetry: { maxRetries: 3, backoffBaseMs: 1000, backoffMaxMs: 30000 },
  agentAutoRetry: true,
  vectorBackend: "sqlite_vec",
  llmEnabled: true,
  autostart: false,
  keepOriginalImage: false,
  mediaDirCustom: "",
  // 运行期注入项在浏览器预览里没有真目录：mediaDir 空 → 缩略图走内存 Map（见 lib/media.ts 的 putMockImage）。
  mediaDir: "",
  mediaUsageBytes: 0,
  mediaImageCount: 0,
  apiKeys: { openai_compat: { hasKey: true, last4: "3f7a" } },
};

export function fakeTestLlm(model?: string): Promise<LlmTestResult> {
  const ok = Math.random() > 0.25;
  return new Promise((resolve) =>
    setTimeout(
      () =>
        resolve(
          ok
            ? { ok: true, latencyMs: 420 + Math.round(Math.random() * 600), model: model || mockConfig.llmPrimary.model }
            : { ok: false, errorCode: "E_LLM_TIMEOUT", message: "请求超时（mock）" },
        ),
      600 + Math.random() * 900,
    ),
  );
}

// —— 假搜索（本地对 mock 列表做包含匹配 + 伪相似度） ——
function bigrams(s: string): Set<string> {
  const set = new Set<string>();
  for (let i = 0; i + 1 < s.length; i++) set.add(s.slice(i, i + 2));
  return set;
}

// 查询侧覆盖率：短 query 对长文档比 Dice 更稳，模拟"语义相近但非字面命中"
function simScore(q: Set<string>, doc: Set<string>): number {
  if (!q.size) return 0;
  let inter = 0;
  for (const g of q) if (doc.has(g)) inter++;
  return inter / q.size;
}

// 火花回路：向量近邻的 mock 版——内容 bigram 的 Dice 相似度取 Top-K。
// Dice 对称、惩罚长度失衡，近似 embedding 余弦的手替身。真实版换成向量近邻检索。
export function relatedFragments(
  target: FragmentDetail,
  items: FragmentDetail[],
  k = 4,
): { frag: FragmentDetail; score: number }[] {
  const a = bigrams(target.content);
  if (!a.size) return [];
  return items
    .filter((f) => f.id !== target.id && f.status === "done" && f.layer !== "trash")
    .map((f) => {
      const b = bigrams(f.content);
      let inter = 0;
      for (const g of b) if (a.has(g)) inter++;
      const dice = a.size + b.size ? (2 * inter) / (a.size + b.size) : 0;
      return { frag: f, score: dice };
    })
    .filter((x) => x.score >= 0.15)
    .sort((x, y) => y.score - x.score)
    .slice(0, k);
}

export function fakeSearch(
  items: FragmentDetail[],
  text: string,
  mode: SearchMode,
  limit = 20,
): SearchHit[] {
  const q = text.trim();
  if (!q) return [];
  const cap = Math.min(Math.max(limit, 1), 50);
  const qBigrams = bigrams(q);
  const hits: SearchHit[] = [];
  for (const f of items) {
    if (f.status !== "done") continue;
    const inTitle = (f.title ?? "").includes(q);
    const inContent = f.content.includes(q);
    const inTags = (f.result?.tags ?? []).some((t) => t.includes(q));
    const keyword = inTitle || inContent || inTags;
    const sim = simScore(qBigrams, bigrams(f.content));
    const semantic = sim >= 0.3;
    if (mode === "keyword" && !keyword) continue;
    if (mode === "semantic" && !keyword && !semantic) continue;
    if (!keyword && !semantic) continue;
    const exactScore =
      0.55 + (Number(inTitle) + Number(inContent) + Number(inTags)) * 0.12;
    const score = semantic
      ? Math.max(keyword ? exactScore : 0, 0.3 + sim * 0.6)
      : exactScore;
    const matched: Matched =
      keyword && semantic ? "both" : keyword ? "keyword" : "semantic";
    hits.push({ fragment: toSummary(f), score: Math.min(0.99, score), matched });
  }
  hits.sort((a, b) => b.score - a.score);
  return hits.slice(0, cap);
}

// —— 大模型使用统计（02 §7.5）演示数据 ——
// 真环境读 `llm_calls` 聚合；浏览器演示环境没有库，只能造一份。**按日期做稳定散列**（绝不用
// Math.random）：切范围、重进页面时同一天的数字必须一致，否则演示本身就在制造"统计在乱跳"的观感。
const USAGE_HISTORY_DAYS = 90;
const USAGE_CHAT_MODEL = "qwen3-max";
const USAGE_EMB_MODEL = "text-embedding-v3";

/** mulberry32：把日期串散成 32 位种子，再取确定性序列。 */
function seededStream(key: string) {
  let h = 2166136261;
  for (let i = 0; i < key.length; i++) h = Math.imul(h ^ key.charCodeAt(i), 16777619);
  let s = h >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const localDay = (offsetBack: number) => {
  const d = new Date();
  d.setDate(d.getDate() - offsetBack);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

const blankBucket = (key: string): UsageBucket => ({
  key,
  calls: 0,
  failures: 0,
  tokensIn: 0,
  tokensOut: 0,
  tokensUnknown: 0,
  avgLatencyMs: 0,
});

/** 累加一条"某任务在某天的合计"。桶只存均值，所以延迟按 calls 加权回累。 */
function addTo(bucket: Map<string, UsageBucket>, key: string, calls: number, tin: number, tout: number, failures: number, unknown: number, avgLatency: number) {
  if (calls <= 0) return;
  const b = bucket.get(key) ?? blankBucket(key);
  const prevCalls = b.calls;
  b.calls += calls;
  b.failures += failures;
  b.tokensIn += tin;
  b.tokensOut += tout;
  b.tokensUnknown += unknown;
  b.avgLatencyMs = Math.round((b.avgLatencyMs * prevCalls + avgLatency * calls) / b.calls);
  bucket.set(key, b);
}

export function mockUsageStats(rangeDays: number): UsageStats {
  const span = rangeDays > 0 ? Math.min(rangeDays, USAGE_HISTORY_DAYS) : USAGE_HISTORY_DAYS;
  const days = new Map<string, UsageBucket>();
  const tasks = new Map<string, UsageBucket>();
  const models = new Map<string, UsageBucket>();
  let chatCalls = 0;
  let embeddingCalls = 0;

  for (let back = span - 1; back >= 0; back--) {
    const key = localDay(back);
    const r = seededStream(key);
    const weekend = new Date(`${key}T12:00:00`).getDay() % 6 === 0;
    const frags = Math.max(0, Math.round((1 + r() * 11) * (weekend ? 0.3 : 1)));
    const skills = r() < 0.55 ? 0 : 1 + Math.floor(r() * 2);
    // 该天上游整批没回 usage（有些端点确实这样）——用来演示"合计只是下限"的提示。
    const blind = r() < 0.08;
    const jitter = () => 0.8 + r() * 0.4;

    const rows = [
      { task: "classify", model: USAGE_CHAT_MODEL, n: frags, tin: 780, tout: 34, lat: 820 },
      { task: "tag", model: USAGE_CHAT_MODEL, n: frags, tin: 820, tout: 46, lat: 940 },
      { task: "summarize", model: USAGE_CHAT_MODEL, n: frags, tin: 900, tout: 190, lat: 1750 },
      { task: "extract_links", model: USAGE_CHAT_MODEL, n: frags, tin: 860, tout: 58, lat: 700 },
      { task: "skill_run", model: USAGE_CHAT_MODEL, n: skills, tin: 1400, tout: 420, lat: 2300 },
      { task: "embed", model: USAGE_EMB_MODEL, n: frags, tin: 310, tout: 0, lat: 260 },
    ];

    for (const row of rows) {
      const calls = Math.round(row.n * jitter());
      if (calls === 0) continue;
      const failures = r() < 0.22 ? Math.min(calls, 1 + Math.floor(r() * 2)) : 0;
      const tin = blind ? 0 : Math.round(calls * row.tin * jitter());
      const tout = blind ? 0 : Math.round(calls * row.tout * jitter());
      const unknown = blind ? calls : 0;
      const day = days.get(key) ?? blankBucket(key);
      const prev = day.calls;
      day.calls += calls;
      day.failures += failures;
      day.tokensIn += tin;
      day.tokensOut += tout;
      day.tokensUnknown += unknown;
      day.avgLatencyMs = Math.round((day.avgLatencyMs * prev + row.lat * calls) / day.calls);
      days.set(key, day);
      addTo(tasks, row.task, calls, tin, tout, failures, unknown, row.lat);
      addTo(models, row.model, calls, tin, tout, failures, unknown, row.lat);
      if (row.model === USAGE_CHAT_MODEL) chatCalls += calls;
      else embeddingCalls += calls;
    }
  }

  const sum = (bs: Iterable<UsageBucket>) => {
    let calls = 0, failures = 0, tin = 0, tout = 0, unknown = 0, lat = 0;
    for (const b of bs) {
      calls += b.calls;
      failures += b.failures;
      tin += b.tokensIn;
      tout += b.tokensOut;
      unknown += b.tokensUnknown;
      lat += b.avgLatencyMs * b.calls;
    }
    return { calls, failures, tin, tout, unknown, avg: calls ? Math.round(lat / calls) : 0 };
  };
  const t = sum(days.values());
  const sortDesc = (m: Map<string, UsageBucket>) =>
    [...m.values()].sort((a, b) => b.calls - a.calls || a.key.localeCompare(b.key));
  return {
    rangeDays,
    totalCalls: t.calls,
    totalFailures: t.failures,
    tokensIn: t.tin,
    tokensOut: t.tout,
    tokensUnknown: t.unknown,
    avgLatencyMs: t.avg,
    chatCalls,
    embeddingCalls,
    byDay: [...days.values()].sort((a, b) => a.key.localeCompare(b.key)),
    byTask: sortDesc(tasks),
    byModel: sortDesc(models),
    // 演示库"从未清空过"，所以最早记录固定在 90 天前。
    firstRecordedAt: new Date(`${localDay(USAGE_HISTORY_DAYS - 1)}T09:14:00`).toISOString(),
  };
}

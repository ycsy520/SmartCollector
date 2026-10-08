// 与 docs/arch/02-Tauri-Command清单.md 逐字段镜像的前端契约类型。
// 修改需同步 02 清单与 Rust dto/；时间一律 ISO 8601 UTC 字符串，ID 为 UUID v4。

// 处理状态机（03 §2）。skipped=入队前闸门判定为垃圾/仅记录，永不进 AI 流水线（02 §1.1）。
export type FragmentStatus =
  | "pending"
  | "running"
  | "done"
  | "failed"
  | "skipped";
export type FragmentSource = "manual" | "clipboard" | "link";
// 分拣层：缓冲区 → 归档 / 垃圾站（三层分拣模型，见 02 §2.6 / 03 §3.9）。
export type Layer = "buffer" | "archived" | "trash";
// 信息类型标签（与分类正交的第二维）。image 在 v1.0 仅占位。
export type MediaType = "text" | "link" | "image";
export type Category =
  | "技术"
  | "产品"
  | "商业"
  | "学习"
  | "生活"
  | "资讯"
  | "创意"
  | "其他";

export interface ExtractedLink {
  url: string;
  text?: string;
}

// 收集异常标识（三层分拣模型 V9，见 02 §7.1）。每条自带面向用户的说明与下一步动作。
// 色义固定：灰=知情 / 琥珀=需看一眼；红仅用于破坏性操作，标识本身不着红。
// 真实版产出：retrash=收集时 hash 墓碑比对；dup=相似度比对；conflict/verify-fail=verify skill × top-k；
// junk=入队前启发式闸门判低价值；sensitive=**本地规则**检出隐私信息（零 AI、零网络），该条不发送模型；
// stale=正文在结果生成后被人工修订，摘要/向量描述的是旧文本（读时算，点「重新处理」即消解，02 §2.5）。
export type FragmentFlagKind = "retrash" | "dup" | "conflict" | "verify-fail" | "junk" | "sensitive" | "stale";
export interface FragmentFlag {
  kind: FragmentFlagKind;
  message: string; // 异常说明
  action: string; // 该条自带的下一步动作文案
  refId?: string; // dup/conflict 指向的对照片段 id
}

/// 人工修订账的一条（存 fragments.edit_log，详情页原文下方渲染，02 §2.5 批次6-①）。
/// 含隐私时 excerpt 只写占位词，绝不落明文（与 sensitive flag 同口径）。
export interface EditEntry {
  at: string;
  field: "content" | "note";
  beforeChars: number;
  afterChars: number;
  excerpt?: string;
}

export interface FragmentSummary {
  id: string;
  title?: string;
  excerpt: string; // 前 120 字符
  status: FragmentStatus;
  category?: Category;
  tags: string[];
  createdAt: string;
  layer: Layer;
  mediaType: MediaType;
  archivedBy?: "manual" | "auto"; // 归档来源：manual 主动放行(不标记) / auto 超时替你收(轻角标)
  // 批次21-B：图片文件名（相对 media 目录，后端 media_path）。缩略图靠它拼 asset URL；非图片为 undefined。
  mediaPath?: string;
  flags?: FragmentFlag[]; // 收集异常标识（列表卡片徽标用）
}

export interface FragmentResult {
  category: Category;
  subcategory?: string;
  tags: string[];
  summary: string;
  links: ExtractedLink[];
  degraded: boolean;
  modelUsed?: string;
  processedAt: string;
}

export interface ManualFlags {
  title?: boolean;
  category?: boolean;
  tags?: boolean;
}

export interface FragmentDetail {
  id: string;
  content: string;
  title?: string;
  externalUrl?: string;
  source: FragmentSource;
  status: FragmentStatus;
  createdAt: string;
  updatedAt: string;
  result?: FragmentResult;
  layer: Layer;
  mediaType: MediaType;
  archivedBy?: "manual" | "auto"; // 归档来源；auto=14 天超时替你收，可补审
  reviewed?: boolean; // 是否经人工分拣
  trashedAt?: string; // 进垃圾站时刻（30 天倒计时基准）
  note?: string; // 人工附言，参与检索（02 §2.2 / 03 §3.9）
  editLog?: EditEntry[]; // 人工修订账（正文/笔记改动），详情页原文下方（02 §2.5 批次6-①）
  priorResults?: FragmentResult[]; // 再加工前的历史版本（映射 03 processing_results.version/superseded）
  flags?: FragmentFlag[]; // 收集异常标识，每条自带下一步动作（V9，02 §7.1）
  manual?: ManualFlags; // 人工值优先标记：retry 不覆盖已改字段（02 §2.5）
  // 批次21-B：图片原样收藏——文件在 media 目录，正文只是占位。非图片为 undefined。
  mediaPath?: string;
}

// submit_image 入参（02 §1.4）：data = **裸 base64**（后端解码器只认 base64 字母表，data URL 的
// `data:...;base64,` 前缀由 store 在边界剥掉）；格式一律按魔数判，不信前端报的 MIME。
// note = 人工附言，是图片唯一的可检索描述（除此之外只能按收集时间找）。
export interface ImageInput {
  data: string;
  note?: string;
  /** 批次23：与图**同时**提交的正文。给了就和图合成一条（照常入队加工），不给就是纯图片条目。 */
  content?: string;
}

export interface SubmitInput {
  content: string;
  title?: string;
  note?: string; // 人工附言，参与检索（02 §1.1）
  skillIds?: string[]; // 采集时勾选的再加工技能：后端**不自动跑**（批次9「AI 判断、用户执行」），仅随本次会话标记为「待加工」，由你在详情页逐条点 `run_skill`
}

// —— Command 入参/出参包装（镜像 Rust dto/，供 invoke.ts 类型化）——
export interface ListQuery {
  offset?: number;
  limit?: number;
  layer?: Layer;
  status?: FragmentStatus;
  category?: Category;
  tag?: string;
  source?: FragmentSource;
  mediaType?: MediaType;
  reviewed?: boolean; // 补审队列：false=筛"超时替你收但未人工审过"的归档条目（02 §2.1）
  archivedBy?: "manual" | "auto"; // 归档来源，与 reviewed 配合定位补审队列
}

export interface Page<T> {
  items: T[];
  total: number;
}

export interface SubmitOutcome {
  fragmentId: string;
  status: FragmentStatus;
  /** true=原文已有活跃片段，未新建，id 指向那一条（幂等收集）。 */
  duplicate: boolean;
}

/**
 * `submit_clipboard` 出参（02 §1.2）。后端是 untagged 枚举，两种形状无判别字段，
 * 调用方用 `"skipped" in r` 分支。
 */
export type ClipboardOutcome =
  | SubmitOutcome
  | { skipped: true; reason: "duplicate" | "empty" | "no_text" };

export interface RetryOutcome {
  status: FragmentStatus;
  retryCount: number;
}

export interface DeleteOutcome {
  deleted: boolean;
}

// run_skill 出参（02 §2.7）：新结果版本号。
export interface SkillOutcome {
  newVersion: number;
}

export type Matched = "keyword" | "semantic" | "both";

export interface SearchHit {
  fragment: FragmentSummary;
  score: number;
  matched: Matched;
}

// search_fragments 出参（02 §3.1；degraded=embedding 失败降级为纯 keyword）。
export interface SearchResult {
  items: SearchHit[];
  degraded?: boolean;
}

export type SearchMode = "hybrid" | "keyword" | "semantic";

export interface SearchQuery {
  text: string;
  mode?: SearchMode;
  limit?: number;
}

// —— UI v2 扩展类型（02 §7：再加工技能 / 分拣习惯 / 火花回路）——
export type SkillTrigger = "auto" | "manual" | "at"; // 自动 / 详情页快捷 / 输入框 @指令
export interface Skill {
  id: string;
  name: string;
  builtin: boolean; // 内置：prompt 只读、不可删，仅可 enabled 开关
  trigger: SkillTrigger;
  mediaType: MediaType | "all";
  category: Category | "all";
  prompt: string; // 模板，含 {{原文}} 占位（04 §6）
  enabled: boolean;
}

export type HabitState = "candidate" | "active" | "paused";
export interface HabitRule {
  id: string;
  pattern: string; // 条件摘要（AI 生成的人话）
  action: string; // 丢弃 / 快速归档 / @skill:{id}
  hits: number; // 同类动作次数
  total: number; // 观察样本数
  state: HabitState;
  recentAuto?: number; // active 态：近 7 天自动处理数（周摘要可整批撤销）
  samples: string[]; // 证据样本（片段摘录）
}

export interface RelatedHit {
  fragment: FragmentSummary;
  score: number;
}

/** `list_related` 外层包装（02 §7.4）。 */
export interface RelatedOutcome {
  items: RelatedHit[];
}

export interface WeekDigest {
  sorted: number; // 本周人工归档
  trashed: number; // 本周丢弃
  autoArchived: number; // 本周替你归档（超时 auto）
  pending: number; // 当前碎片区待裁决
  insight: string; // 一句数据推导的导语
}

// —— 大模型使用统计（02 §7.5）：只回计量，不回任何正文；金额不在后端算 ——
export interface UsageBucket {
  key: string; // 本机日历日 '2026-09-29' / 任务名 / 模型名
  calls: number;
  failures: number;
  tokensIn: number;
  tokensOut: number;
  // 本桶里"上游没回 token 数"的调用条数。>0 时合计只是下限，界面必须写明。
  tokensUnknown: number;
  avgLatencyMs: number;
}

export interface UsageStats {
  rangeDays: number; // 0 = 全部历史
  totalCalls: number;
  totalFailures: number;
  tokensIn: number;
  tokensOut: number;
  tokensUnknown: number;
  avgLatencyMs: number;
  chatCalls: number;
  embeddingCalls: number;
  byDay: UsageBucket[];
  byTask: UsageBucket[];
  byModel: UsageBucket[];
  // null = 从未调用过（空态据此说话，不能说"近 N 天无调用"）。
  firstRecordedAt: string | null;
}

// —— 导出（02 §8）：只读聚合，落成 Markdown 文件目录 ——
export interface ExportQuery {
  // 显式 id 优先（前端"看到的即导出的"就靠这条）；空数组视同未指定，落到下面的筛选。
  ids?: string[];
  layer?: Layer | "all"; // 缺省 = archived（与列表默认层一致）
  status?: FragmentStatus;
  category?: Category;
  tag?: string;
  source?: FragmentSource;
  mediaType?: MediaType;
  includeVersions?: boolean; // 默认 false：只导当前版本
}

export interface ExportOutcome {
  dir: string; // 本次导出目录绝对路径
  files: string[]; // 相对 dir 的文件名，_index.md 在末尾
}

// —— 配置（03 文档 §4 config 默认键的前端视图；API Key 永不回传明文，按 provider 存 keyring）——
// provider 为供应商标识：预设 "deepseek" | "qwen" | "openai"，或自定义 "openai_compat"（OpenAI 兼容端点）。
export interface LlmEndpointConfig {
  provider: string;
  baseUrl: string;
  model: string;
  timeoutS: number;
  maxTokens: number;
}

export interface EmbeddingConfig {
  provider: string;
  baseUrl: string;
  model: string;
  dim: number;
}

export interface RetryConfig {
  maxRetries: number;
  backoffBaseMs: number;
  backoffMaxMs: number;
}

export interface SecretInfo {
  hasKey: boolean;
  last4?: string;
}

export interface ConfigDTO {
  llmPrimary: LlmEndpointConfig;
  llmFallback: LlmEndpointConfig | null;
  llmEmbedding: EmbeddingConfig;
  agentRetry: RetryConfig;
  agentAutoRetry: boolean;
  vectorBackend: "sqlite_vec" | "brute";
  // 大模型总开关：false 时 worker 不领任务、技能/指令拒绝、语义检索退关键词。关闭绝不清空已配端点/密钥。
  llmEnabled: boolean;
  autostart: boolean;
  // 「唤起并填草稿」加速键（默认 CommandOrControl+Alt+K，可改）。
  pasteShortcut: string;
  // 键位已存但 OS 注册失败的简述（被别的应用占用等）；正常为 null/undefined。
  pasteShortcutError?: string | null;
  // 按 provider 的密钥状态（仅覆盖当前配置引用的 provider）；永不回传明文。
  apiKeys: Record<string, SecretInfo>;
  // 批次21-B：粘贴图片>5MB 时前端默认重编码压小（有损、不可逆）；打开则原字节直存。
  keepOriginalImage: boolean;
  // 批次22-A：用户**设定**的存放目录（空串=默认 `<app_data_dir>/media`）。与下面的 mediaDir 必须分开：
  // mediaDir 是每次由磁盘现算的生效路径，把它当配置回传保存会把"默认"钉死成一条绝对路径（换机器即失效），
  // 而「恢复默认」这个动作也只有在这个设定值上才表达得出来。
  mediaDirCustom: string;
  // media 目录绝对路径（正斜杠）与占用。运行期由命令层注入，不是库里存的配置。
  mediaDir: string;
  mediaUsageBytes: number;
  mediaImageCount: number;
}

// test_llm_config 目标端点（02 §4.3）：主/备用为 chat，向量为 embedding。默认 primary。
export type LlmTestTarget = "primary" | "fallback" | "embedding";

export interface LlmTestResult {
  ok: boolean;
  latencyMs?: number;
  model?: string;
  errorCode?: string;
  message?: string;
}

// update_config 入参（02 §4.2 Partial<ConfigDTO> 的镜像，各字段可选，仅提供者被合并）。
// secrets.apiKeys：provider→明文密钥（写 keyring）；值为空串=删除该 provider 密钥。
export interface ConfigPatch {
  llmPrimary?: LlmEndpointConfig;
  llmFallback?: LlmEndpointConfig | null;
  llmEmbedding?: EmbeddingConfig;
  agentRetry?: RetryConfig;
  agentAutoRetry?: boolean;
  vectorBackend?: "sqlite_vec" | "brute";
  llmEnabled?: boolean; // 大模型总开关（02 §4.2）：只翻布尔，不动其它端点/密钥
  autostart?: boolean;
  pasteShortcut?: string;
  keepOriginalImage?: boolean; // 批次21-B：图片压缩开关（02 §4.2）
  mediaDirCustom?: string; // 批次22-A：改存放目录（空串=恢复默认）；后端会搬图 + 放行 asset 作用域
  secrets?: { apiKeys?: Record<string, string> };
}

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  ConfigDTO,
  EmbeddingConfig,
  LlmEndpointConfig,
  LlmTestResult,
  LlmTestTarget,
  SecretInfo,
} from "../types/ipc";
import { testLlmConnection, useSettings } from "../stores/settings";
import { Button } from "../components/ui/button";
import { Icon, type IconName } from "../components/ui/icon";
import { Field, Switch, TextInput } from "../components/ui/form";
import { formatBytes } from "../lib/media";
import { isTauri } from "../lib/invoke";
import { AboutSection } from "../components/AboutSection";
import { HelpSection } from "../components/HelpSection";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { UsageSection } from "../components/UsageSection";
import { HabitSection } from "../components/HabitCard";
import { SkillSection } from "../components/SkillCard";
import { toast } from "../components/ui/toast";
import { DEFAULT_PASTE_SHORTCUT, checkShortcutConflict, formatShortcut } from "../lib/shortcut";
import { readTheme, setTheme, type ThemePref } from "../lib/theme";

// 供应商预设：选预设自动填 Base URL / 模型（仍可手改）；"自定义"为 OpenAI 兼容端点，全手填。
// provider 名同时是 keyring 的 account 与密钥 env 名（见 config.rs::api_key_for）。
interface Preset {
  id: string;
  label: string;
  baseUrl: string;
  chatModel?: string;
  embedModel?: string;
  embedDim?: number;
}
const PRESETS: Preset[] = [
  { id: "deepseek", label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", chatModel: "deepseek-chat" },
  {
    id: "qwen",
    label: "通义千问（DashScope）",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    chatModel: "qwen-plus",
    embedModel: "text-embedding-v3",
    embedDim: 1024,
  },
  {
    id: "openai",
    label: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    chatModel: "gpt-4o-mini",
    embedModel: "text-embedding-3-small",
    embedDim: 1536,
  },
  { id: "openai_compat", label: "自定义（OpenAI 兼容）", baseUrl: "" },
];
const presetById = (id: string): Preset =>
  PRESETS.find((p) => p.id === id) ?? PRESETS[PRESETS.length - 1];

// 单端点连通测试：按钮 + 结果摘要（成功显示模型与延迟，失败显示错误码/消息）。
function TestButton({
  testing,
  result,
  onTest,
}: {
  testing: boolean;
  result: LlmTestResult | null;
  onTest: () => void;
}) {
  return (
    <div className="flex items-center gap-2">
      <Button variant="ghost" onClick={onTest} disabled={testing}>
        {testing ? "测试中…" : "测试连接"}
      </Button>
      {result && (
        <span
          className={`inline-flex items-center gap-1 text-xs ${
            result.ok ? "text-success" : "text-error"
          }`}
        >
          <Icon name={result.ok ? "Check" : "TriangleAlert"} size={13} />
          {result.ok
            ? `${result.model} · ${result.latencyMs}ms`
            : `${result.errorCode} ${result.message}`}
        </span>
      )}
    </div>
  );
}

// 主题色：设备级观好，点了即刻生效且只写 localStorage（不进 draft、不进「保存」），
// 所以底部保存按钮的 dirty 与它无关——文案必须说清"仅本机"，别让用户以为存进了配置库。
const THEME_OPTS: { id: ThemePref; icon: IconName; label: string }[] = [
  { id: "system", icon: "Monitor", label: "随系统" },
  { id: "dark", icon: "Moon", label: "深色" },
  { id: "light", icon: "Sun", label: "浅色" },
];

function ThemeSection() {
  const [pref, setPref] = useState<ThemePref>(() => readTheme());
  const pick = (id: ThemePref) => {
    setTheme(id);
    setPref(id);
  };
  return (
    <section className="md-elev rounded-2xl bg-card p-5">
      <h2 className="mb-3 text-xs font-semibold text-hint">外观</h2>
      <div
        role="group"
        aria-label="主题色"
        className="inline-flex gap-1 rounded-full border border-outline-variant/70 bg-surface-high p-1"
      >
        {THEME_OPTS.map((o) => (
          <button
            key={o.id}
            type="button"
            aria-pressed={pref === o.id}
            onClick={() => pick(o.id)}
            className={`md-press inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary/60 ${
              pref === o.id
                ? "bg-secondary-container font-semibold text-on-secondary-container"
                : "text-on-surface-variant hover:bg-surface-highest"
            }`}
          >
            <Icon name={o.icon} size={14} />
            {o.label}
          </button>
        ))}
      </div>
      <p className="mt-3 text-xs leading-relaxed text-hint">
        仅本机生效（不写入配置数据库，也不参与「保存」）。「随系统」跟随 Windows 深浅色设置实时翻转。
      </p>
    </section>
  );
}

function ProviderSelect({
  value,
  onChange,
}: {
  value: string;
  onChange: (p: Preset) => void;
}) {
  return (
    <Field label="供应商">
      <select
        value={PRESETS.some((p) => p.id === value) ? value : "openai_compat"}
        onChange={(e) => onChange(presetById(e.target.value))}
        className="w-full rounded-lg border border-outline bg-card px-3 py-1.5 text-sm text-on-surface outline-none transition-colors ease-emph focus:border-primary focus:ring-1 focus:ring-primary/30"
      >
        {PRESETS.map((p) => (
          <option key={p.id} value={p.id}>
            {p.label}
          </option>
        ))}
      </select>
    </Field>
  );
}

// 单个 provider 的密钥输入：写入即覆盖（保存时经 secrets 落 keyring），清除发空串删除。
function KeyInput({
  provider,
  info,
  value,
  cleared,
  onValue,
  onClear,
}: {
  provider: string;
  info?: SecretInfo;
  value: string;
  cleared: boolean;
  onValue: (v: string) => void;
  onClear: (c: boolean) => void;
}) {
  if (!provider) return null;
  return (
    <Field label={`API Key（${provider}）`}>
      <div className="flex items-center gap-2">
        <TextInput
          type="password"
          value={value}
          placeholder={info?.hasKey ? `已存 **** ${info.last4}，输入以覆盖` : "sk-…"}
          autoComplete="off"
          onChange={(e) => onValue(e.target.value)}
          className="flex-1"
        />
        {info?.hasKey && !value.trim() && !cleared && (
          <button
            type="button"
            className="shrink-0 text-xs text-hint hover:text-error"
            onClick={() => onClear(true)}
          >
            清除
          </button>
        )}
        {cleared && (
          <button
            type="button"
            className="shrink-0 text-xs text-hint hover:text-on-surface"
            onClick={() => onClear(false)}
          >
            撤销
          </button>
        )}
      </div>
      <p className="mt-1 text-xs text-hint">
        {value.trim()
          ? `将保存新密钥（尾号 ${value.trim().slice(-4)}）·「测试连接」会直接用这串试，无需先保存`
          : cleared
            ? "将在保存时删除该 provider 的密钥"
            : "存于系统 keyring，不落库、不回传明文"}
      </p>
    </Field>
  );
}

function EndpointFields({
  title,
  value,
  onChange,
  apiKeys,
  keyDraft,
  cleared,
  onKey,
  onClear,
  testing,
  result,
  onTest,
}: {
  title: string;
  value: LlmEndpointConfig | null;
  onChange: (v: LlmEndpointConfig | null) => void;
  apiKeys: Record<string, SecretInfo>;
  keyDraft: Record<string, string>;
  cleared: Record<string, boolean>;
  onKey: (provider: string, v: string) => void;
  onClear: (provider: string, c: boolean) => void;
  testing: boolean;
  result: LlmTestResult | null;
  onTest: () => void;
}) {
  if (!value)
    return (
      <div className="flex items-center justify-between rounded-lg border border-dashed border-outline-variant p-3">
        <span className="text-sm text-hint">{title}：未配置</span>
        <Button
          variant="ghost"
          onClick={() =>
            onChange({
              provider: "openai_compat",
              baseUrl: "",
              model: "",
              timeoutS: 60,
              maxTokens: 2048,
            })
          }
        >
          启用
        </Button>
      </div>
    );
  const set = (patch: Partial<LlmEndpointConfig>) => onChange({ ...value, ...patch });
  const applyPreset = (p: Preset) =>
    onChange({
      ...value,
      provider: p.id,
      baseUrl: p.baseUrl || value.baseUrl,
      model: p.chatModel || value.model,
    });
  return (
    <fieldset className="md-elev rounded-2xl bg-card p-4">
      <legend className="px-1 text-xs font-semibold text-on-surface-variant">{title}</legend>
      <div className="grid grid-cols-2 gap-3">
        <ProviderSelect value={value.provider} onChange={applyPreset} />
        <Field label="模型名">
          <TextInput
            value={value.model}
            placeholder="deepseek-chat"
            onChange={(e) => set({ model: e.target.value })}
          />
        </Field>
        <Field label="Base URL">
          <TextInput
            value={value.baseUrl}
            placeholder="https://…/v1"
            onChange={(e) => set({ baseUrl: e.target.value })}
          />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="超时（秒）">
            <TextInput
              type="number"
              min={1}
              max={300}
              value={value.timeoutS}
              onChange={(e) => set({ timeoutS: Number(e.target.value) })}
            />
          </Field>
          <Field label="Max Tokens">
            <TextInput
              type="number"
              value={value.maxTokens}
              onChange={(e) => set({ maxTokens: Number(e.target.value) })}
            />
          </Field>
        </div>
      </div>
      <div className="mt-3">
        <KeyInput
          provider={value.provider}
          info={apiKeys[value.provider]}
          value={keyDraft[value.provider] ?? ""}
          cleared={!!cleared[value.provider]}
          onValue={(v) => onKey(value.provider, v)}
          onClear={(c) => onClear(value.provider, c)}
        />
      </div>
      <div className="mt-2 flex items-center justify-between">
        <TestButton testing={testing} result={result} onTest={onTest} />
        <button
          className="text-xs text-hint hover:text-error"
          onClick={() => onChange(null)}
        >
          停用
        </button>
      </div>
    </fieldset>
  );
}

// 客户端校验，镜像 src-tauri/src/config.rs::validate（03 §4 键值约束）；Rust 侧仍是最终防线。
function validateDraft(d: ConfigDTO): string[] {
  const errs: string[] = [];
  const ep = (e: LlmEndpointConfig | null, name: string) => {
    if (!e || (!e.model.trim() && !e.baseUrl.trim())) return; // 未启用视为未配置
    if (!e.model.trim()) errs.push(`${name}：模型名不能为空`);
    if (!/^https?:\/\//.test(e.baseUrl.trim()))
      errs.push(`${name}：Base URL 需以 http(s):// 开头`);
    if (!Number.isFinite(e.timeoutS) || e.timeoutS < 1 || e.timeoutS > 300)
      errs.push(`${name}：超时须为 1–300 秒`);
    if (!Number.isFinite(e.maxTokens) || e.maxTokens < 1)
      errs.push(`${name}：Max Tokens 须为正整数`);
  };
  ep(d.llmPrimary, "主模型");
  ep(d.llmFallback, "备用模型");
  if (d.llmEmbedding.model.trim() || d.llmEmbedding.baseUrl.trim()) {
    if (!/^https?:\/\//.test(d.llmEmbedding.baseUrl.trim()))
      errs.push("向量模型：Base URL 需以 http(s):// 开头");
    if (!Number.isFinite(d.llmEmbedding.dim) || d.llmEmbedding.dim < 1)
      errs.push("向量模型：维度须为正整数");
  }
  const r = d.agentRetry;
  if (!Number.isInteger(r.maxRetries) || r.maxRetries < 0 || r.maxRetries > 5)
    errs.push("最大重试次数须为 0–5 整数");
  if (!Number.isFinite(r.backoffBaseMs) || r.backoffBaseMs < 100)
    errs.push("退避基数须 ≥100ms");
  if (!Number.isFinite(r.backoffMaxMs) || r.backoffMaxMs < r.backoffBaseMs)
    errs.push("退避上限须 ≥ 退避基数");
  // 与后端 validate_accelerator 同口径：先在前端挡住明显不合法的键位，省一次往返。
  const scParts = d.pasteShortcut.split("+").map((p) => p.trim());
  if (scParts.length < 2 || scParts.some((p) => !p))
    errs.push("加速键须为「修饰键+主键」形式，例如 CommandOrControl+Alt+K");
  else {
    const clash = checkShortcutConflict(d.pasteShortcut);
    if (clash) errs.push(`加速键不能与应用内「${clash}」快捷键相同，请换一个键位`);
  }
  return errs;
}

// ============ 设置分区：主导航（App 左侧栏）｜二级导航｜内容列 ============
// 分组、分组认领的字段、搜索索引三张表都是**静态声明**。不去 DOM 抓文案：
// 改一句标签就漏检，而且窄态下未挂载的分组根本不在文档里，抓不到也滚不到。
type GroupId = "appearance" | "llm" | "tools" | "usage" | "system" | "help" | "about";

const GROUPS: { id: GroupId; label: string }[] = [
  { id: "appearance", label: "外观主题" },
  { id: "llm", label: "大模型配置" },
  { id: "tools", label: "信息处理工具配置" },
  { id: "usage", label: "token 消耗统计" },
  { id: "system", label: "系统" },
  { id: "help", label: "帮助" },
  { id: "about", label: "关于" },
];

// 每个分组认领的 draft 字段——改动落在哪个分组，「未保存」的点就挂在哪一项上。
// 新增字段必须在这里归组，否则它既不会有点、也说不清自己属于哪一栏。
const GROUP_FIELDS: Record<GroupId, (keyof ConfigDTO)[]> = {
  appearance: [],
  llm: ["llmEnabled", "llmPrimary", "llmFallback", "llmEmbedding", "agentRetry", "agentAutoRetry"],
  tools: [],
  usage: [],
  system: ["autostart", "pasteShortcut", "keepOriginalImage", "mediaDirCustom"],
  help: [],
  about: [],
};

/** 搜索命中后的落点：group 决定切到哪一栏，anchor 决定滚到哪个区块。 */
const SEARCH_INDEX: { term: string; group: GroupId; anchor: string }[] = [
  { term: "主题色 · 随系统 / 深色 / 浅色", group: "appearance", anchor: "set-appearance" },
  { term: "大模型总开关 · 关闭 · 降级 · 暂停加工", group: "llm", anchor: "set-llm-master" },
  { term: "主模型（摘要 / 问答）· 供应商 · 模型名 · Base URL", group: "llm", anchor: "set-llm-primary" },
  { term: "备用模型（降级）", group: "llm", anchor: "set-llm-fallback" },
  { term: "向量模型 · embedding · 维度 dim", group: "llm", anchor: "set-llm-embedding" },
  { term: "API Key · 密钥 · 清除密钥", group: "llm", anchor: "set-llm-primary" },
  { term: "测试连接 · 连通性", group: "llm", anchor: "set-llm-primary" },
  { term: "重试次数 · 退避基数 · 退避上限 · 失败自动重排", group: "llm", anchor: "set-retry" },
  { term: "技能 · 再加工 · 提示词", group: "tools", anchor: "set-skills" },
  { term: "习惯 · 周回顾 · 火花回路", group: "tools", anchor: "set-habits" },
  { term: "token 用量 · 单价 · 估算金额", group: "usage", anchor: "set-usage" },
  { term: "开机自动启动 · 剪贴板收集 · 加速键", group: "system", anchor: "set-system" },
  { term: "图片 · 保留原图 · 压缩 · media 目录占用 · 存放目录 · 存储 · 自定义路径", group: "system", anchor: "set-media" },
  { term: "帮助 · 从零上手 · 快捷键 · 三层去向 · 隐私边界 · 常见问题", group: "help", anchor: "set-help" },
  { term: "版本 · 检查更新 · 仓库地址", group: "about", anchor: "set-about" },
];

// 二级导航换成侧列的宽度：再窄就退回顶部 Chip 行，把横向余量全留给内容列。
// 800（全局 minWidth）时三列会把内容压到 488px，装不下"供应商 + Base URL + 测试连接"一行。
const SIDE_QUERY = "(min-width: 1024px)";

function SettingsForm() {
  const saved = useSettings((s) => s.config);
  const savePatch = useSettings((s) => s.savePatch);
  const [draft, setDraft] = useState<ConfigDTO>(saved);
  // 每个端点独立的测试态（primary / fallback / embedding）。
  const [testState, setTestState] = useState<
    Partial<Record<LlmTestTarget, { testing: boolean; result: LlmTestResult | null }>>
  >({});
  const [keyDraft, setKeyDraft] = useState<Record<string, string>>({});
  const [cleared, setCleared] = useState<Record<string, boolean>>({});

  const setKey = (provider: string, v: string) =>
    setKeyDraft((m) => ({ ...m, [provider]: v }));
  const setClear = (provider: string, c: boolean) =>
    setCleared((m) => ({ ...m, [provider]: c }));

  const embeddingChanged =
    draft.llmEmbedding.model !== saved.llmEmbedding.model ||
    draft.llmEmbedding.dim !== saved.llmEmbedding.dim;

  // 待提交密钥：非空输入=写入；标记清除且未重新输入=删除（空串约定）。
  const secrets: Record<string, string> = {};
  for (const [p, v] of Object.entries(keyDraft)) if (v.trim()) secrets[p] = v.trim();
  for (const [p, c] of Object.entries(cleared)) if (c && !(p in secrets)) secrets[p] = "";

  // 相对已存配置是否有实际改动（apiKeys 是只读展示态、pasteShortcutError 是运行时状态，
  // media* 三项是后端每次现算的磁盘占用（收一张图就变），三者都不算"用户改了配置"，故排除。
  // 不排掉的话：draft 是挂载时的一次性快照，占用数一变「未保存」的点就永远挂着）。
  const stripKeys = (c: ConfigDTO) =>
    JSON.stringify({
      ...c,
      apiKeys: undefined,
      pasteShortcutError: undefined,
      mediaDir: undefined,
      mediaUsageBytes: undefined,
      mediaImageCount: undefined,
    });
  const dirty = stripKeys(draft) !== stripKeys(saved) || Object.keys(secrets).length > 0;

  const save = async () => {
    const errs = validateDraft(draft);
    if (errs.length) {
      toast(`无法保存：${errs[0]}${errs.length > 1 ? ` 等 ${errs.length} 处` : ""}`, "error");
      return;
    }
    if (!dirty) {
      toast("没有需要保存的改动", "info");
      return;
    }
    if (await savePatch(draft, secrets)) {
      setKeyDraft({});
      setCleared({});
      toast("配置已保存", "info");
    }
  };

  // 换存放目录（批次22-A）：这里**只改草稿**，搬图与放行作用域发生在点「保存」之后。
  // 不给"选完立刻生效"的第二条路径——那会让未保存的草稿与磁盘状态各说一半，而本页其余字段
  // 全都是"草稿 → 保存"一个动作。
  const pickMediaDir = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      directory: true,
      title: "选择图片存放目录",
      defaultPath: draft.mediaDirCustom || saved.mediaDir || undefined,
    });
    if (typeof picked === "string" && picked.trim()) setDraft({ ...draft, mediaDirCustom: picked });
  };

  const runTest = async (target: LlmTestTarget) => {
    // 带上密钥框里的草稿（若有）：让用户"粘完即测、不必先保存"。后端只在本次 ping 用它，绝不落库。
    const provider =
      target === "embedding"
        ? draft.llmEmbedding.provider
        : target === "fallback"
          ? draft.llmFallback?.provider
          : draft.llmPrimary.provider;
    const draftKey = provider ? keyDraft[provider]?.trim() : "";
    setTestState((s) => ({ ...s, [target]: { testing: true, result: null } }));
    try {
      const r = await testLlmConnection(target, draftKey || undefined);
      setTestState((s) => ({ ...s, [target]: { testing: false, result: r } }));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setTestState((s) => ({
        ...s,
        [target]: { testing: false, result: { ok: false, errorCode: "E_TEST", message: msg } },
      }));
    }
  };
  const tst = (t: LlmTestTarget) => testState[t] ?? { testing: false, result: null };

  const setEmbed = (patch: Partial<EmbeddingConfig>) =>
    setDraft({ ...draft, llmEmbedding: { ...draft.llmEmbedding, ...patch } });
  const applyEmbedPreset = (p: Preset) =>
    setDraft({
      ...draft,
      llmEmbedding: {
        provider: p.id,
        baseUrl: p.baseUrl || draft.llmEmbedding.baseUrl,
        model: p.embedModel || draft.llmEmbedding.model,
        dim: p.embedDim ?? draft.llmEmbedding.dim,
      },
    });

  const wide = useMediaQuery(SIDE_QUERY);
  const [group, setGroup] = useState<GroupId>("appearance");
  const [query, setQuery] = useState("");
  const [flash, setFlash] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pendingAnchor = useRef<string | null>(null);

  const hits = useMemo(() => {
    const q = query.trim();
    if (!q) return [];
    return SEARCH_INDEX.filter(
      (e) =>
        e.term.includes(q) ||
        GROUPS.find((g) => g.id === e.group)!.label.includes(q),
    );
  }, [query]);

  const goto = (g: GroupId, anchor?: string) => {
    pendingAnchor.current = anchor ?? null;
    if (anchor) setQuery("");
    setGroup(g);
  };

  // 切分组回到顶部；带锚点时滚到那个区块并描边 1.2s。
  // 用 useLayoutEffect 而不是 rAF：窗口不可见时 rAF 不触发，跳转会当场失效。
  useLayoutEffect(() => {
    const anchor = pendingAnchor.current;
    pendingAnchor.current = null;
    if (!anchor) {
      scrollRef.current?.scrollTo({ top: 0 });
      return;
    }
    const el = document.getElementById(anchor);
    if (!el) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    el.scrollIntoView({ block: "start", behavior: reduce ? "auto" : "smooth" });
    setFlash(anchor);
    const t = setTimeout(() => setFlash(null), 1200);
    return () => clearTimeout(t);
  }, [group]);

  const fieldChanged = (f: keyof ConfigDTO) =>
    JSON.stringify(draft[f]) !== JSON.stringify(saved[f]);
  // 分组归属只决定"未保存的点挂在哪一栏"；能否保存仍以 stripKeys 的 dirty 为准——
  // 万一有新字段忘了归组，保存条仍在，只是少一个点（宁可漏提示，不可漏保存）。
  const dirtyGroups = GROUPS.filter(
    (g) =>
      GROUP_FIELDS[g.id].some(fieldChanged) ||
      (g.id === "llm" && Object.keys(secrets).length > 0),
  ).map((g) => g.id);
  // 锚点外层统一 rounded-xl：ring 跟着元素自己的圆角画，不写圆角就会在圆角卡片外圈出一个方框。
  const anchorCls = (id: string) =>
    `scroll-mt-2 rounded-xl ${flash === id ? "ring-2 ring-primary/60" : ""}`;

  // 二级导航 + 搜索。写成**返回 JSX 的函数**而非内联组件：内联组件每次父级渲染都是新的
  // 元素类型，React 会整棵重挂载——搜索框每敲一个字符就掉焦点。
  const navPanel = (compact: boolean) => {
    const searching = query.trim().length > 0;
    return (
      <>
        {/* 窄屏下搜索框与 tab 之间必须有间距：两侧各自的 padding 都缺一半时会贴死（宽屏走 p-2 有 8px）。 */}
        <div className="p-2">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && hits.length) goto(hits[0].group, hits[0].anchor);
              if (e.key === "Escape") setQuery("");
            }}
            aria-label="搜索设置项"
            placeholder="搜索设置项"
            className="w-full min-w-0 rounded-lg border border-outline-variant bg-surface-high px-2.5 py-1.5 text-xs outline-none placeholder:text-hint focus-visible:ring-2 focus-visible:ring-primary/60"
          />
          {searching && (
            <p className="px-1 pt-2 text-[11px] leading-relaxed text-hint">
              {hits.length ? `命中 ${hits.length} 项` : `没有叫「${query.trim()}」的设置项`}
            </p>
          )}
        </div>
        {searching ? (
          <ul
            className={
              compact
                ? "flex gap-1.5 overflow-x-auto px-2 pt-1 pb-2"
                : "flex flex-col gap-0.5 px-2 pt-1 pb-2"
            }
          >
            {hits.map((h) => (
              <li key={`${h.group}:${h.anchor}:${h.term}`} className={compact ? "shrink-0" : ""}>
                <button
                  onClick={() => goto(h.group, h.anchor)}
                  className="md-press block w-full rounded-lg px-2.5 py-1.5 text-left outline-none hover:bg-surface-high focus-visible:ring-2 focus-visible:ring-primary/60"
                >
                  <span className="block truncate text-xs text-on-surface">{h.term}</span>
                  <span className="block text-[11px] text-hint">
                    {GROUPS.find((g) => g.id === h.group)!.label}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <nav
            aria-label="设置分区"
            className={
              compact
                ? "flex gap-1.5 overflow-x-auto px-2 pb-2"
                : "flex flex-col gap-0.5 p-2 pt-0"
            }
          >
            {GROUPS.map((g) => {
              const active = group === g.id;
              return (
                <button
                  key={g.id}
                  onClick={() => goto(g.id)}
                  aria-current={active ? "true" : undefined}
                  title={
                    dirtyGroups.includes(g.id) ? `${g.label}：有未保存改动` : undefined
                  }
                  className={`md-press flex shrink-0 items-center gap-1.5 rounded-full px-3 py-1.5 text-left text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary/60 ${
                    compact ? "whitespace-nowrap" : "w-full rounded-lg"
                  } ${
                    active
                      ? "bg-secondary-container font-semibold text-on-secondary-container"
                      : "text-on-surface-variant hover:bg-surface-high"
                  }`}
                >
                  {g.label}
                  {dirtyGroups.includes(g.id) && (
                    <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary" />
                  )}
                </button>
              );
            })}
          </nav>
        )}
      </>
    );
  };

  return (
    <div className="flex h-full min-h-0">
      {wide && (
        <aside className="flex w-[184px] shrink-0 flex-col overflow-y-auto border-r border-outline-variant bg-surface-low">
          {navPanel(false)}
        </aside>
      )}
      <div className="relative flex min-w-0 flex-1 flex-col">
        {!wide && (
          <div className="shrink-0 border-b border-outline-variant bg-surface-low">
            {navPanel(true)}
          </div>
        )}
        <div
          ref={scrollRef}
          className="mx-auto flex w-full min-h-0 max-w-3xl flex-1 flex-col gap-5 overflow-y-auto px-6 pt-6 pb-24"
        >
          <h1 className="text-lg font-semibold text-on-surface">
            {GROUPS.find((g) => g.id === group)!.label}
          </h1>

          {group === "appearance" && (
            <div id="set-appearance" className={anchorCls("set-appearance")}>
              <ThemeSection />
            </div>
          )}

          {group === "llm" && (
            <>
              <div id="set-llm-master" className={anchorCls("set-llm-master")}>
                <section className="md-elev rounded-2xl bg-card p-5">
                  <label className="flex items-center justify-between text-sm text-on-surface">
                    <span className="font-semibold">启用大模型</span>
                    <Switch
                      checked={draft.llmEnabled}
                      onChange={(v) => setDraft({ ...draft, llmEnabled: v })}
                    />
                  </label>
                  <p className="mt-2 text-xs leading-relaxed text-hint">
                    {draft.llmEnabled
                      ? "关闭后自动加工、再加工技能与语义检索会整体降级，下方的大模型配置（端点与重试）也会锁定不可编辑。新收集的片段停在队列不加工，检索退回纯关键词。已配的端点、模型与密钥一律原样保留，随时打开即可恢复编辑与使用。"
                      : "大模型已关闭——后台不加工、技能不可用、检索只用关键词，下方大模型配置已锁定。你配的端点与密钥都还在，打开上方开关即刻恢复编辑与使用。"}
                  </p>
                </section>
              </div>

              {/* 总开关关闭时整块大模型配置禁用：必须先「启用大模型」才能改端点/密钥/重试策略。
                  用原生 fieldset[disabled] 一次禁用内部所有 select/input/button/switch（含自定义 Button），
                  并加 opacity-50 给出可见的禁用态；开启即恢复可编辑。 */}
              <fieldset
                disabled={!draft.llmEnabled}
                aria-label="大模型配置"
                className={`m-0 flex min-w-0 flex-col gap-5 border-0 p-0 transition-opacity ${
                  draft.llmEnabled ? "" : "opacity-50"
                }`}
              >
              <div id="set-llm-primary" className={anchorCls("set-llm-primary")}>
                <EndpointFields
                  title="主模型（摘要 / 问答）"
                  value={draft.llmPrimary}
                  onChange={(v) => v && setDraft({ ...draft, llmPrimary: v })}
                  apiKeys={saved.apiKeys}
                  keyDraft={keyDraft}
                  cleared={cleared}
                  onKey={setKey}
                  onClear={setClear}
                  testing={tst("primary").testing}
                  result={tst("primary").result}
                  onTest={() => runTest("primary")}
                />
              </div>
              <div id="set-llm-fallback" className={anchorCls("set-llm-fallback")}>
                <EndpointFields
                  title="备用模型（降级）"
                  value={draft.llmFallback}
                  onChange={(v) => setDraft({ ...draft, llmFallback: v })}
                  apiKeys={saved.apiKeys}
                  keyDraft={keyDraft}
                  cleared={cleared}
                  onKey={setKey}
                  onClear={setClear}
                  testing={tst("fallback").testing}
                  result={tst("fallback").result}
                  onTest={() => runTest("fallback")}
                />
              </div>
              <div className="text-xs text-hint">
                分类 / 标签 / 摘要的路由档位（小模型 vs 大模型）在 llm.* 配置内固定映射，UI 暂不暴露。
              </div>

              <div id="set-llm-embedding" className={anchorCls("set-llm-embedding")}>
                <fieldset className="md-elev rounded-2xl bg-card p-4">
                  <legend className="px-1 text-xs font-semibold text-on-surface-variant">
                    向量模型（embedding）
                  </legend>
                  <div className="grid grid-cols-2 gap-3">
                    <ProviderSelect value={draft.llmEmbedding.provider} onChange={applyEmbedPreset} />
                    <Field label="模型名">
                      <TextInput
                        value={draft.llmEmbedding.model}
                        placeholder="text-embedding-v3"
                        onChange={(e) => setEmbed({ model: e.target.value })}
                      />
                    </Field>
                    <Field label="Base URL">
                      <TextInput
                        value={draft.llmEmbedding.baseUrl}
                        placeholder="https://…/v1"
                        onChange={(e) => setEmbed({ baseUrl: e.target.value })}
                      />
                    </Field>
                    <Field label="维度 dim">
                      <TextInput
                        type="number"
                        value={draft.llmEmbedding.dim}
                        onChange={(e) => setEmbed({ dim: Number(e.target.value) })}
                      />
                    </Field>
                  </div>
                  <div className="mt-3">
                    <KeyInput
                      provider={draft.llmEmbedding.provider}
                      info={saved.apiKeys[draft.llmEmbedding.provider]}
                      value={keyDraft[draft.llmEmbedding.provider] ?? ""}
                      cleared={!!cleared[draft.llmEmbedding.provider]}
                      onValue={(v) => setKey(draft.llmEmbedding.provider, v)}
                      onClear={(c) => setClear(draft.llmEmbedding.provider, c)}
                    />
                  </div>
                  <div className="mt-2">
                    <TestButton
                      testing={tst("embedding").testing}
                      result={tst("embedding").result}
                      onTest={() => runTest("embedding")}
                    />
                  </div>
                </fieldset>
              </div>
              {embeddingChanged && (
                <div className="flex items-start gap-2 rounded-lg border border-warn/30 bg-warn-container px-4 py-2.5 text-sm text-on-warn-container">
                  <Icon name="TriangleAlert" size={15} className="mt-0.5" />
                  <span>更换 embedding 模型或维度将触发向量全量重建（保存后后台执行）。</span>
                </div>
              )}

              <div id="set-retry" className={anchorCls("set-retry")}>
                <section className="md-elev rounded-2xl bg-card p-5">
                  <h2 className="mb-3 text-xs font-semibold text-hint">处理与重试</h2>
                  <div className="grid grid-cols-3 gap-3">
                    <Field label="最大重试次数" hint="0–5">
                      <TextInput
                        type="number"
                        min={0}
                        max={5}
                        value={draft.agentRetry.maxRetries}
                        onChange={(e) =>
                          setDraft({
                            ...draft,
                            agentRetry: { ...draft.agentRetry, maxRetries: Number(e.target.value) },
                          })
                        }
                      />
                    </Field>
                    <Field label="退避基数（ms）">
                      <TextInput
                        type="number"
                        value={draft.agentRetry.backoffBaseMs}
                        onChange={(e) =>
                          setDraft({
                            ...draft,
                            agentRetry: { ...draft.agentRetry, backoffBaseMs: Number(e.target.value) },
                          })
                        }
                      />
                    </Field>
                    <Field label="退避上限（ms）">
                      <TextInput
                        type="number"
                        value={draft.agentRetry.backoffMaxMs}
                        onChange={(e) =>
                          setDraft({
                            ...draft,
                            agentRetry: { ...draft.agentRetry, backoffMaxMs: Number(e.target.value) },
                          })
                        }
                      />
                    </Field>
                  </div>
                  <label className="mt-3 flex items-center justify-between text-sm text-on-surface">
                    失败片段自动重排（agent.auto_retry）
                    <Switch
                      checked={draft.agentAutoRetry}
                      onChange={(v) => setDraft({ ...draft, agentAutoRetry: v })}
                    />
                  </label>
                </section>
              </div>
              </fieldset>
            </>
          )}

          {group === "tools" && (
            <>
              <div id="set-skills" className={anchorCls("set-skills")}>
                <SkillSection />
              </div>
              <div id="set-habits" className={anchorCls("set-habits")}>
                <HabitSection />
              </div>
            </>
          )}

          {group === "usage" && (
            <div id="set-usage" className={anchorCls("set-usage")}>
              <UsageSection />
            </div>
          )}

          {group === "system" && (
            <div id="set-system" className={anchorCls("set-system")}>
              <section className="md-elev rounded-2xl bg-card p-5">
                <h2 className="mb-3 text-xs font-semibold text-hint">开机与剪贴板</h2>
                <label className="flex items-center justify-between text-sm text-on-surface">
                  开机自动启动
                  <Switch
                    checked={draft.autostart}
                    onChange={(v) => setDraft({ ...draft, autostart: v })}
                  />
                </label>

                {/* 收集加速键：可改键位。注册失败（被别的应用占用）不保存——配置里不留按了没反应的键。 */}
                <div className="mt-4 flex flex-wrap items-center gap-2">
                  <span className="text-sm text-on-surface">收集加速键</span>
                  <input
                    value={draft.pasteShortcut}
                    onChange={(e) => setDraft({ ...draft, pasteShortcut: e.target.value })}
                    spellCheck={false}
                    className="h-8 w-52 rounded-lg border border-outline-variant bg-surface px-2 font-mono text-xs text-on-surface outline-none focus:border-primary"
                    placeholder="CommandOrControl+Alt+K"
                  />
                  {draft.pasteShortcut !== DEFAULT_PASTE_SHORTCUT && (
                    <button
                      type="button"
                      onClick={() => setDraft({ ...draft, pasteShortcut: DEFAULT_PASTE_SHORTCUT })}
                      className="text-xs text-hint hover:text-on-surface"
                    >
                      恢复默认
                    </button>
                  )}
                  <span className="text-xs text-hint">
                    当前显示为 {formatShortcut(draft.pasteShortcut)}
                  </span>
                </div>
                {checkShortcutConflict(draft.pasteShortcut) && (
                  <p className="mt-2 text-xs leading-relaxed text-error">
                    与「{checkShortcutConflict(draft.pasteShortcut)}」快捷键冲突，窗口在前台时会同时触发，请换一个键位。
                  </p>
                )}
                {saved.pasteShortcutError && (
                  <p className="mt-2 text-xs leading-relaxed text-warn">
                    已保存的键位没能注册成功：{saved.pasteShortcutError}。改一个不冲突的键位再保存。
                  </p>
                )}
                <p className="mt-3 text-xs leading-relaxed text-hint">
                  收集加速键：按下唤起窗口、读一次剪贴板填进首页输入框——不落库、不经模型，收不收由你决定。
                </p>
                <p className="mt-2 text-xs leading-relaxed text-hint">
                  剪贴板：不后台监听，也不会自动收走你复制过的内容。只有你主动收集时
                  （输入框粘贴，或托盘菜单「收集剪贴板」）才读取一次。
                </p>
              </section>

              <div id="set-media" className={`mt-4 ${anchorCls("set-media")}`}>
                <section className="md-elev rounded-2xl bg-card p-5">
                  <h2 className="mb-3 text-xs font-semibold text-hint">图片收集</h2>
                  <label className="flex items-center justify-between gap-3 text-sm text-on-surface">
                    保留原图（超过 5MB 也不压缩）
                    <Switch
                      checked={draft.keepOriginalImage}
                      onChange={(v) => setDraft({ ...draft, keepOriginalImage: v })}
                    />
                  </label>
                  <p className="mt-2 text-xs leading-relaxed text-hint">
                    图片只原样存在本机：不发送给模型、不占 token、也不会被识别出内容。
                    关掉「保留原图」时，超过 5MB 的会在收集时重编码压小一次——
                    <span className="text-on-surface-variant">压缩不可逆</span>，要留截图原样就开着它。
                    单张超过 30MB 一律拒收。没写附言的图片只能按收集时间找回来。
                  </p>
                  <dl className="mt-3 space-y-1 text-xs">
                    <div className="flex items-baseline justify-between gap-3">
                      <dt className="shrink-0 text-hint">已收藏图片</dt>
                      <dd className="text-on-surface-variant">
                        {saved.mediaImageCount} 张 · {formatBytes(saved.mediaUsageBytes)}
                      </dd>
                    </div>
                    <div className="flex items-baseline justify-between gap-3">
                      <dt className="shrink-0 text-hint">存放目录</dt>
                      <dd className="flex min-w-0 items-baseline justify-end gap-2">
                        <span
                          className="min-w-0 truncate text-on-surface-variant"
                          title={saved.mediaDir || undefined}
                        >
                          {/* 目录自启动即已知（旧文案"收进第一张图后生成"是错的）；空=进程未初始化。 */}
                          {saved.mediaDir || "（尚未初始化）"}
                        </span>
                        <button
                          type="button"
                          disabled={!isTauri()}
                          onClick={() => void pickMediaDir()}
                          title={
                            isTauri()
                              ? "换个目录，已收藏的图片会一起搬过去"
                              : "浏览器预览选不了本机目录"
                          }
                          className="shrink-0 text-xs text-primary hover:underline disabled:text-hint disabled:no-underline"
                        >
                          更改…
                        </button>
                        {!!saved.mediaDirCustom && (
                          <button
                            type="button"
                            onClick={() => setDraft({ ...draft, mediaDirCustom: "" })}
                            title={`回到默认目录（当前自定义为 ${saved.mediaDirCustom}）`}
                            className="shrink-0 text-xs text-hint hover:text-on-surface"
                          >
                            恢复默认
                          </button>
                        )}
                      </dd>
                    </div>
                    {draft.mediaDirCustom !== saved.mediaDirCustom && (
                      <p className="pt-1 text-right text-xs leading-relaxed text-primary">
                        保存后改为：{draft.mediaDirCustom.trim() || "默认目录"}
                      </p>
                    )}
                  </dl>
                  <p className="mt-2 text-xs leading-relaxed text-hint">
                    换目录会把已收藏的图片一并搬过去；搬不动就整件保存失败，不会只搬一半。
                    图片始终只在本机，不发送给模型。
                  </p>
                </section>
              </div>
            </div>
          )}

          {group === "help" && (
            <div id="set-help" className={anchorCls("set-help")}>
              <HelpSection />
            </div>
          )}

          {group === "about" && (
            <div id="set-about" className={anchorCls("set-about")}>
              <AboutSection />
            </div>
          )}
        </div>

        {/* 保存按钮浮在内容列右下角。**遮罩已整条删除**（用户反馈原文给了两个方案："缩短"或
            "底部按钮置于遮罩上方"，选后者并推到底：不保留任何蒙纱）。
            原实现是 `absolute inset-0` + `bg-gradient-to-t from-surface …`，等于给整列下半程
            永久盖一层渐变，滚动到底时最后一行的标签/输入/链接被压在纱下看不清——
            `pointer-events-none` 只保证不吃点击，不吃视觉。
            去掉的两条依据：① `Button` 默认档 `primary` 是 `bg-primary` 实心 + `shadow-e1`，
            本身不透明且有投影分层，压在滚动内容上照样可读，不需要底色垫；
            ② 遮罩在页面**不滚动**时仍然全程在场，纯装饰、纯损害。
            配套：滚动容器改 `pt-5 pb-24`（`:665`），给按钮留出 96px 净空，
            滚到底时最后一项落在按钮**上方**而不是它背后。
            外层仍是 `pointer-events-none absolute` 定位条，只有按钮本体 `pointer-events-auto`，
            其余 96px 条带照常可点可选中下方文字。 */}
        <div className="pointer-events-none absolute inset-x-0 bottom-0 z-10 flex h-24 items-end justify-end px-8 pb-6">
          <div className="pointer-events-auto">
            <Button
              onClick={save}
              disabled={!dirty}
              title={dirty ? undefined : "当前没有改动可保存"}
            >
              保存{dirty ? "" : "（无改动）"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

export function SettingsPage() {
  const loaded = useSettings((s) => s.loaded);
  const load = useSettings((s) => s.load);
  // 真实模式：进入设置页才从 DB 拉配置；loaded 前不挂载表单，避免用默认值误覆盖已存配置。
  useEffect(() => {
    void load();
  }, [load]);
  if (!loaded)
    return (
      <div className="mx-auto flex h-full max-w-3xl items-center justify-center px-6 text-sm text-hint">
        加载配置中…
      </div>
    );
  return <SettingsForm />;
}

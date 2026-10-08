// 设置表单状态：双模——Tauri 运行时经 IPC 读写 DB 配置（02 §4）；浏览器预览仍用 mock。
// 密钥经 secrets → 后端写 keyring（每 provider 一把），永不回传明文；mock 分支仅更新本地状态。
import { create } from "zustand";
import { useEffect } from "react";
import type { ConfigDTO, ConfigPatch, LlmTestResult, LlmTestTarget } from "../types/ipc";
import { fakeTestLlm, mockConfig } from "../lib/mock";
import * as api from "../lib/invoke";
import { isTauri } from "../lib/invoke";
import { DEFAULT_PASTE_SHORTCUT } from "../lib/shortcut";
import { toast } from "../components/ui/toast";

const LIVE = isTauri();
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

// 真实模式加载前的占位（表单 gate 在 loaded 之后挂载，此值仅短暂存在、不用于编辑）。
const PENDING_CONFIG: ConfigDTO = {
  llmPrimary: { provider: "openai_compat", baseUrl: "", model: "", timeoutS: 60, maxTokens: 2048 },
  llmFallback: null,
  llmEmbedding: { provider: "openai_compat", baseUrl: "", model: "", dim: 1024 },
  agentRetry: { maxRetries: 3, backoffBaseMs: 1000, backoffMaxMs: 8000 },
  agentAutoRetry: true,
  vectorBackend: "sqlite_vec",
  llmEnabled: true,
  autostart: false,
  pasteShortcut: DEFAULT_PASTE_SHORTCUT,
  keepOriginalImage: false,
  mediaDirCustom: "",
  mediaDir: "",
  mediaUsageBytes: 0,
  mediaImageCount: 0,
  apiKeys: {},
};

interface SettingsState {
  config: ConfigDTO;
  loaded: boolean;
  load: () => Promise<void>;
  savePatch: (patch: Partial<ConfigDTO>, secrets?: Record<string, string>) => Promise<boolean>;
}

export const useSettings = create<SettingsState>((set, get) => ({
  config: LIVE ? PENDING_CONFIG : mockConfig,
  loaded: !LIVE,

  // 挂载即从 DB 拉当前配置（仅真实模式需要；重复调用幂等）。
  load: async () => {
    if (!LIVE || get().loaded) return;
    try {
      set({ config: await api.getConfig(), loaded: true });
    } catch (e) {
      toast(errMsg(e), "error");
    }
  },

  // 全表单保存：真实模式**只发送相对已存配置实际改动的字段**（后端 ConfigPatch 逐字段校验，
  // 未改的空向量段若原样提交会被"未配置完整"挡下）；密钥 secrets 单独附上（非空=写入，空串=删除）。
  savePatch: async (patch, secrets) => {
    const keys = secrets && Object.keys(secrets).length ? secrets : undefined;
    if (!LIVE) {
      set((s) => {
        const apiKeys = { ...s.config.apiKeys };
        for (const [p, v] of Object.entries(keys ?? {}))
          apiKeys[p] = v.trim() ? { hasKey: true, last4: v.slice(-4) } : { hasKey: false };
        return { config: { ...s.config, ...patch, apiKeys } };
      });
      return true;
    }
    const cur = get().config;
    const ne = (a: unknown, b: unknown) => JSON.stringify(a) !== JSON.stringify(b);
    const cp: ConfigPatch = {};
    if (patch.llmPrimary && ne(patch.llmPrimary, cur.llmPrimary)) cp.llmPrimary = patch.llmPrimary;
    if ("llmFallback" in patch && ne(patch.llmFallback ?? null, cur.llmFallback ?? null))
      cp.llmFallback = patch.llmFallback ?? null;
    if (patch.llmEmbedding && ne(patch.llmEmbedding, cur.llmEmbedding)) cp.llmEmbedding = patch.llmEmbedding;
    if (patch.agentRetry && ne(patch.agentRetry, cur.agentRetry)) cp.agentRetry = patch.agentRetry;
    if (patch.agentAutoRetry !== undefined && patch.agentAutoRetry !== cur.agentAutoRetry)
      cp.agentAutoRetry = patch.agentAutoRetry;
    if (patch.vectorBackend && patch.vectorBackend !== cur.vectorBackend) cp.vectorBackend = patch.vectorBackend;
    // 总开关：改了才发（只翻布尔，其它端点/密钥不受影响）。
    if (patch.llmEnabled !== undefined && patch.llmEnabled !== cur.llmEnabled) cp.llmEnabled = patch.llmEnabled;
    if (patch.autostart !== undefined && patch.autostart !== cur.autostart) cp.autostart = patch.autostart;
    // 图片压缩开关（批次21-B）：mediaDir 等运行期注入字段永不回传，只发这个真配置位。
    if (
      patch.keepOriginalImage !== undefined &&
      patch.keepOriginalImage !== cur.keepOriginalImage
    )
      cp.keepOriginalImage = patch.keepOriginalImage;
    // 存放目录（批次22-A）：发的是**设定值**（空串=恢复默认）。未改就不发——后端收到这个字段
    // 会走一遍"搬图 + 放行 asset 作用域 + 自检"，同目录时虽是 no-op，也没必要白跑。
    if (
      patch.mediaDirCustom !== undefined &&
      patch.mediaDirCustom !== cur.mediaDirCustom
    )
      cp.mediaDirCustom = patch.mediaDirCustom;
    // 加速键：改了才发；但若上次注册失败（pasteShortcutError），原样重存也发——
    // 留出"占用它的应用已退出，点一次保存即复位"的路径，不必先改成一个怪键位再改回来。
    if (
      patch.pasteShortcut !== undefined &&
      (ne(patch.pasteShortcut, cur.pasteShortcut) || cur.pasteShortcutError)
    )
      cp.pasteShortcut = patch.pasteShortcut;
    if (keys) cp.secrets = { apiKeys: keys };
    try {
      set({ config: await api.updateConfig(cp) });
      return true;
    } catch (e) {
      toast(errMsg(e), "error");
      return false;
    }
  },
}));

/**
 * 取 media 目录（图片文件所在处，见 02 §4 / `services/media.rs`）。
 * 配置只在收集页加载过，而缩略图在库/检索/详情都要显示——空目录会让真图显示成占位，
 * 所以这里自带一次幂等 `load()`（浏览器预览下 load 直接 return）。
 */
export function useMediaDir(): string {
  const mediaDir = useSettings((s) => s.config.mediaDir);
  const load = useSettings((s) => s.load);
  useEffect(() => {
    void load();
  }, [load]);
  return mediaDir;
}

/**
 * 后端 `worker::llm_ready` 的前端镜像（批次28 §5）：主端点填了 model + base_url，且该 provider
 * 已存密钥——三者缺一，worker 就不领任务、条目永远停在 `pending`（不产全兜底的垃圾结果）。
 * 返回 `null` 表示配置尚未加载（真实模式首轮 IPC 在途）：界面此时两种文案都不发，避免先闪一下
 * "未配置"再翻成"加工中"。调用方需保证已 `load()`（收集页/详情页本就在挂载时拉配置）。
 */
export function useLlmReady(): boolean | null {
  const cfg = useSettings((s) => s.config);
  const loaded = useSettings((s) => s.loaded);
  const ready =
    cfg.llmEnabled &&
    !!cfg.llmPrimary.model.trim() &&
    !!cfg.llmPrimary.baseUrl.trim() &&
    !!cfg.apiKeys[cfg.llmPrimary.provider]?.hasKey;
  return loaded ? ready : null;
}

/** 大模型是否被总开关关闭（区别于"未配置"）：配置齐全但用户手动关了。loaded 前返回 null。 */
export function useLlmDisabled(): boolean | null {
  const cfg = useSettings((s) => s.config);
  const loaded = useSettings((s) => s.loaded);
  return loaded ? !cfg.llmEnabled : null;
}

// 连通测试：真实模式测**已保存**的 DB 配置（非当前草稿）对应端点，未配置抛 E_CONFIG_MISSING。
export async function testLlmConnection(
  target: LlmTestTarget = "primary",
  apiKey?: string,
): Promise<LlmTestResult> {
  if (LIVE) return api.testLlmConfig(target, apiKey);
  const cfg = useSettings.getState().config;
  const model =
    target === "embedding" ? cfg.llmEmbedding.model : target === "fallback" ? cfg.llmFallback?.model : cfg.llmPrimary.model;
  return fakeTestLlm(model);
}

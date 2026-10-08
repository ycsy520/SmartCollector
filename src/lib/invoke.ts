// 类型安全的 invoke 封装：每个 02 清单 command 一个函数，参数按 Tauri v2 约定用 camelCase 键。
// 纯 IPC 层——不含 mock 分支；是否调用由 store 侧的 isTauri() 决定（见 stores/fragments.ts）。
import { invoke } from "@tauri-apps/api/core";
import type {
  ClipboardOutcome,
  ConfigDTO,
  ConfigPatch,
  DeleteOutcome,
  ExportOutcome,
  ExportQuery,
  FragmentDetail,
  HabitRule,
  ImageInput,
  ListQuery,
  LlmTestResult,
  LlmTestTarget,
  Page,
  FragmentSummary,
  RelatedOutcome,
  RetryOutcome,
  SearchQuery,
  SearchResult,
  Skill,
  SkillOutcome,
  SubmitInput,
  SubmitOutcome,
  UsageStats,
  WeekDigest,
} from "../types/ipc";

/** 是否运行在 Tauri 桌面运行时内（浏览器预览为 false，走 mock）。 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** 后端错误码 → 用户可读说法（02 §6）。码仍附在括号里，便于反馈时定位。 */
const FRIENDLY: Record<string, string> = {
  E_INPUT_EMPTY: "内容为空",
  E_INPUT_INVALID: "内容不符合要求",
  E_INPUT_TOO_LARGE: "内容太长（超出上限）",
  E_URL_INVALID: "链接格式不对",
  E_FETCH_FAILED: "抓取失败，检查网络后重试",
  E_CLIPBOARD_READ: "读不到剪贴板",
  E_NOT_FOUND: "这条记录已经不在了",
  E_STATE_CONFLICT: "这条当前状态下不能这样操作",
  E_RETRY_LIMIT: "重试次数已用完",
  // 隐私闸门拒发（02 §2.2）：正常路径由详情页警告弹窗吸收，走到这里说明状态已过期，
  // 文案要说明"为什么没发出去"，否则用户以为按钮坏了。
  E_SENSITIVE_CONFIRM: "这条含隐私信息，需要你先确认才会发送给模型",
  E_DB_READ: "本地数据读取失败",
  E_DB_WRITE: "本地数据保存失败",
  E_CONFIG_MISSING: "模型还没配置好，先去设置页填端点与密钥",
  E_SECRET_STORE: "系统密钥库不可用，密钥没保存",
  E_LLM_TIMEOUT: "模型响应超时，稍后再试",
  E_LLM_RATE_LIMIT: "模型调用太频繁，稍后再试",
  E_LLM_BAD_OUTPUT: "模型返回的内容没能解析，已按原文兜底保存",
  E_VECTOR_UNAVAILABLE: "向量检索暂不可用，关键词检索仍可用",
  E_INTERNAL: "出了点意外，请重试或查看日志",
};

/** 把后端 `{ code, message }` 结构化错误抛成 Error（人话优先，附错误码）。 */
function rethrow(e: unknown): never {
  if (e && typeof e === "object" && "code" in e) {
    const { code, message } = e as { code: string; message?: string };
    throw new Error(`${FRIENDLY[code] ?? "操作没成功"}（${code}${message ? ` · ${message}` : ""}）`);
  }
  throw e instanceof Error ? e : new Error(String(e));
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    return rethrow(e);
  }
}

// —— 片段（02 §1–2）——
export const submitText = (input: SubmitInput) =>
  call<SubmitOutcome>("submit_text", { input });
export const submitClipboard = () => call<ClipboardOutcome>("submit_clipboard");
// 02 §1.4：图片原样收藏，不经模型。data 是 data URL（或裸 base64），后端按魔数判格式。
export const submitImage = (input: ImageInput) =>
  call<SubmitOutcome>("submit_image", { input });
export const getFragments = (query: ListQuery) =>
  call<Page<FragmentSummary>>("get_fragments", { query });
export const getFragment = (fragmentId: string) =>
  call<FragmentDetail>("get_fragment", { fragmentId });
// 隐私闸门（02 §2.2 / §2.3）：命中身份证/卡号/手机号/口令的条目，后端默认拒绝重新处理。
// `confirmSensitive` 只允许由用户看过警告弹窗后带 true 进来——它不是"跳过提示"的开关，
// 前端非确认路径一律传 false，让后端做最终判定（本地规则与详情页旗标同源，不会两套口径）。
export const retryFragment = (fragmentId: string, confirmSensitive: boolean) =>
  call<RetryOutcome>("retry_fragment", { fragmentId, confirmSensitive });
export const deleteFragment = (fragmentId: string) =>
  call<DeleteOutcome>("delete_fragment", { fragmentId });
// 02 §2.8：物理删除，仅接受回收站里的条目；不可撤销，调用方必须先二次确认。
export const purgeFragment = (fragmentId: string) =>
  call<{ purged: boolean }>("purge_fragment", { fragmentId });
export const updateFragment = (
  fragmentId: string,
  patch: { content?: string; title?: string; category?: string; tags?: string[]; note?: string },
) => call<FragmentDetail>("update_fragment", { fragmentId, patch });
export const setFragmentLayer = (fragmentId: string, to: string) =>
  call<FragmentDetail>("set_fragment_layer", { fragmentId, to });
// 技能/一次性指令同样把整段正文外发（02 §2.7），确认口径与 retry_fragment 一致。
// `skillId` 与 `instruction` 二选一：跑既有技能传 skillId；一次性指令传 instruction（skillId 给 null）。
export const runSkill = (
  fragmentId: string,
  skillId: string | null,
  confirmSensitive: boolean,
  instruction?: string,
) =>
  call<SkillOutcome>("run_skill", {
    fragmentId,
    skillId,
    confirmSensitive,
    instruction: instruction ?? null,
  });

// —— 搜索（02 §3）——
export const searchFragments = (query: SearchQuery) =>
  call<SearchResult>("search_fragments", { query });

// —— 导出（02 §8）——
export const exportFragments = (query: ExportQuery) =>
  call<ExportOutcome>("export_fragments", { query });

// —— 配置（02 §4）——
export const getConfig = () => call<ConfigDTO>("get_config");
export const updateConfig = (patch: ConfigPatch) =>
  call<ConfigDTO>("update_config", { patch });
export const testLlmConfig = (target?: LlmTestTarget, apiKey?: string) =>
  call<LlmTestResult>("test_llm_config", { target, apiKey });

// —— 分拣辅助（02 §7.2 / §7.3）——
export const listSkills = () => call<Skill[]>("list_skills");
export const saveSkill = (skill: Skill) => call<Skill>("save_skill", { skill });
export const deleteSkill = (skillId: string) =>
  call<DeleteOutcome>("delete_skill", { skillId });
export const resetSkill = (skillId: string) => call<Skill>("reset_skill", { skillId });
export const listHabits = () => call<HabitRule[]>("list_habits");
export const setHabitState = (habitId: string, state: string) =>
  call<HabitRule>("set_habit_state", { habitId, state });

// —— 火花回路（02 §7.4）——
export const listRelated = (fragmentId: string, limit?: number) =>
  call<RelatedOutcome>("list_related", { fragmentId, limit });
export const getWeekDigest = () => call<WeekDigest>("get_week_digest");

// —— 大模型使用统计（02 §7.5）——
export const getUsageStats = (rangeDays?: number) =>
  call<UsageStats>("get_usage_stats", { rangeDays });

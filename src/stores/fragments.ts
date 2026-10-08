// 片段列表状态：双模——Tauri 运行时走真实 IPC（invoke.ts）+ 事件增量同步（events.ts）；
// 浏览器预览（npm run dev）仍由 lib/mock 驱动，保留可视化 demo。分支只在本 store，页面契约不变。
import { create } from "zustand";
import type {
  Category,
  EditEntry,
  FragmentDetail,
  FragmentSource,
  FragmentStatus,
  Layer,
  MediaType,
} from "../types/ipc";
import {
  bindContentLookup,
  cancelFakeProcess,
  detectCollectFlags,
  scheduleFakeProcess,
  seedFragments,
  uid,
  type Skill,
} from "../lib/mock";
import * as api from "../lib/invoke";
import { isTauri } from "../lib/invoke";
import { putMockImage } from "../lib/media";
import * as ev from "../lib/events";
import { toast, startToast } from "../components/ui/toast";
import { useProcessing } from "./processing";

export interface Filters {
  status: FragmentStatus | "all";
  category: Category | "all";
  source: FragmentSource | "all";
  mediaType: MediaType | "all";
  tag: string | null;
  /**
   * 「只看待加工」：会话内采集时勾了技能、尚未点 `run_skill` 的条目（批次31-A2 解"被积压淹没"）。
   * 纯前端视图筛选，数据源是会话态 `pickedSkills`（不落库、重启即清），零 IPC 零外发。
   */
  pendingOnly: boolean;
}

const DEFAULT_FILTERS: Filters = {
  status: "all",
  category: "all",
  source: "all",
  mediaType: "all",
  tag: null,
  pendingOnly: false,
};

interface FragmentsState {
  items: FragmentDetail[];
  /** 真实模式下首轮 IPC 装载是否仍在途（页面据此显示"载入中"，而不是把空列表当成"真的没有"） */
  loading: boolean;
  filters: Filters;
  setFilters: (patch: Partial<Filters>) => void;
  resetFilters: () => void;
  get: (id: string) => FragmentDetail | undefined;
  submitText: (
    content: string,
    opts?: { title?: string; note?: string; skills?: Skill[] },
  ) => Promise<{ id: string; duplicate: boolean } | null>;
  /**
   * 批次21-B：图片原样收藏（02 §1.4），字节永不进 AI 流水线。
   * 批次23：`content` = 与图**同时**写的那段正文。给了就合成**一条**（文字照常入队加工，
   * 图仍是这条自己的附件），不给就是纯图片条目（占位正文 + 永不整理）。
   */
  submitImage: (
    data: string,
    opts?: { note?: string; content?: string; skills?: Skill[] },
  ) => Promise<{ id: string; duplicate: boolean } | null>;
  /**
   * 隐私闸门（02 §2.2）：`confirmSensitive` 默认 false —— 自动补跑、批量入口这类"没有用户
   * 在环"的调用绝不替用户授权外发；只有详情页警告弹窗点下确认才传 true，后端据此放行。
   */
  retry: (id: string, confirmSensitive?: boolean) => void;
  runSkill: (id: string, skill: Skill, confirmSensitive?: boolean) => void;
  /**
   * 一次性指令（02 §2.7 instruction 分支）：用现场输入的指令当技能正文加工一次，不进技能库、不落库。
   * 与 `runSkill` 同一套隐私/图片/done 闸门；由用户在详情页当场输入并点击发起，外发授权即这次点击。
   */
  runInstruction: (id: string, instruction: string, confirmSensitive?: boolean) => void;
  patchManual: (
    id: string,
    patch: { title?: string; tags?: string[]; category?: Category },
  ) => void;
  archive: (id: string) => void;
  trash: (id: string) => void;
  restore: (id: string) => void;
  /** 批量裁决与撤销用的通用搬层：语义同 set_fragment_layer，静默（不逐条弹 toast）。 */
  setLayer: (id: string, layer: Layer) => void;
  hardDelete: (id: string) => void;
  /** 批量清空回收站：逐条走既有 purge_fragment（真删不可撤销，故不提供撤销动作），只报一次总结。 */
  hardDeleteAll: (ids: string[]) => void;
  remove: (id: string) => void;
  setNote: (id: string, note: string) => void;
  setContent: (id: string, content: string) => void;
  /**
   * 采集时勾选、但尚未加工的再加工技能（批次28 C-1）：`fragmentId → skillId[]`。
   * 后端不代跑（批次9「AI 判断、用户执行」），这里只是**随本次会话**的 UI 标记——刷新即清，
   * 但技能本就没自动跑，标记丢了也不误导。详情页据此显式列出「N 个技能待加工」供逐条点。
   */
  pickedSkills: Record<string, string[]>;
  /** 详情页点跑某条待加工技能后，把它从该条的待加工集合摘掉。 */
  consumePick: (id: string, skillId: string) => void;
  /**
   * 外发动作在途表（批次30-A）：`fragmentId → true`。runSkill/runInstruction/retry 都是"整段正文
   * 外发、等模型回来才有新版本"的慢动作，双击 = 两次外发 = 凭空多一版（还多烧一次 token、
   * 隐私条目多泄一次）。这里给每片段一枚在途闸：入口置位、`.finally` 清位，期间重复点击直接吞掉。
   * 详情页据此把技能 chip / 一次性指令「加工」/「重新处理」置为 disabled + 转圈。
   * 仅 LIVE 需要（浏览器 mock 是同步假加工，没有往返窗口）；纯会话态，不落库、刷新即清。
   */
  running: Record<string, boolean>;
}

const LIVE = isTauri();

// 采集时勾选的再加工技能：仅记进会话态 `pickedSkills` 供详情页呈现，绝不在后台自动跑（批次9 + 批次28 C-1）。
function markPicks(id: string, skillIds: string[]) {
  if (!skillIds.length) return;
  useFragments.setState((s) => ({
    pickedSkills: { ...s.pickedSkills, [id]: skillIds },
  }));
}

// 外发在途闸（批次30-A）：置位/清位都走这里，保证 running 表只由外发动作自己维护。
function setRunning(id: string, on: boolean) {
  useFragments.setState((s) => {
    if (!!s.running[id] === on) return s; // 无变化不触发订阅
    const next = { ...s.running };
    if (on) next[id] = true;
    else delete next[id];
    return { running: next };
  });
}

function syncCounts(items: FragmentDetail[]) {
  useProcessing.getState().setCounts({
    pending: items.filter((f) => f.status === "pending").length,
    running: items.filter((f) => f.status === "running").length,
    failed: items.filter((f) => f.status === "failed").length,
    total: items.length,
  });
}

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

// —— 真实模式（Tauri）：items 为后端镜像，事件与命令回执是唯一写入源 ——
function setItems(items: FragmentDetail[]) {
  useFragments.setState({ items });
  syncCounts(items);
}
function upsert(d: FragmentDetail) {
  const items = useFragments.getState().items;
  setItems(items.some((f) => f.id === d.id) ? items.map((f) => (f.id === d.id ? d : f)) : [d, ...items]);
}
function removeId(id: string) {
  setItems(useFragments.getState().items.filter((f) => f.id !== id));
}
async function hydrate(id: string) {
  try {
    upsert(await api.getFragment(id));
  } catch (e) {
    toast(errMsg(e), "error");
  }
}
// 拉取三层全量并回填详情（get_fragments 仅返回摘要，页面消费详情字段）。
// 只补齐 items 里尚不存在的片段，绝不整体替换——load 在途期间 created/status/updated 事件
// 可能已抢先写入更新详情（如 worker 处理完成），整体替换会用旧快照覆盖它们造成丢失更新，
// 使片段卡在 running/pending 直到下一次（可能永不到来的）事件。首启 items 为空时等价于全量回填。
//
// 分页：02 §0 规定 limit 上限 100，传 500 会被后端 clamp 成 100 且不报错——多出来的条目
// 不会报错，只是**永远看不见**。所以必须按 `total` 翻页取满，而不是指望一次拉全。
const LIST_PAGE = 100;

async function loadLayerIds(layer: Layer): Promise<string[]> {
  const ids: string[] = [];
  for (let offset = 0; ; offset += LIST_PAGE) {
    const page = await api.getFragments({ layer, offset, limit: LIST_PAGE });
    ids.push(...page.items.map((s) => s.id));
    // 空页兜底：total 与实际行数在并发删除下可能对不上，宁可停手也不要死循环
    if (page.items.length === 0 || ids.length >= page.total) return ids;
  }
}

async function load() {
  try {
    const layers: Layer[] = ["buffer", "archived", "trash"];
    const idGroups = await Promise.all(layers.map((l) => loadLayerIds(l)));
    const ids = idGroups.flat();
    const dets = (await Promise.all(ids.map((id) => api.getFragment(id).catch(() => null))))
      .filter((d): d is FragmentDetail => d != null);
    const existing = useFragments.getState().items;
    const have = new Set(existing.map((f) => f.id));
    setItems([...existing, ...dets.filter((d) => !have.has(d.id))]);
  } catch (e) {
    toast(errMsg(e), "error");
  } finally {
    useFragments.setState({ loading: false });
  }
}

export const useFragments = create<FragmentsState>((set, get) => {
  bindContentLookup((id) => get().items.find((f) => f.id === id)?.content);

  const patch = (id: string, p: Partial<FragmentDetail>) => {
    set((s) => ({
      items: s.items.map((f) =>
        f.id === id ? { ...f, ...p, updatedAt: new Date().toISOString() } : f,
      ),
    }));
    syncCounts(get().items);
  };

  const settle = (id: string, ok: boolean) => {
    if (!ok) toast(`处理失败：${id.slice(0, 8)}…（mock E_LLM_TIMEOUT）`, "error");
  };

  // 浏览器 mock：装载种子并驱动假处理定时器；真实模式留给 initFragments() 拉取。
  if (!LIVE) {
    syncCounts(seedFragments);
    seedFragments
      .filter((f) => f.status === "pending" || f.status === "running")
      .forEach((f) => scheduleFakeProcess(f.id, patch, settle));
  }

  return {
    items: LIVE ? [] : seedFragments,
    // 真实模式：initFragments() 紧跟着 load()，首轮 IPC 在途——此刻列表空 ≠ 没有内容
    loading: LIVE,
    filters: DEFAULT_FILTERS,
    pickedSkills: {},
    running: {},
    setFilters: (p) =>
      set((s) => ({ filters: { ...s.filters, ...p } })),
    resetFilters: () => set({ filters: DEFAULT_FILTERS }),
    get: (id) => get().items.find((f) => f.id === id),
    consumePick: (id, skillId) =>
      set((s) => {
        const cur = s.pickedSkills[id];
        if (!cur) return s;
        const rest = cur.filter((x) => x !== skillId);
        const next = { ...s.pickedSkills };
        if (rest.length) next[id] = rest;
        else delete next[id];
        return { pickedSkills: next };
      }),

    submitText: async (content, opts) => {
      if (LIVE) {
        const skillIds = (opts?.skills ?? []).map((s) => s.id);
        try {
          const r = await api.submitText({
            content,
            title: opts?.title,
            note: opts?.note?.trim() || undefined,
            skillIds,
          });
          if (skillIds.length) markPicks(r.fragmentId, skillIds);
          return { id: r.fragmentId, duplicate: r.duplicate };
        } catch (e) {
          toast(errMsg(e), "error");
          return null;
        }
      }
      // 幂等收集：同原文已在缓冲区/归档库时不新建，回执那一条（镜像后端 duplicate）。
      const trimmed = content.trim();
      const existing = get().items.find(
        (f) => f.content.trim() === trimmed && f.layer !== "trash",
      );
      if (existing) return { id: existing.id, duplicate: true };
      const id = uid();
      const now = new Date().toISOString();
      const isLink = /^https?:\/\/\S+$/i.test(content.trim());
      const flags = detectCollectFlags(content, get().items);
      const detail: FragmentDetail = {
        id,
        content,
        title: opts?.title,
        source: "manual",
        status: "pending",
        createdAt: now,
        updatedAt: now,
        layer: "buffer",
        mediaType: isLink ? "link" : "text",
        externalUrl: isLink ? content.trim() : undefined,
        reviewed: false,
        note: opts?.note?.trim() || undefined,
        flags: flags.length ? flags : undefined,
      };
      set((s) => ({ items: [detail, ...s.items] }));
      syncCounts(get().items);
      const chosen = opts?.skills ?? [];
      // 批次28 C-1：勾选只标「待加工」，不自动跑（与真实模式一致——后端不代跑，由用户在详情逐条点）。
      if (chosen.length) markPicks(id, chosen.map((sk) => sk.id));
      scheduleFakeProcess(id, patch, (id2, ok) => settle(id2, ok));
      const retrash = flags.find((f) => f.kind === "retrash");
      if (retrash) toast(`${retrash.message}——真的还要吗？（可在详情里对照）`, "info");
      return { id, duplicate: false };
    },

    // 批次21-B：图片走独立命令（02 §1.4），不进文本的 submit_text。纯图片落库即 skipped——
    // 状态文案「未整理」正是它的真实状态：一张没人描述的图，AI 无从整理，也不该被发出去。
    // 批次23：与图同时给的正文不再另起一条，而是合成一条带图的文字碎片（同一条闸门口径）。
    submitImage: async (data, opts) => {
      const note = opts?.note?.trim() || undefined;
      const text = opts?.content?.trim() || "";
      const skillIds = (opts?.skills ?? []).map((s) => s.id);
      if (LIVE) {
        try {
          // 后端 `decode_base64` 只吃裸 base64（见到 data URL 前缀里的 `:` `;` 就判"数据已损坏"），
          // 而缩略图预览需要 data URL。前缀在这道边界剥掉，两处各取所需。
          const r = await api.submitImage({
            data: data.replace(/^data:[^,]*,/, ""),
            note,
            content: text || undefined,
          });
          if (text.length && skillIds.length) markPicks(r.fragmentId, skillIds);
          return { id: r.fragmentId, duplicate: r.duplicate };
        } catch (e) {
          toast(errMsg(e), "error");
          return null;
        }
      }
      // 浏览器预览没有 media 目录，也没有 asset 协议：字节留在内存 Map 里，文件名照真契约造。
      const id = uid();
      const ext = (data.match(/^data:image\/([a-z+]+)/i)?.[1] ?? "png").replace("jpeg", "jpg");
      const name = `${id.slice(0, 8)}.${ext}`;
      putMockImage(name, data);
      const now = new Date().toISOString();
      const detail: FragmentDetail = {
        id,
        content: text ? `${text}\n\n【图片】${name}` : `【图片】${name}`,
        source: "manual",
        status: text ? "pending" : "skipped",
        createdAt: now,
        updatedAt: now,
        layer: "buffer",
        mediaType: text ? "text" : "image",
        mediaPath: name,
        reviewed: false,
        note,
      };
      set((s) => ({ items: [detail, ...s.items] }));
      syncCounts(get().items);
      if (text) {
        const chosen = opts?.skills ?? [];
        if (chosen.length) markPicks(id, chosen.map((sk) => sk.id));
        scheduleFakeProcess(id, patch, (id2, ok) => settle(id2, ok));
      }
      return { id, duplicate: false };
    },

    runSkill: (id, skill, confirmSensitive = false) => {
      const f = get().get(id);
      if (!f) return;
      if (f.status !== "done" || !f.result) {
        toast("基础处理完成后再加工", "error");
        return;
      }
      if (LIVE) {
        if (get().running[id]) return; // 在途闸（批次30-A）：吞掉重复点击，绝不发起第二次外发
        setRunning(id, true);
        api
          .runSkill(id, skill.id, confirmSensitive)
          .then(() => {
            get().consumePick(id, skill.id); // 已加工，从「待加工」集合摘掉
            toast(`已用「${skill.name}」生成新版本，可在版本切换查看`, "info");
          })
          .catch((e) => toast(errMsg(e), "error")) // 完成后 updated 事件回填详情与历史版本
          .finally(() => setRunning(id, false));
        return;
      }
      // 假再加工：以 skill 为据生成"新版本"（真实版走 04 §0 兜底 + skill.prompt 模板）。
      const prev = f.result;
      const prior = f.priorResults ?? [];
      patch(id, {
        result: {
          ...prev,
          summary: `【${skill.name}】${prev.summary}`,
          modelUsed: `skill:${skill.name}`,
          processedAt: new Date().toISOString(),
        },
        priorResults: [prev, ...prior],
      });
      get().consumePick(id, skill.id);
      toast(`已用「${skill.name}」生成新版本，可在版本切换查看`, "info");
    },

    // 一次性指令（02 §2.7 instruction 分支）：把现场指令当技能正文跑一次，不进技能库、不落库。
    // 与 runSkill 同过隐私/图片/done 闸门（后端 precheck_runnable 共用一份），差别只在正文来自当场输入。
    runInstruction: (id, instruction, confirmSensitive = false) => {
      const text = instruction.trim();
      const f = get().get(id);
      if (!f || !text) return;
      if (f.status !== "done" || !f.result) {
        toast("基础处理完成后再加工", "error");
        return;
      }
      if (LIVE) {
        if (get().running[id]) return; // 在途闸（批次30-A）：与 runSkill 同一枚，防重复外发
        setRunning(id, true);
        api
          .runSkill(id, null, confirmSensitive, text)
          .then(() => toast("已按一次性指令生成新版本，可在版本切换查看", "info"))
          .catch((e) => toast(errMsg(e), "error")) // 完成后 updated 事件回填详情与历史版本
          .finally(() => setRunning(id, false));
        return;
      }
      // 假版：把指令当技能名生成新版本，与真机口径一致（model_used=skill:一次性指令）。
      const prev = f.result;
      patch(id, {
        result: {
          ...prev,
          summary: `【一次性指令】${prev.summary}`,
          modelUsed: `skill:一次性指令`,
          processedAt: new Date().toISOString(),
        },
        priorResults: [prev, ...(f.priorResults ?? [])],
      });
      toast("已按一次性指令生成新版本，可在版本切换查看", "info");
    },

    retry: (id, confirmSensitive = false) => {
      const f = get().get(id);
      if (!f) return;
      const retryable =
        f.status === "failed" ||
        f.status === "skipped" ||
        (f.status === "done" &&
          (f.result?.degraded || f.flags?.some((fl) => fl.kind === "stale")));
      if (!retryable) {
        toast("当前状态不可重试", "error");
        return;
      }
      if (LIVE) {
        if (get().running[id]) return; // 在途闸（批次30-A）：重排队也是外发前置，双击无意义且可能重复入队
        setRunning(id, true);
        api
          .retryFragment(id, confirmSensitive)
          .then((r) => patch(id, { status: r.status })) // 队列已复置，后续 running/done 由 worker 事件推进
          .catch((e) => toast(errMsg(e), "error"))
          .finally(() => setRunning(id, false));
        return;
      }
      const prev = f.result;
      const prior = f.priorResults ?? [];
      const manual = f.manual ?? {};
      cancelFakeProcess(id);
      patch(id, { status: "pending", result: undefined });
      scheduleFakeProcess(
        id,
        (pid, p) => {
          if (p.status === "done" && p.result && prev) {
            // 人工改过的分类/标签在重新处理后保留（03 §2 manual 优先）；旧结果转入历史版本
            patch(pid, {
              ...p,
              result: {
                ...p.result,
                ...(manual.category
                  ? { category: prev.category, subcategory: prev.subcategory }
                  : {}),
                ...(manual.tags ? { tags: prev.tags } : {}),
              },
              priorResults: [prev, ...prior],
            });
          } else {
            patch(pid, p);
          }
        },
        settle,
      );
    },

    patchManual: (id, p) => {
      // 只有标签增删需要"操作成功/失败"的岛提示（批次31 第3片）。分类下拉是即时可见的选择，
      // 点一下结果就在眼前，不需要提示占位——故按 `p.tags` 分流，无岛时错误仍走 toast。
      if (LIVE) {
        const h = p.tags !== undefined ? startToast("标签更新中…") : null;
        api
          .updateFragment(id, p)
          .then((d) => {
            upsert(d); // update_fragment 回执即最新详情
            h?.settle("标签已更新");
          })
          .catch((e) => {
            if (h) h.settle(`标签更新失败：${errMsg(e)}`, "error");
            else toast(errMsg(e), "error");
          });
        return;
      }
      set((s) => ({
        items: s.items.map((f) => {
          if (f.id !== id) return f;
          const next = { ...f, updatedAt: new Date().toISOString() };
          const manual = { ...f.manual };
          if (p.title !== undefined) {
            next.title = p.title;
            manual.title = true;
          }
          if (p.tags && next.result) {
            next.result = { ...next.result, tags: p.tags };
            manual.tags = true;
          }
          if (p.category && next.result) {
            next.result = { ...next.result, category: p.category };
            manual.category = true;
          }
          next.manual = manual;
          return next;
        }),
      }));
      if (p.tags !== undefined) toast("标签已更新", "info");
    },

    archive: (id) => {
      if (LIVE) {
        // 乐观翻层即时更新按钮组与「碎片区 N 条」，不等 IPC；成功文案经灵动岛在**回执后**才落
        // （批次31 第2片：动作要有信息量的确认，"已归档"须是真落库后才承诺，失败即改报失败并回滚快照）。
        const prev = get().items.find((f) => f.id === id);
        patch(id, { layer: "archived", archivedBy: "manual", reviewed: true });
        const h = startToast("归档中…");
        api
          .setFragmentLayer(id, "archived")
          .then((d) => {
            upsert(d);
            h.settle("已归档");
          })
          .catch((e) => {
            if (prev) patch(id, { layer: prev.layer, archivedBy: prev.archivedBy, reviewed: prev.reviewed });
            h.settle(`归档失败：${errMsg(e)}`, "error");
          });
        return;
      }
      patch(id, { layer: "archived", archivedBy: "manual", reviewed: true });
      toast("已归档", "info");
    },

    trash: (id) => {
      if (LIVE) {
        const prev = get().items.find((f) => f.id === id);
        patch(id, { layer: "trash", trashedAt: new Date().toISOString(), archivedBy: undefined });
        const h = startToast("移入回收站…");
        api
          .setFragmentLayer(id, "trash")
          .then((d) => {
            upsert(d);
            h.settle("已移入回收站");
          })
          .catch((e) => {
            if (prev)
              patch(id, {
                layer: prev.layer,
                trashedAt: prev.trashedAt,
                archivedBy: prev.archivedBy,
              });
            h.settle(`丢弃失败：${errMsg(e)}`, "error");
          });
        return;
      }
      cancelFakeProcess(id);
      patch(id, {
        layer: "trash",
        trashedAt: new Date().toISOString(),
        archivedBy: undefined,
      });
      toast("已移入回收站", "info");
    },

    restore: (id) => {
      if (LIVE) {
        // 移回 buffer 的文案按来源层区分：归档库→「移回碎片区」，回收站→「恢复」。
        const prev = get().items.find((f) => f.id === id);
        const fromTrash = prev?.layer === "trash";
        patch(id, { layer: "buffer", trashedAt: undefined, archivedBy: undefined, reviewed: false });
        const h = startToast(fromTrash ? "恢复中…" : "移回中…");
        api
          .setFragmentLayer(id, "buffer")
          .then((d) => {
            upsert(d);
            h.settle(fromTrash ? "已恢复至碎片区" : "已移回碎片区");
          })
          .catch((e) => {
            if (prev)
              patch(id, {
                layer: prev.layer,
                trashedAt: prev.trashedAt,
                archivedBy: prev.archivedBy,
                reviewed: prev.reviewed,
              });
            h.settle(`移回失败：${errMsg(e)}`, "error");
          });
        return;
      }
      const f = get().items.find((x) => x.id === id);
      patch(id, {
        layer: "buffer",
        trashedAt: undefined,
        archivedBy: undefined,
        reviewed: false,
      });
      toast(f?.layer === "trash" ? "已恢复至碎片区" : "已移回碎片区", "info");
    },

    setLayer: (id, layer) => {
      if (LIVE) {
        api.setFragmentLayer(id, layer).then(upsert).catch((e) => toast(errMsg(e), "error"));
        return;
      }
      const f = get().items.find((x) => x.id === id);
      if (!f || f.layer === layer) return;
      if (layer === "trash") cancelFakeProcess(id);
      patch(
        id,
        layer === "buffer"
          ? { layer, trashedAt: undefined, archivedBy: undefined, reviewed: false }
          : layer === "archived"
            ? { layer, archivedBy: "manual", reviewed: true, trashedAt: undefined }
            : { layer, trashedAt: new Date().toISOString(), archivedBy: undefined },
      );
    },

    hardDelete: (id) => {
      if (LIVE) {
        // 真删（02 §2.8）：主表行 + FTS + 墓碑 + 向量一起没，不可撤销——故成功才报"已删除"，
        // 失败只报失败，不预先给一个可能兑现不了的承诺。条目退场由 purged 事件驱动。
        api.purgeFragment(id)
          .then(() => toast("已彻底删除，无法恢复", "info"))
          .catch((e) => toast(errMsg(e), "error"));
        return;
      }
      cancelFakeProcess(id);
      set((s) => ({ items: s.items.filter((f) => f.id !== id) }));
      syncCounts(get().items);
      toast("已彻底删除，无法恢复", "info");
    },

    hardDeleteAll: (ids) => {
      if (ids.length === 0) return;
      if (LIVE) {
        // 逐条真删：N 次独立 IPC，非事务——中途失败的条目不回滚已成功项，故按成败计数如实报。
        // 不预先弹"已删除"：与单条同口径，成功才承诺。条目退场由 purged 事件驱动。
        Promise.allSettled(ids.map((id) => api.purgeFragment(id))).then((rs) => {
          const ok = rs.filter((r) => r.status === "fulfilled").length;
          const fail = ids.length - ok;
          toast(
            fail === 0
              ? `已彻底删除 ${ok} 条，无法恢复`
              : `已彻底删除 ${ok} 条，${fail} 条失败（无法恢复的部分请重试）`,
            fail === 0 ? "info" : "error",
          );
        });
        return;
      }
      ids.forEach(cancelFakeProcess);
      set((s) => ({ items: s.items.filter((f) => !ids.includes(f.id)) }));
      syncCounts(get().items);
      toast(`已彻底删除 ${ids.length} 条，无法恢复`, "info");
    },

    // 详情页「删除」按钮语义 = 移入垃圾站（软删除，可恢复）
    remove: (id) => get().trash(id),

    setNote: (id, note) => {
      if (LIVE) {
        // 批次31 第3片：笔记保存统一走岛，"已保存"只在回执后承诺（取代详情页 blur 即亮的就地微确认）。
        const h = startToast("保存中…");
        api
          .updateFragment(id, { note })
          .then((d) => {
            upsert(d); // update_fragment 回执即最新详情（含 note 落库/清除）
            h.settle("笔记已保存");
          })
          .catch((e) => h.settle(`保存失败：${errMsg(e)}`, "error"));
        return;
      }
      patch(id, { note });
      toast("笔记已保存", "info");
    },

    // 批次6-①：人工修订正文。LIVE 走 update_fragment({content})（后端纯本地、删旧向量、置过期标记）；
    // mock 本地镜像同一语义：改正文 + 追加 edit_log + 有结果时打 stale flag 提示"结果已过期"。
    setContent: (id, content) => {
      if (LIVE) {
        // 批次31 第3片：原文保存统一走岛；撞活跃同文 → E_STATE_CONFLICT，回执失败时如实报失败。
        const h = startToast("保存中…");
        api
          .updateFragment(id, { content })
          .then((d) => {
            upsert(d);
            h.settle("正文已保存");
          })
          .catch((e) => h.settle(`保存失败：${errMsg(e)}`, "error"));
        return;
      }
      const f = get().get(id);
      if (!f || content === f.content) return;
      const now = new Date().toISOString();
      const head = f.content.slice(0, 40);
      const entry: EditEntry = {
        at: now,
        field: "content",
        beforeChars: f.content.length,
        afterChars: content.length,
        excerpt: `“${head}${f.content.length > 40 ? "…" : ""}”`,
      };
      const flags = (f.flags ?? []).filter((fl) => fl.kind !== "stale");
      if (f.result)
        flags.push({
          kind: "stale",
          message: "内容已修订 · AI 结果与向量描述的是改前文本",
          action: "点「重新处理」按新正文重整理（会发送给模型）",
        });
      patch(id, {
        content,
        editLog: [...(f.editLog ?? []), entry].slice(-20),
        flags,
      });
      toast("正文已保存", "info");
    },
  };
});

// 真实模式初始化：拉全量 + 订阅 fragment 事件做增量同步。App 挂载时调用一次。
let started = false;
export function initFragments(): void {
  if (!LIVE || started) return;
  started = true;
  void load();
  ev.onFragmentCreated((p) => void hydrate(p.fragmentId));
  ev.onFragmentStatus((p) => {
    patchStatus(p.fragmentId, p.status);
    if (p.status === "failed" && p.errorCode)
      toast(`处理失败：${p.errorCode}`, "error");
    // 批次28 C-1：done 后**不**自动跑勾选技能。旧实现在此无差别 `runSkill(...,false)`——confirmSensitive
    // 硬编 false 会绕过后端隐私闸门、未经用户点头就把正文外发并扣额度，违批次9「AI 判断、用户执行」。
    // 勾选的技能现只留在 pickedSkills 里作为「待加工」标记，由用户在详情页逐条点 run_skill。
  });
  ev.onFragmentUpdated((p) => void hydrate(p.fragmentId));
  // 后台 30 天到期清除（02 §2）：条目是被物理销毁的，不能 hydrate（会 NotFound 弹错误），直接从镜像移除。
  ev.onFragmentPurged((p) => removeId(p.fragmentId));
}

function patchStatus(id: string, status: FragmentStatus) {
  const items = useFragments.getState().items;
  if (!items.some((f) => f.id === id)) return; // 未知片段：待 created 事件补齐
  setItems(items.map((f) => (f.id === id ? { ...f, status, updatedAt: new Date().toISOString() } : f)));
}

// V8（仅浏览器 mock）：14 天超时→自动归档（archivedBy:auto，可补审）。判定与后端
// `db::fragments::auto_archive_expired` 逐条对齐：缓冲区 + 未审 + done + created_at/updated_at
// 均早于 14 天前（updated_at 那道闸让「移回缓冲区」能续期，不被下一轮立刻再收走）。
// 在 store 构建完成后执行，避免在 create() 初始化器内调用 get()（此时尚未就绪）。
function autoArchiveExpired() {
  const cutoff = Date.now() - 14 * 86_400_000;
  const expired = new Set(
    useFragments
      .getState()
      .items.filter(
        (f) =>
          f.layer === "buffer" &&
          f.status === "done" &&
          !f.reviewed &&
          new Date(f.createdAt).getTime() < cutoff &&
          new Date(f.updatedAt).getTime() < cutoff,
      )
      .map((f) => f.id),
  );
  if (!expired.size) return;
  useFragments.setState((s) => ({
    items: s.items.map((f) =>
      expired.has(f.id)
        ? {
            ...f,
            layer: "archived",
            archivedBy: "auto",
            reviewed: false,
            updatedAt: new Date().toISOString(),
          }
        : f,
    ),
  }));
  syncCounts(useFragments.getState().items);
}

if (!LIVE) {
  autoArchiveExpired();
  setInterval(autoArchiveExpired, 60_000);
}

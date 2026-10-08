// 再加工技能（Skill）状态：双模——Tauri 运行时经 IPC 读写 skills 表（02 §7.2）；
// 浏览器预览用 mockSkills（保留可视化 demo）。分支只在本 store，页面契约不变。
// 契约：save_skill 回执即最新 Skill（新技能由后端生成 id）；内置技能不可删（后端 E_STATE_CONFLICT），
// 但 prompt 可改（02 §7.2），改坏了用 reset_skill 回到后端权威默认。
import { create } from "zustand";
import type { Skill } from "../types/ipc";
import { mockSkills } from "../lib/mock";
import * as api from "../lib/invoke";
import { isTauri } from "../lib/invoke";
import { toast } from "../components/ui/toast";

const LIVE = isTauri();
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

interface SkillsState {
  skills: Skill[];
  loaded: boolean;
  load: () => Promise<void>;
  // 保存（新增或修改）：回执即最新；返回保存后的 Skill，失败返回 null（已 toast）。
  save: (skill: Skill) => Promise<Skill | null>;
  remove: (id: string) => Promise<void>;
  /** 内置技能恢复默认名称与 prompt（改坏了不至于永久回不去）。成功返回 true。 */
  reset: (id: string) => Promise<boolean>;
}

// 浏览器 mock 下"内置默认"的唯一来源：模块初始化时对 mockSkills 取快照
// （mock 分支的 save 只替换数组元素、不改这些对象，故快照不会被污染）。
const BUILTIN_SEEDS = new Map(
  mockSkills
    .filter((s) => s.builtin)
    .map((s) => [s.id, { name: s.name, prompt: s.prompt }]),
);

// 空 id = 新增（后端分配）；mock 下用时间戳补一个本地 id。
export const useSkills = create<SkillsState>((set, get) => ({
  skills: LIVE ? [] : mockSkills,
  loaded: !LIVE,

  load: async () => {
    if (!LIVE || get().loaded) return;
    try {
      set({ skills: await api.listSkills(), loaded: true });
    } catch (e) {
      toast(errMsg(e), "error");
    }
  },

  save: async (skill) => {
    try {
      let saved: Skill;
      if (LIVE) {
        saved = await api.saveSkill(skill);
      } else {
        // mock 必须复刻后端的边界：内置技能只接受 prompt + enabled，
        // 否则浏览器里"改了名称也能存"的演示在 LIVE 里是静默失效的假象。
        const cur = get().skills.find((x) => x.id === skill.id);
        saved =
          cur?.builtin
            ? { ...cur, prompt: skill.prompt, enabled: skill.enabled }
            : { ...skill, id: skill.id || `s-${Date.now()}` };
      }
      set((s) => ({
        skills: s.skills.some((x) => x.id === saved.id)
          ? s.skills.map((x) => (x.id === saved.id ? saved : x))
          : [...s.skills, saved],
      }));
      return saved;
    } catch (e) {
      toast(errMsg(e), "error");
      return null;
    }
  },

  remove: async (id) => {
    try {
      if (LIVE) await api.deleteSkill(id);
      set((s) => ({ skills: s.skills.filter((x) => x.id !== id) }));
    } catch (e) {
      toast(errMsg(e), "error");
    }
  },

  reset: async (id) => {
    try {
      const next = LIVE
        ? await api.resetSkill(id)
        : (() => {
            const seed = BUILTIN_SEEDS.get(id);
            const cur = get().skills.find((x) => x.id === id);
            if (!seed || !cur) throw new Error("内置默认不可用");
            return { ...cur, name: seed.name, prompt: seed.prompt };
          })();
      set((s) => ({ skills: s.skills.map((x) => (x.id === next.id ? next : x)) }));
      return true;
    } catch (e) {
      toast(errMsg(e), "error");
      return false;
    }
  },
}));

// 供采集 @指令补全 / 详情页快捷按钮读取（当前快照，非订阅）。
export const snapshotSkills = () => useSkills.getState().skills;

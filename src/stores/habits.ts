// 分拣习惯（Habit）状态：双模——Tauri 运行时经 IPC 读写 habits 表（02 §7.3）；
// 浏览器预览用 mockHabits。仅 candidate/active/paused 三态可持久化（后端 set_habit_state 校验），
// 「以后再说」是组件本地临时隐藏，不落库（02 无 dismiss 命令，见待确认清单）。
import { create } from "zustand";
import type { HabitRule, HabitState } from "../types/ipc";
import { mockHabits } from "../lib/mock";
import * as api from "../lib/invoke";
import { isTauri } from "../lib/invoke";
import { toast } from "../components/ui/toast";

const LIVE = isTauri();
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

interface HabitsState {
  rules: HabitRule[];
  loaded: boolean;
  load: () => Promise<void>;
  setState: (id: string, state: HabitState) => Promise<void>;
}

export const useHabits = create<HabitsState>((set, get) => ({
  rules: LIVE ? [] : mockHabits,
  loaded: !LIVE,

  load: async () => {
    if (!LIVE || get().loaded) return;
    try {
      set({ rules: await api.listHabits(), loaded: true });
    } catch (e) {
      toast(errMsg(e), "error");
    }
  },

  // 状态迁移：后端回执即最新规则（含 hits/total/recentAuto 的权威值）；失败仅 toast、不改本地。
  setState: async (id, state) => {
    try {
      const next = LIVE ? await api.setHabitState(id, state) : null;
      set((s) => ({
        rules: s.rules.map((r) =>
          r.id === id ? (next ?? { ...r, state }) : r,
        ),
      }));
    } catch (e) {
      toast(errMsg(e), "error");
    }
  },
}));

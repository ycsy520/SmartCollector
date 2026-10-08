// 处理中状态计数（侧栏角标用）。纯计数容器，无 mock：值一律由 fragments store 的 syncCounts
// 从 items 派生——真实模式下 items 来自 IPC 全量 + fragment://status 事件增量，故计数即服务端真值。
import { create } from "zustand";

interface ProcessingState {
  pending: number;
  running: number;
  failed: number;
  total: number;
  setCounts: (c: {
    pending: number;
    running: number;
    failed: number;
    total: number;
  }) => void;
}

export const useProcessing = create<ProcessingState>((set) => ({
  pending: 0,
  running: 0,
  failed: 0,
  total: 0,
  setCounts: (c) => set(c),
}));

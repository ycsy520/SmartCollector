// 事件监听封装（02 §0）：Rust→前端六类事件。浏览器预览（非 Tauri）下订阅为无操作，
// 由 store 侧决定是否依赖（mock 模式不派发这些事件）。
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from "./invoke";
import type { FragmentStatus } from "../types/ipc";

export interface FragmentCreatedPayload {
  fragmentId: string;
  source: string;
}
export interface FragmentStatusPayload {
  fragmentId: string;
  status: FragmentStatus;
  errorCode?: string;
}
export interface FragmentUpdatedPayload {
  fragmentId: string;
}
export interface FragmentPurgedPayload {
  fragmentId: string;
}
export interface ConfigChangedPayload {
  keys: string[];
}
export interface ClipboardDraftPayload {
  /** null = 剪贴板里没有文本（例如只复制了图片）。 */
  text: string | null;
}

// 事件名（与 Rust events.rs 常量一致）。
export const EVENTS = {
  created: "fragment://created",
  status: "fragment://status",
  updated: "fragment://updated",
  purged: "fragment://purged",
  config: "config://changed",
  draft: "clipboard://draft",
} as const;

// 非 Tauri 环境直接返回空清理函数，避免 import 后即报错。
function sub<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  if (!isTauri()) return Promise.resolve(() => {});
  return listen<T>(event, (e) => handler(e.payload)).catch(() => () => {});
}

// 开发期（npm run dev）没有 Rust 侧派发方：`window.__scEmitDraft(text)` 手动注入一条，
// 让「加速键→草稿」这条链路能在浏览器里端到端验证。仅 DEV 注册，生产构建里没有这个入口。
if (import.meta.env.DEV && !isTauri()) {
  Object.defineProperty(globalThis, "__scEmitDraft", {
    configurable: true,
    value: (text: string | null) =>
      window.dispatchEvent(new CustomEvent(EVENTS.draft, { detail: { text } })),
  });
}

export const onFragmentCreated = (h: (p: FragmentCreatedPayload) => void) =>
  sub(EVENTS.created, h);export const onFragmentStatus = (h: (p: FragmentStatusPayload) => void) =>
  sub(EVENTS.status, h);
export const onFragmentUpdated = (h: (p: FragmentUpdatedPayload) => void) =>
  sub(EVENTS.updated, h);
export const onFragmentPurged = (h: (p: FragmentPurgedPayload) => void) =>
  sub(EVENTS.purged, h);
export const onConfigChanged = (h: (p: ConfigChangedPayload) => void) =>
  sub(EVENTS.config, h);

// 剪贴板草稿：Tauri 下由加速键派发，浏览器 mock 下由上面的 DEV 注入器派发到同名 window 事件。
export const onClipboardDraft = (h: (p: ClipboardDraftPayload) => void): Promise<UnlistenFn> => {
  if (isTauri()) return sub(EVENTS.draft, h);
  const on = (e: Event) => h((e as CustomEvent<ClipboardDraftPayload>).detail);
  window.addEventListener(EVENTS.draft, on);
  return Promise.resolve(() => window.removeEventListener(EVENTS.draft, on));
};

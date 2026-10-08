// 加速键的显示名：把 Tauri 的跨平台写法（CommandOrControl+Alt+K）翻成用户看得懂的组合键。
// 修饰键顺序保持配置里的写法，只换符号——不重排，否则"Ctrl+Shift+V"会变成另一个键位的样子。
const IS_MAC =
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

// 收集加速键可改（设置→系统）。唤起窗口另有托盘双击/任务栏，不再独占一枚系统级全局键。
// 与 Rust config::accelerator_conflict 同口径——改任一侧都要同步另一侧。
export const DEFAULT_PASTE_SHORTCUT = "CommandOrControl+Alt+K";

// 应用内固定、且能注册成合法全局键的组合。撞上它们会导致窗口在前台时"全局唤起"与"应用内动作"双触发。
const PRIMARY_MODS = new Set(["ctrl", "control", "commandorcontrol", "cmd", "command", "meta"]);
const RESERVED_KEYS: Record<string, string> = {
  enter: "收集（Ctrl+Enter）",
  a: "批量全选（Ctrl+A）",
};

// 命中保留键返回动作名，否则 null。不排 Shift：Ctrl+Shift+Enter 同样触发收集。
export function checkShortcutConflict(raw: string): string | null {
  const parts = raw
    .split("+")
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean);
  if (parts.length < 2) return null;
  const key = parts[parts.length - 1].replace(/^key/, "");
  const mods = parts.slice(0, -1);
  const hasPrimary = mods.some((m) => PRIMARY_MODS.has(m));
  return hasPrimary ? RESERVED_KEYS[key] ?? null : null;
}

const ALIAS: Record<string, string> = {
  commandorcontrol: IS_MAC ? "⌘" : "Ctrl",
  command: "⌘",
  cmd: "⌘",
  control: "Ctrl",
  ctrl: "Ctrl",
  alt: IS_MAC ? "⌥" : "Alt",
  option: "⌥",
  shift: IS_MAC ? "⇧" : "Shift",
  super: IS_MAC ? "⌘" : "Win",
  win: "Win",
};

export function formatShortcut(raw: string): string {
  const parts = raw
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  // 与 Rust 侧 validate_accelerator 同口径：至少「修饰键 + 主键」，否则原样显示用户输入。
  if (parts.length < 2) return raw;
  const shown = parts.map((p, i) => {
    const alias = ALIAS[p.toLowerCase()];
    if (alias) return alias;
    if (i === parts.length - 1) return p.replace(/^Key/i, "").toUpperCase();
    return p;
  });
  return IS_MAC ? shown.join("") : shown.join("+");
}

export function isMac(): boolean {
  return IS_MAC;
}

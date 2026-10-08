// 主题偏好：设备级观好，只进 localStorage（与归档库视图切换同一惯例），不进配置库/契约。
export type ThemePref = "system" | "dark" | "light";

const KEY = "sc.theme";
const darkMedia = () => window.matchMedia("(prefers-color-scheme: dark)");

export function readTheme(): ThemePref {
  const v = localStorage.getItem(KEY);
  return v === "dark" || v === "light" ? v : "system";
}

export function resolveTheme(pref: ThemePref): "dark" | "light" {
  if (pref !== "system") return pref;
  return darkMedia().matches ? "dark" : "light";
}

// 令牌侧只认 html[data-theme]（见 index.css），所以"随系统"也一律解析成显式值再落地。
export function applyTheme(pref: ThemePref): void {
  document.documentElement.dataset.theme = resolveTheme(pref);
}

export function setTheme(pref: ThemePref): void {
  localStorage.setItem(KEY, pref);
  applyTheme(pref);
}

/** 渲染前调用一次：消除首帧错色，并让"随系统"跟着系统实时翻转。 */
export function initTheme(): void {
  applyTheme(readTheme());
  darkMedia().addEventListener("change", () => {
    if (readTheme() === "system") applyTheme("system");
  });
}

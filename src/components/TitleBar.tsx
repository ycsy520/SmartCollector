// 自绘标题栏：tauri.conf.json 的 decorations:false 之后窗口无原生标题条，
// 最小化/最大化/关闭全由这里承担。只在 Tauri 运行时渲染——浏览器里标题栏归操作系统，
// 画出来是点了不会有任何反应的假控件。
// 关闭的语义是"隐藏到托盘"（lib.rs 拦下 CloseRequested 后 hide），文案必须照实说。
import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AppMark } from "./AppMark";
import { Icon, type IconName } from "./ui/icon";

// 悬停底色按分支整体给出，不能 base + 追加：同属性下后写出的类会短路先写出的。
const WIN_BTN =
  "grid h-full w-[46px] place-items-center text-on-surface-variant transition-colors duration-hover ease-emph hover:bg-primary/10 hover:text-on-surface active:bg-primary/15";
const CLOSE_BTN =
  "grid h-full w-[46px] place-items-center text-on-surface-variant transition-colors duration-hover ease-emph hover:bg-error-container hover:text-on-error-container active:bg-error-container/70";

function WinBtn({
  label,
  icon,
  onClick,
  close,
}: {
  label: string;
  icon: IconName;
  onClick: () => void;
  close?: boolean;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={close ? CLOSE_BTN : WIN_BTN}
    >
      <Icon name={icon} size={16} strokeWidth={1.5} />
    </button>
  );
}

export function TitleBar({ pageLabel }: { pageLabel: string }) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const sync = () =>
      win.isMaximized().then((m) => {
        if (alive) setMaximized(m);
      });
    sync();
    const un = win.onResized(sync);
    return () => {
      alive = false;
      un.then((f) => f());
    };
  }, []);

  const win = getCurrentWindow();

  return (
    <div
      data-tauri-drag-region
      className="flex h-10 shrink-0 select-none items-center border-b border-outline-variant/60 bg-surface-low"
    >
      <div data-tauri-drag-region className="flex min-w-0 items-center gap-2 pl-3">
        {/* pointer-events-none：拖拽属性不继承，SVG 自成一个命中区，
            不点亮穿透就会在 logo 上留一块拖不动的死区。 */}
        <AppMark size={20} className="pointer-events-none" />
        <span data-tauri-drag-region className="truncate text-xs font-medium text-on-surface-variant">
          Smart Collector
        </span>
        <span data-tauri-drag-region className="text-xs text-outline">
          /
        </span>
        <span data-tauri-drag-region className="truncate text-xs font-semibold text-on-surface">
          {pageLabel}
        </span>
      </div>
      <div className="ml-auto flex h-full">
        <WinBtn label="最小化" icon="Minus" onClick={() => win.minimize()} />
        <WinBtn
          label={maximized ? "还原" : "最大化"}
          icon={maximized ? "Minimize2" : "Maximize2"}
          onClick={() => win.toggleMaximize()}
        />
        <WinBtn label="隐藏到托盘" icon="X" close onClick={() => win.close()} />
      </div>
    </div>
  );
}

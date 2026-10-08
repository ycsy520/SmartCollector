import type { ComponentProps } from "react";
import { Icon, type IconName } from "./icon";

// M3 按钮族：filled / tonal / outlined / text / destructive。
// primary、ghost、danger 为向后兼容别名。
type Variant =
  | "filled"
  | "tonal"
  | "outlined"
  | "text"
  | "danger"
  | "danger-text"
  | "primary"
  | "ghost";

// 尺寸独立成 mutually-exclusive 的一档，而不是让调用方用 className 覆写基类：
// 项目没有 tailwind-merge，基类里写死 `px-5 text-sm` 时页面再传 `px-2 text-xs` 会被
// CSS 层序（谁在样式表里更靠后谁赢）盖掉，而不是后者胜出——这正是回收站
// 「彻底删除」按钮被撑到换行的根因。md 即历史默认值，opt-in sm 才变小，其余调用点零行为变化。
type Size = "sm" | "md";

// 底色与字色只能来自 STYLE：Tailwind 把 bg-transparent / text-on-surface 排在
// bg-primary、text-on-primary 之后（同属性内按 theme 键序出 CSS），放进基类会让所有
// 实心档静默掉色——preflight 本就已把 <button> 底色归零，这两个类纯属有害的多余。
// - hover 不用 bg-surface-high（灰向，会把品牌暖色的 tonal 按钮洗成灰）；
// - 不用 hover:opacity-*（会把文字一起调透明；M3 的态层是"更深一层"而不是"更淡一层"）；
// - text/outlined 用 primary/10 → /15 的半透明叠加，这是 state layer 的正规写法；
// - 实心档只给 hover 一个压态色，按下反馈交给 .md-press 的弹簧缩放——
//   再造一个 active: 色只是把同一档颜色写两遍。
const STYLE: Record<Variant, string> = {
  filled: "bg-primary text-on-primary shadow-e1 hover:bg-primary-press",
  primary: "bg-primary text-on-primary shadow-e1 hover:bg-primary-press",
  tonal:
    "bg-secondary-container text-on-secondary-container hover:bg-secondary-press",
  outlined:
    "border border-outline text-primary hover:bg-primary/10 active:bg-primary/15",
  ghost:
    "border border-outline text-primary hover:bg-primary/10 active:bg-primary/15",
  text: "text-primary hover:bg-primary/10 active:bg-primary/15",
  danger: "bg-error text-on-error shadow-e1 hover:bg-error-press",
  // 破坏性但低视觉权重：裸红字、hover 才叠半透明红——回收站「彻底删除」这类
  // 你极少点、又不该像 filled danger 那样抢主操作（恢复）眼球的场景。
  "danger-text": "text-error hover:bg-error/10 active:bg-error/15",
};

const SIZE: Record<Size, string> = {
  md: "h-9 px-5 text-sm",
  sm: "h-7 px-3 text-xs",
};

export function Button({
  variant = "primary",
  size = "md",
  icon,
  className = "",
  children,
  ...rest
}: ComponentProps<"button"> & {
  variant?: Variant;
  /** 高度/横向内边距/字号三合一；md 为历史默认，sm 供行内/工具条等紧凑场景。 */
  size?: Size;
  /** 取 ui/icon.tsx 白名单里的图标名；M3 按钮图标 16px、跟随字色、对读屏隐藏。 */
  icon?: IconName;
}) {
  return (
    <button
      className={`md-press inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-full font-semibold outline-none focus-visible:ring-2 focus-visible:ring-primary/60 disabled:pointer-events-none disabled:opacity-40 ${SIZE[size]} ${STYLE[variant]} ${className}`}
      {...rest}
    >
      {icon && <Icon name={icon} />}
      {children}
    </button>
  );
}

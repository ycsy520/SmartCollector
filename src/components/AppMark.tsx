// 应用标记：宽卡匣坐底、圆片悬于其上、卡匣上沿被挖出圆钵插槽、匣内左下嵌一枚圆点
// （与 src-tauri/app-icon.png 母版同一套几何参数）。
// 自绘 SVG 而非位图：随主题令牌翻转、任意尺寸不糊，侧栏与标题栏共用一份。
// 色值直接取 index.css 的 --md-* 变量（与 Tailwind 的 bg-primary/text-on-primary 同源），
// 因为 SVG 的 fill 不吃 Tailwind 类；形状与底互为 primary / on-primary，深浅两态自动反相。
const FIELD = "rgb(var(--md-primary))";
const INK = "rgb(var(--md-on-primary))";

export function AppMark({
  size = 28,
  className = "",
  title,
}: {
  size?: number;
  className?: string;
  title?: string;
}) {
  return (
    <svg
      viewBox="0 0 1024 1024"
      width={size}
      height={size}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      className={`shrink-0 ${className}`}
    >
      {/* 卡匣 / 悬币 / 收纳点三个几何体：原为批次26 的 ActionMark 转场钩子，该组件已退休
          （改由顶部灵动岛给动作反馈），故去掉 mark-* 类名，只留静态形状。 */}
      <rect width="1024" height="1024" rx="214" fill={FIELD} />
      <rect x="107" y="413" width="810" height="514" rx="119" fill={INK} />
      <circle cx="512" cy="419" r="110" fill={FIELD} />
      <circle cx="512" cy="285" r="90" fill={INK} />
      <circle cx="272" cy="770" r="86" fill={FIELD} />
    </svg>
  );
}

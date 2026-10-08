// M3 辅助/筛选 Chip：药丸造型 + tonal 容器；active 态用主色描边。
export function Chip({
  label,
  active = false,
  className = "",
  onClick,
}: {
  label: string;
  active?: boolean;
  className?: string;
  onClick?: () => void;
}) {
  // border-transparent 只留给静态标签分支：Tailwind 把它排在 border-primary /
  // border-outline-variant 之后，写进 base 会让交互 Chip 的描边静默消失。
  // 交互 Chip 的常态底色也必须由组件按 active 分支自己给，不能由调用点用 className 追加：
  // 实测 `.bg-surface-high`(字节 18507) / `.text-on-surface-variant`(23567) 排在
  // `.bg-secondary-container`(17968) / `.text-on-secondary-container`(23235) 之后，
  // 调用点传来的"常态"样式会静默盖掉选中态的 tonal 高亮（筛选 chip 选中与未选中同色）。
  const base =
    "inline-flex max-w-[12rem] truncate items-center rounded-full border px-3 py-1 text-xs font-medium";
  if (!onClick)
    return (
      <span className={`${base} border-transparent ${className}`}>{label}</span>
    );
  return (
    <button
      type="button"
      onClick={onClick}
      className={`${base} md-press ${className} ${
        active
          ? "border-primary bg-secondary-container text-on-secondary-container"
          : "border-outline-variant bg-surface-high text-on-surface-variant hover:bg-surface-highest"
      }`}
    >
      {label}
    </button>
  );
}

import type { Config } from "tailwindcss";
import colors from "tailwindcss/colors";

// 色值以「R G B」三元组存于 CSS 变量；用 <alpha-value> 暴露以兼容 /10、/95 等透明度写法。
const c = (name: string) => `rgb(var(${name}) / <alpha-value>)`;

// tailwindcss/colors 至今仍导出 v2 时代的旧别名（lightBlue/warmGray/trueGray/coolGray/blueGray），
// 整包展开会让每次构建刷 5 条 deprecation 警告。剔除后只保留现行色板，行为不变。
const LEGACY_ALIASES = ["lightBlue", "warmGray", "trueGray", "coolGray", "blueGray"];
const palette = Object.fromEntries(
  Object.keys(colors)
    .filter((name) => !LEGACY_ALIASES.includes(name))
    .map((name) => [name, (colors as Record<string, unknown>)[name]]),
);

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    // 颜色放在顶层 theme.colors（非 extend）：先展开默认色板，再覆写内置族到 M3 角色 var，
    // 保证 bg-stone-*/bg-white/text-stone-* 等旧类名确实继承 M3 配色并随明暗翻转。
    colors: {
      ...palette,

      // —— M3 角色色 ——
      primary: c("--md-primary"),
      "on-primary": c("--md-on-primary"),
      "primary-container": c("--md-primary-container"),
      "on-primary-container": c("--md-on-primary-container"),
      "primary-press": c("--md-primary-press"),
      secondary: c("--md-secondary"),
      "on-secondary": c("--md-on-secondary"),
      "secondary-container": c("--md-secondary-container"),
      "on-secondary-container": c("--md-on-secondary-container"),
      "secondary-press": c("--md-secondary-press"),
      error: c("--md-error"),
      "on-error": c("--md-on-error"),
      "error-container": c("--md-error-container"),
      "on-error-container": c("--md-on-error-container"),
      "error-press": c("--md-error-press"),
      success: c("--md-success"),
      "on-success": c("--md-on-success"),
      "success-container": c("--md-success-container"),
      "on-success-container": c("--md-on-success-container"),
      warn: c("--md-warn"),
      "warn-container": c("--md-warn-container"),
      "on-warn-container": c("--md-on-warn-container"),
      info: c("--md-info"),
      "info-container": c("--md-info-container"),
      "on-info-container": c("--md-on-info-container"),
      surface: c("--md-surface"),
      "surface-lowest": c("--md-surface-container-lowest"),
      "surface-low": c("--md-surface-container-low"),
      "surface-container": c("--md-surface-container"),
      "surface-high": c("--md-surface-container-high"),
      "surface-highest": c("--md-surface-container-highest"),
      "on-surface": c("--md-on-surface"),
      "on-surface-variant": c("--md-on-surface-variant"),
      // 第三级文字（hint/说明/时间戳）专用角色，取代此前泛滥的 text-stone-400（= --md-outline，描边色）
      hint: c("--md-text-hint"),
      outline: c("--md-outline"),
      "outline-variant": c("--md-outline-variant"),
      "inverse-surface": c("--md-inverse-surface"),
      "inverse-on-surface": c("--md-inverse-on-surface"),
      card: c("--md-card"),
      "card-2": c("--md-card-2"),

      // —— 分类色（表达性 tonal） ——
      cat: {
        tech: c("--cat-tech"),
        "tech-on": c("--cat-tech-on"),
        product: c("--cat-product"),
        "product-on": c("--cat-product-on"),
        business: c("--cat-business"),
        "business-on": c("--cat-business-on"),
        learning: c("--cat-learning"),
        "learning-on": c("--cat-learning-on"),
        life: c("--cat-life"),
        "life-on": c("--cat-life-on"),
        news: c("--cat-news"),
        "news-on": c("--cat-news-on"),
        creative: c("--cat-creative"),
        "creative-on": c("--cat-creative-on"),
        other: c("--cat-other"),
        "other-on": c("--cat-other-on"),
      },

      // —— 品牌别名：cinnabar→M3 主色；paper→surface ——
      cinnabar: {
        DEFAULT: c("--md-primary"),
      },
      paper: c("--md-surface"),

      // —— 覆写内置色族：旧类名继承 M3 配色并随明暗翻转 ——
      white: c("--md-card"),
      black: c("--md-scrim"),
      stone: {
        50: c("--md-card"),
        100: c("--md-card-2"),
        200: c("--md-surface-container-high"),
        300: c("--md-outline-variant"),
        400: c("--md-outline"),
        500: c("--md-on-surface-variant"),
        600: c("--md-on-surface-variant"),
        700: c("--md-on-surface"),
        800: c("--md-on-surface"),
        900: c("--md-on-surface"),
        950: c("--md-on-surface"),
      },
      red: {
        50: c("--md-on-error"),
        100: c("--md-error-container"),
        200: c("--md-error-container"),
        300: c("--md-error"),
        400: c("--md-error"),
        500: c("--md-error"),
        600: c("--md-error"),
        700: c("--md-error"),
        800: c("--md-error"),
        900: c("--md-error"),
      },
      emerald: {
        50: c("--md-success-container"),
        100: c("--md-success-container"),
        200: c("--md-success-container"),
        500: c("--md-success"),
        600: c("--md-success"),
        700: c("--md-success"),
        800: c("--md-on-success-container"),
      },
      green: {
        500: c("--md-success"),
        600: c("--md-success"),
        700: c("--md-success"),
      },
      blue: {
        100: c("--md-info-container"),
        500: c("--md-info"),
        600: c("--md-info"),
        700: c("--md-info"),
        800: c("--md-on-info-container"),
      },
      sky: {
        100: c("--md-info-container"),
        800: c("--md-on-info-container"),
      },
      amber: {
        50: c("--md-warn-container"),
        100: c("--md-warn-container"),
        200: c("--md-warn-container"),
        700: c("--md-on-warn-container"),
        800: c("--md-on-warn-container"),
      },
    },
    extend: {
      borderRadius: {
        DEFAULT: "12px",
        sm: "8px",
        md: "12px",
        lg: "16px",
        xl: "28px",
        "2xl": "28px",
        "3xl": "32px",
        full: "9999px",
      },
      fontFamily: {
        sans: [
          "Roboto Flex",
          "system-ui",
          "-apple-system",
          "Segoe UI",
          "PingFang SC",
          "Microsoft YaHei",
          "sans-serif",
        ],
      },
      /* 曲线与时长的唯一源在 index.css 的 :root（--ease-* / --dur-*）。
         这里只引用不复制：复制一份就会有两处真相，改一处静默不生效。 */
      transitionTimingFunction: {
        spring: "var(--ease-spring)",
        emph: "var(--ease-emph)",
        "out-strong": "var(--ease-out-strong)",
        drawer: "var(--ease-drawer)",
      },
      transitionDuration: {
        press: "var(--dur-press)",
        hover: "var(--dur-hover)",
        instant: "var(--dur-instant)",
        enter: "var(--dur-enter)",
        exit: "var(--dur-exit)",
        emphasis: "var(--dur-emph)",
        flash: "var(--dur-flash)",
      },
    },
  },
  plugins: [],
} satisfies Config;

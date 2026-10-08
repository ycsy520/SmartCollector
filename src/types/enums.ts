// 枚举展示元数据（颜色/标签），与 types/ipc.ts 的联合类型对应。
import type { IconName } from "../components/ui/icon";
import type {
  Category,
  FragmentFlagKind,
  FragmentSource,
  FragmentStatus,
  Layer,
  MediaType,
} from "./ipc";

export const STATUSES: FragmentStatus[] = ["pending", "running", "done", "failed", "skipped"];

export const STATUS_LABEL: Record<FragmentStatus, string> = {
  pending: "排队中",
  running: "处理中",
  done: "完成",
  failed: "失败",
  skipped: "未整理",
};

export const LAYER_LABEL: Record<Layer, string> = {
  buffer: "碎片",
  archived: "已归档",
  trash: "垃圾站",
};

export const MEDIA_TYPES: MediaType[] = ["text", "link", "image"];

export const MEDIA_TYPE_LABEL: Record<MediaType, string> = {
  text: "文本",
  link: "链接",
  image: "图片",
};

export const MEDIA_TYPE_ICON: Record<MediaType, IconName> = {
  text: "AlignLeft",
  link: "Link",
  image: "Image",
};

// 收集异常标识（V9）：短标签 + 严重度（灰=info 知情 / 琥珀=warn 需看一眼）。红仅用于破坏性操作，此处不出现。
export const FLAG_LABEL: Record<FragmentFlagKind, string> = {
  retrash: "曾删除",
  dup: "疑似重复",
  conflict: "信息冲突",
  "verify-fail": "存疑",
  junk: "低价值",
  sensitive: "含隐私",
  stale: "已修订",
};

export const FLAG_TONE: Record<FragmentFlagKind, "info" | "warn"> = {
  retrash: "warn",
  dup: "warn",
  conflict: "warn",
  "verify-fail": "info",
  junk: "warn",
  // 隐私也走琥珀不走红：红色按既定制约只留给破坏性操作，而这条要用户"看一眼并处理"，不是报错。
  sensitive: "warn",
  // 过期也走灰（知情）不琥珀：改完正文不是需要警惕的事，只是"结果旧了，要不要重跑"的一次提示。
  stale: "info",
};

export const FLAG_TONE_STYLE: Record<"info" | "warn", string> = {
  info: "border-outline-variant bg-surface-container text-on-surface-variant",
  warn: "border-warn-container bg-warn-container text-on-warn-container",
};

export const SOURCE_LABEL: Record<FragmentSource, string> = {
  manual: "手动",
  clipboard: "剪贴板",
  link: "链接",
};

export const CATEGORIES: Category[] = [
  "技术",
  "产品",
  "商业",
  "学习",
  "生活",
  "资讯",
  "创意",
  "其他",
];

export const CATEGORY_COLOR: Record<Category, string> = {
  技术: "bg-cat-tech text-cat-tech-on",
  产品: "bg-cat-product text-cat-product-on",
  商业: "bg-cat-business text-cat-business-on",
  学习: "bg-cat-learning text-cat-learning-on",
  生活: "bg-cat-life text-cat-life-on",
  资讯: "bg-cat-news text-cat-news-on",
  创意: "bg-cat-creative text-cat-creative-on",
  其他: "bg-cat-other text-cat-other-on",
};

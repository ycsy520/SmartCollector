// 图标层：全站只从这里取图标，页面/组件不得直接 import lucide 的深层路径。
// 用具名导入而非 `import * as Lu` + 动态取值——后者会让 Vite 的 tree-shaking 失效，
// 把整库 1500+ 图标全打进包（本文件存在的唯一理由就是把"取哪几个图标"固定成静态 import）。
import {
  AlignLeft,
  Archive,
  ArrowLeft,
  Check,
  ChevronDown,
  ClipboardPaste,
  Copy,
  Download,
  Flag,
  Image,
  Inbox,
  Info,
  LayoutGrid,
  Link,
  List,
  Loader2,
  Maximize2,
  Minimize2,
  Minus,
  Monitor,
  Moon,
  Pencil,
  RefreshCw,
  Search,
  Settings,
  Sparkles,
  Sun,
  Trash2,
  TriangleAlert,
  Undo2,
  ArrowUp,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

export const ICONS = {
  AlignLeft,
  Archive,
  ArrowLeft,
  Check,
  ChevronDown,
  ClipboardPaste,
  Copy,
  Download,
  Flag,
  Image,
  Inbox,
  Info,
  LayoutGrid,
  Link,
  List,
  Loader2,
  Maximize2,
  Minimize2,
  Minus,
  Monitor,
  Moon,
  Pencil,
  RefreshCw,
  Search,
  Settings,
  Sparkles,
  Sun,
  Trash2,
  TriangleAlert,
  Undo2,
  ArrowUp,
  X,
} satisfies Record<string, LucideIcon>;

export type IconName = keyof typeof ICONS;

/** 行内图标：默认 16px（与 12–14px 正文同高），颜色跟随 currentColor。 */
export function Icon({
  name,
  size = 16,
  className = "",
  strokeWidth = 2,
}: {
  name: IconName;
  size?: number;
  className?: string;
  strokeWidth?: number;
}) {
  const Cmp = ICONS[name];
  return (
    <Cmp
      size={size}
      strokeWidth={strokeWidth}
      aria-hidden="true"
      className={`shrink-0 ${className}`}
    />
  );
}

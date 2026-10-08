// 网格瓦片：归档库"浏览/找回"态的减配卡片——常态只做看，不承载单条分拣动作。
// 多选态（#72）例外：露勾选框、整块点击=切换选中，供批量丢弃。
import type { FragmentSummary } from "../types/ipc";
import { CATEGORY_COLOR, MEDIA_TYPE_ICON, MEDIA_TYPE_LABEL } from "../types/enums";
import { Icon } from "./ui/icon";
import { MediaThumb } from "./MediaThumb";
import { timeAgo } from "../lib/time";
import { imageSrc, stripImageMarker } from "../lib/media";
import { useMediaDir } from "../stores/settings";

export function FragmentTile({
  item,
  onClick,
  selectable,
  selected,
  active,
}: {
  item: FragmentSummary;
  onClick?: () => void;
  selectable?: boolean;
  selected?: boolean;
  active?: boolean; // 并置态：这条正在右栏详情里看着
}) {
  // 网格是"看图找图"的那一态，图片条目没缩略图就等于只剩一串文件名。
  // 只认有文件引用的行（mediaPath）：批次23 的合并行 mediaType 是 text，图照样要显。
  const mediaDir = useMediaDir();
  const hasImage = !!item.mediaPath;
  const pureImage = item.mediaType === "image";
  const thumb = hasImage ? imageSrc(item.mediaPath, mediaDir) : undefined;
  const shown = pureImage ? item.excerpt : stripImageMarker(item.excerpt, item.mediaPath);
  return (
    <div
      onClick={onClick}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      aria-pressed={selectable ? selected : undefined}
      onKeyDown={
        onClick
          ? (e) => {
              if (e.target !== e.currentTarget) return;
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onClick();
              }
            }
          : undefined
      }
      /* 与 FragmentCard 同一套强调通道，且边框三选一互斥（常态边框放进 base 会吞掉失败边框，
         不给边框类又会回落到 preflight 的 #e5e7eb）：边框=状态，ring=选中。 */
      className={`md-elev md-elev-hover relative flex h-full min-w-0 flex-col gap-1.5 rounded-2xl border bg-card p-3 outline-none focus-visible:ring-2 focus-visible:ring-primary/60 ${
        item.status === "failed"
          ? "border-error/60"
          : active && !selected
            ? "border-primary/70"
            : "border-outline-variant/70"
      } ${active && !selected ? "bg-surface-high" : ""} ${
        selected ? "ring-2 ring-primary/50" : ""
      } ${onClick ? "cursor-pointer" : ""}`}
    >
      <div className="flex items-center justify-between gap-2">
        <span
          className={`shrink-0 truncate rounded-full px-2 py-0.5 text-[10px] font-medium ${
            item.category ? CATEGORY_COLOR[item.category] : "bg-cat-other text-cat-other-on"
          }`}
        >
          {item.category ?? "未分类"}
        </span>
        <span className="flex shrink-0 items-center gap-1.5">
          {selectable && (
            <span
              aria-hidden
              className={`flex h-4 w-4 items-center justify-center rounded border ${
                selected
                  ? "border-primary bg-primary text-on-primary"
                  : "border-outline-variant"
              }`}
            >
              {selected && <Icon name="Check" size={11} strokeWidth={3} />}
            </span>
          )}
          <span className="text-xs text-on-surface-variant/60" title={MEDIA_TYPE_LABEL[item.mediaType]}>
            <Icon name={MEDIA_TYPE_ICON[item.mediaType]} size={13} />
          </span>
        </span>
      </div>
      {hasImage && (
        <MediaThumb
          src={thumb}
          alt=""
          imgCls="h-16 w-full shrink-0 rounded-lg border border-outline-variant bg-surface-high object-cover"
          boxCls="h-16 w-full shrink-0 bg-surface-high"
        />
      )}
      <p className="line-clamp-2 min-w-0 flex-1 break-words text-xs leading-relaxed text-on-surface-variant [overflow-wrap:anywhere]">
        {shown}
      </p>
      <span className="shrink-0 text-[10px] text-hint">
        {timeAgo(item.createdAt)}
        {item.archivedBy === "auto" && <span className="ml-1 text-on-surface-variant/60">· 替你收的</span>}
      </span>
    </div>
  );
}

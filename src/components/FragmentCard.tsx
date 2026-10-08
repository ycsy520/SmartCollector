import { useState } from "react";
import type { FragmentSummary, Matched } from "../types/ipc";
import {
  CATEGORY_COLOR,
  FLAG_LABEL,
  FLAG_TONE,
  FLAG_TONE_STYLE,
  MEDIA_TYPE_ICON,
  MEDIA_TYPE_LABEL,
} from "../types/enums";
import { Chip } from "./ui/chip";
import { Icon } from "./ui/icon";
import { StatusBadge } from "./StatusBadge";
import { MediaThumb } from "./MediaThumb";
import { timeAgo } from "../lib/time";
import { imageSrc, stripImageMarker } from "../lib/media";
import { useMediaDir } from "../stores/settings";
import { useFragments } from "../stores/fragments";

// 后端 excerpt 固定 120 字（`commands::fragment::to_summary`）；到此长度即视为被截断。
const EXCERPT_LIMIT = 120;

function Highlighted({ text, query }: { text: string; query?: string }) {
  if (!query || !text.includes(query)) return <>{text}</>;
  const [head, ...rest] = text.split(query);
  return (
    <>
      {head}
      <mark>{query}</mark>
      <Highlighted text={rest.join(query)} query={query} />
    </>
  );
}

export function FragmentCard({
  item,
  query,
  score,
  matched,
  onClick,
  onArchive,
  onTrash,
  dim,
  fresh,
  leaving,
  selectable,
  selected,
  active,
}: {
  item: FragmentSummary;
  query?: string;
  score?: number;
  matched?: Matched;
  onClick?: () => void;
  onArchive?: () => void; // 缓冲区放行（归档）
  onTrash?: () => void; // 缓冲区丢弃（进垃圾站）
  dim?: number; // 老化透明度 0.4~1，缓冲区越久越淡
  fresh?: boolean; // 刚收进缓冲区：一次性入场 + 高亮环
  leaving?: boolean; // 正在放行/丢弃：退场动效后再移除
  selectable?: boolean; // 多选模式：整卡点击改为勾选，露出选择框
  selected?: boolean;
  active?: boolean; // 并置态：这条正在右栏详情里看着（与"多选勾选"是两回事）
}) {
  const failed = item.status === "failed";
  const actionable = !!(onArchive || onTrash);
  // 就地展开：批量扫卡片时"多看几行"不该付出"进详情再退回"的代价（Keep 借鉴 ②）。
  // 展开的仍是后端给的 120 字 excerpt，不是全文——全文要新增批量取详情契约，不做。
  const [expanded, setExpanded] = useState(false);
  const clippable = item.excerpt.length >= EXCERPT_LIMIT;
  // 图片那条：有文件引用就看得见图。批次23 之后"有图"不再等于"mediaType 是 image"——
  // 图 + 正文合成一条时 mediaType 记的是 text（那段字该被整理），只有纯图片条目才记 image。
  // 认 `mediaPath` 而不只认 mediaType：没有文件引用的行"原样收藏"无从谈起（mock 的健壮性
  // 夹具里有一批"正文提到图 URL 的文本行"也标着 image，不能跟着说瞎话）。
  // 取不到预览时留占位框，绝不放会裂开的 <img>（裂图比空框更像 bug）。
  const mediaDir = useMediaDir();
  // 待加工标记（批次31-A1）：采集时勾了、尚未点 run_skill 的技能数，来自会话态 pickedSkills。
  // 取长度（原始值）而非数组引用，避免 pickedSkills 其它键变动时本卡无谓重渲染。
  const pendingPicks = useFragments((s) => s.pickedSkills[item.id]?.length ?? 0);
  const hasImage = !!item.mediaPath;
  // 纯图片条目的正文只有那行占位文件名，它本身就是这条的"内容"；合并条目则剥掉占位行，
  // 显示用户自己写的那段字。
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
              // 只认落在卡片自身上的按键：焦点在卡内按钮上时，空格/回车已由按钮自己处理，
              // 若在此放行会"既执行按钮又跳详情"（键盘用户的连带缺陷，一并堵住）。
              if (e.target !== e.currentTarget) return;
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onClick();
              }
            }
          : undefined
      }
      style={dim !== undefined && !fresh && !leaving && !selected ? { opacity: dim } : undefined}
      /* 强调通道两条，且**互斥**：边框=状态（失败红 / 正在看主色），常态**不描边**——
         A 批「去框」：常态卡片靠 bg-card 色调 + md-elev 阴影与页面分离，不再叠一圈 1px outline
         （框上描框是"视觉压力"的主因）。仍保留 1px 边框宽度（border-transparent）而非 border-0，
         避免失败/在看态切回常态时卡片尺寸抖一下。ring=选中。 */
      className={`md-elev md-elev-hover relative rounded-xl border bg-card p-4 outline-none focus-visible:ring-2 focus-visible:ring-primary/60 ${
        onClick ? "cursor-pointer" : ""
      } ${failed ? "border-error/60" : active && !selected ? "border-primary/70" : "border-transparent"} ${
        active && !selected ? "bg-surface-high" : ""
      } ${selected ? "ring-2 ring-primary/50" : ""} ${
        leaving ? "card-leaving" : fresh ? "card-enter card-fresh" : ""
      }`}
    >
      <div className="flex items-start justify-between gap-3">
        <h3 className="flex min-w-0 items-center gap-1.5 text-sm font-semibold text-on-surface">
          {selectable && (
            <span
              aria-hidden
              className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border ${
                selected
                  ? "border-primary bg-primary text-on-primary"
                  : "border-outline-variant"
              }`}
            >
              {selected && <Icon name="Check" size={11} strokeWidth={3} />}
            </span>
          )}
          <span className="shrink-0 text-on-surface-variant/60" title={MEDIA_TYPE_LABEL[item.mediaType]}>
            <Icon name={MEDIA_TYPE_ICON[item.mediaType]} size={14} />
          </span>
          {item.archivedBy === "auto" && (
            <span
              className="shrink-0 rounded-full bg-secondary-container px-2 py-0.5 text-[10px] font-medium text-on-secondary-container"
              title="超 14 天未分拣，系统替你归档，可补审"
            >
              替你收的
            </span>
          )}
          {item.layer === "archived" && item.status === "pending" && (
            <span
              className="shrink-0 rounded-full bg-surface-high px-2 py-0.5 text-[10px] font-medium text-on-surface-variant"
              title="原文已归档，尚未经大模型加工（摘要为空）。重新启用大模型后会自动补摘要。"
            >
              未整理
            </span>
          )}
          {pendingPicks > 0 && (
            <span
              className="shrink-0 rounded bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-primary"
              title={`采集时勾选了 ${pendingPicks} 个再加工技能，尚未点跑（点开这条逐条加工）`}
            >
              待加工{pendingPicks > 1 ? ` ${pendingPicks}` : ""}
            </span>
          )}
        </h3>
        <StatusBadge status={item.status} />
      </div>

      {hasImage && (
        <div className="mt-2 flex items-center gap-2.5">
          <MediaThumb
            src={thumb}
            alt=""
            imgCls="h-16 w-24 shrink-0 rounded-lg border border-outline-variant bg-surface-high object-cover"
            boxCls="h-16 w-24 shrink-0 bg-surface-high"
            iconSize={18}
          />
          {/* 纯图片条目没有正文，这句是它对自己处境的说明；合并条目下方就是正文，不占地方。 */}
          {pureImage && (
            <p className="min-w-0 text-xs leading-relaxed text-hint">
              原样收藏 · 不经模型
              <br />
              只能按附言与时间找它
            </p>
          )}
        </div>
      )}

      {item.flags && item.flags.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {item.flags.map((fl, i) => (
            <span
              key={i}
              title={fl.message}
              className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium ${FLAG_TONE_STYLE[FLAG_TONE[fl.kind]]}`}
            >
              <Icon name="Flag" size={11} /> {FLAG_LABEL[fl.kind]}
            </span>
          ))}
        </div>
      )}

      <p
        className={`mt-1.5 min-w-0 text-sm leading-relaxed break-words [overflow-wrap:anywhere] text-on-surface-variant ${
          clippable && !expanded ? "line-clamp-3" : ""
        }`}
      >
        <Highlighted text={shown} query={query} />
      </p>
      {clippable && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            setExpanded((v) => !v);
          }}
          aria-expanded={expanded}
          className="mt-0.5 text-xs text-hint underline-offset-2 transition-colors hover:text-primary hover:underline"
        >
          {expanded ? "收起" : "展开全文"}
        </button>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        {item.category && (
          <Chip
            label={item.category}
            className={CATEGORY_COLOR[item.category]}
          />
        )}
        {item.tags.slice(0, 4).map((t) => (
          <Chip key={t} label={t} className="bg-surface-high text-on-surface-variant" />
        ))}
        {item.tags.length > 4 && (
          <span className="text-xs text-hint">+{item.tags.length - 4}</span>
        )}
      </div>

      <div
        className={`mt-2.5 flex items-center justify-between text-xs text-hint ${
          actionable ? "gap-2" : ""
        }`}
      >
        <span>
          {timeAgo(item.createdAt)}
          {score !== undefined && (
            <span className="ml-2 text-primary">
              {Math.round(score * 100)}%
            </span>
          )}
          {matched === "both" && (
            <span className="ml-2 rounded bg-surface-highest px-1 text-on-surface-variant">
              双路命中
            </span>
          )}
        </span>
        {actionable && (
          <span className="flex shrink-0 gap-1.5" onClick={(e) => e.stopPropagation()}>
            {onArchive && (
              <button
                onClick={onArchive}
                className="md-press inline-flex items-center gap-1 rounded-full bg-secondary-container px-3 py-1 font-semibold text-on-secondary-container hover:bg-success-container hover:text-on-success-container"
              >
                <Icon name="Check" size={13} strokeWidth={2.5} />
                放行
              </button>
            )}
            {onTrash && (
              <button
                onClick={onTrash}
                className="md-press inline-flex items-center gap-1 rounded-full border border-outline px-3 py-1 font-medium text-on-surface-variant hover:bg-error-container hover:text-on-error-container"
              >
                <Icon name="Trash2" size={13} />
                丢弃
              </button>
            )}
          </span>
        )}
      </div>

      {score !== undefined && (
        <div className="mt-1 h-1 overflow-hidden rounded-full bg-surface-high">
          <div
            className="h-full rounded-full bg-primary"
            style={{ width: `${Math.min(100, Math.round(score * 100))}%` }}
          />
        </div>
      )}
    </div>
  );
}

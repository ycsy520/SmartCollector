import { useMemo, useState } from "react";
import { useFragments } from "../stores/fragments";
import { toSummary } from "../lib/mock";
import { FragmentCard } from "../components/FragmentCard";
import { Button } from "../components/ui/button";
import { ConfirmDialog } from "../components/ui/dialog";

const DAY_MS = 86_400_000;
const RETAIN_DAYS = 30;

// 回收站：只装明确丢弃的信息，30 天后物理清除；每条可恢复回缓冲区。
export function TrashPage({
  onOpen,
  activeId,
}: {
  onOpen: (id: string) => void;
  activeId?: string | null;
}) {
  const items = useFragments((s) => s.items);
  const loading = useFragments((s) => s.loading);
  const restore = useFragments((s) => s.restore);
  const hardDelete = useFragments((s) => s.hardDelete);
  const hardDeleteAll = useFragments((s) => s.hardDeleteAll);
  // 彻底删除是不可逆动作，且后端真删（主表/FTS/墓碑/向量一起没）——点了先问一次，不就地销毁。
  const [pendingPurge, setPendingPurge] = useState<string | null>(null);
  const [pendingEmpty, setPendingEmpty] = useState(false);

  const trashed = useMemo(
    () =>
      items
        .filter((f) => f.layer === "trash")
        .sort(
          (a, b) =>
            new Date(b.trashedAt ?? b.createdAt).getTime() -
            new Date(a.trashedAt ?? a.createdAt).getTime(),
        ),
    [items],
  );

  const remaining = (iso?: string) => {
    if (!iso) return RETAIN_DAYS;
    const days = Math.floor((Date.now() - new Date(iso).getTime()) / DAY_MS);
    return Math.max(0, RETAIN_DAYS - days);
  };

  return (
    <div className="mx-auto flex h-full max-w-3xl flex-col gap-4 overflow-y-auto px-6 py-5">
      <div className="flex items-center justify-between gap-3">
        <h1 className="text-lg font-semibold text-on-surface">垃圾站</h1>
        <div className="flex items-center gap-3">
          <span className="text-xs text-hint">
            共 {trashed.length} 条 · 满 {RETAIN_DAYS} 天自动清除
          </span>
          {trashed.length > 0 && (
            <Button
              variant="ghost"
              icon="Trash2"
              className="shrink-0 px-2.5 py-1 text-xs text-on-surface-variant"
              onClick={() => setPendingEmpty(true)}
            >
              清空回收站
            </Button>
          )}
        </div>
      </div>

      {trashed.length === 0 ? (
        <p className="py-16 text-center text-sm text-hint">
          {loading
            ? "正在载入…"
            : "垃圾站是空的。这里只放你明确丢弃的信息——不是碎片区，也不催你回来清。"}
        </p>
      ) : (
        <section className="flex flex-col gap-3 pb-6">
          {trashed.map((f) => (
            <div
              key={f.id}
              className="flex items-stretch gap-3"
            >
              <div className="min-w-0 flex-1">
                <FragmentCard item={toSummary(f)} onClick={() => onOpen(f.id)} dim={0.7} active={f.id === activeId} />
              </div>
              <div className="flex w-28 shrink-0 flex-col items-stretch justify-center gap-1.5">
                {(() => {
                  const left = remaining(f.trashedAt);
                  return (
                    <span
                      className={`text-center text-xs ${
                        left <= 7 ? "font-medium text-warn" : "text-hint"
                      }`}
                    >
                      {left > 0 ? `剩 ${left} 天` : "今日清除"}
                    </span>
                  );
                })()}
                <Button
                  variant="tonal"
                  size="sm"
                  icon="Undo2"
                  className="w-full"
                  onClick={() => restore(f.id)}
                >
                  恢复
                </Button>
                <Button
                  variant="danger-text"
                  size="sm"
                  icon="Trash2"
                  className="w-full"
                  onClick={() => setPendingPurge(f.id)}
                >
                  彻底删除
                </Button>
              </div>
            </div>
          ))}
        </section>
      )}

      <ConfirmDialog
        open={pendingPurge !== null}
        title="彻底删除这条？"
        desc="正文、处理结果与向量会一并抹掉，无法恢复。若只是想收回去，选「恢复」回碎片区。"
        confirmText="彻底删除"
        onConfirm={() => {
          if (pendingPurge) {
            hardDelete(pendingPurge);
          }
          setPendingPurge(null);
        }}
        onCancel={() => setPendingPurge(null)}
      />

      <ConfirmDialog
        open={pendingEmpty}
        title={`清空回收站（${trashed.length} 条）？`}
        desc="这里每一条都会立即被彻底删除——正文、处理结果与向量一并抹掉，无法恢复，也等不到 30 天自动清除。若只想收回去，先逐条选「恢复」。"
        confirmText="全部彻底删除"
        onConfirm={() => {
          hardDeleteAll(trashed.map((f) => f.id));
          setPendingEmpty(false);
        }}
        onCancel={() => setPendingEmpty(false)}
      />
    </div>
  );
}

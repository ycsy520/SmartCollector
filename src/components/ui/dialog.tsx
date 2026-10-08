import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "./button";
import { trapTab } from "../../lib/focus";

export function ConfirmDialog({
  open,
  title,
  desc,
  confirmText = "确认",
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  desc?: ReactNode;
  confirmText?: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  // 退场要播完再把状态交给父级：父级一置 open=false 就会直接卸载，动画会被截断成闪跳。
  // 160ms 必须与 index.css 的 --dur-exit（.modal-out/.scrim-out）保持一致。
  const EXIT_MS = 160;
  const [closing, setClosing] = useState(false);
  const exitTimer = useRef<number | null>(null);

  const runAfterExit = (fn: () => void) => {
    if (closing) return;
    setClosing(true);
    exitTimer.current = window.setTimeout(() => {
      exitTimer.current = null;
      setClosing(false);
      fn();
    }, EXIT_MS);
  };

  useEffect(() => {
    if (!open) {
      // 父级用别的途径关掉弹窗时，挂在退场中的回调不能迟到再触发一次 onCancel。
      if (exitTimer.current !== null) {
        window.clearTimeout(exitTimer.current);
        exitTimer.current = null;
      }
      return;
    }
    // 每次真正打开都回到"进入态"：否则父级若绕过 runAfterExit 直接置 open=false，
    // 残留的 closing=true 会让下次打开直接挂上 modal-out（both 填充=永久透明）。
    // 只挂在 open 上——挂在 closing 上会在退场刚开始就把它抹平，modal-out 播不出来。
    setClosing(false);
    confirmRef.current?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") runAfterExit(onCancel);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onCancel, closing]);

  useEffect(
    () => () => {
      if (exitTimer.current !== null) window.clearTimeout(exitTimer.current);
    },
    []
  );

  if (!open) return null;
  return (
    <div
      className={`fixed inset-0 z-40 flex items-center justify-center bg-black/40 backdrop-blur-[1px] ${
        closing ? "scrim-out" : "scrim-in"
      }`}
      onClick={() => runAfterExit(onCancel)}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={`w-80 rounded-3xl border border-outline-variant bg-surface-high p-6 shadow-e2 ${
          closing ? "modal-out" : "modal-in"
        }`}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => trapTab(e, panelRef.current)}
      >
        <h3 className="type-headline text-on-surface">{title}</h3>
        {desc && <p className="mt-2 text-sm text-on-surface-variant">{desc}</p>}
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="ghost" onClick={() => runAfterExit(onCancel)}>
            取消
          </Button>
          <Button ref={confirmRef} variant="danger" onClick={() => runAfterExit(onConfirm)}>
            {confirmText}
          </Button>
        </div>
      </div>
    </div>
  );
}

import { useState } from "react";
import { Chip } from "./ui/chip";
import { Icon } from "./ui/icon";

const MAX_TAGS = 8;

export function TagInput({
  tags,
  onChange,
}: {
  tags: string[];
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState("");

  const commit = () => {
    const t = draft.trim().replace(/[，,]+$/, "");
    if (!t) return;
    if (t.length > 8) return;
    if (!tags.includes(t) && tags.length < MAX_TAGS) onChange([...tags, t]);
    setDraft("");
  };

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tags.map((t) => (
        <span
          key={t}
          className="inline-flex items-center gap-1 rounded-full bg-surface-highest px-2.5 py-0.5 text-xs text-on-surface"
        >
          {t}
          <button
            type="button"
            aria-label={`删除标签 ${t}`}
            className="md-press text-hint hover:text-error"
            onClick={() => onChange(tags.filter((x) => x !== t))}
          >
            <Icon name="X" size={13} />
          </button>
        </span>
      ))}
      {tags.length < MAX_TAGS && (
        <>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === "," || e.key === "，") {
                e.preventDefault();
                commit();
              }
              if (e.key === "Backspace" && !draft && tags.length)
                onChange(tags.slice(0, -1));
            }}
            onBlur={commit}
            placeholder="+ 标签"
            className="w-20 rounded-full border border-dashed border-outline bg-transparent px-2.5 py-0.5 text-xs outline-none transition-colors ease-emph placeholder:text-hint focus:border-primary"
          />
        </>
      )}
      {tags.length === 0 && (
        <Chip label="待补充" className="bg-warn-container text-on-warn-container" />
      )}
    </div>
  );
}

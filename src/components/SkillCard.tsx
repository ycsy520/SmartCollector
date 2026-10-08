// 再加工技能（Skill）管理与编辑器。数据源在 stores/skills（双模：Tauri 真 IPC / 浏览器 mock）。
// skill = {触发 × 适用条件(类型×分类) × prompt 模板}，输出一律作为「新版本 result」
// （映射 03 既有 processing_results.version/superseded，不新增结果字段）。
import { useEffect, useState } from "react";
import type { Category, MediaType, Skill, SkillTrigger } from "../types/ipc";
import { useSkills } from "../stores/skills";
import { CATEGORIES, MEDIA_TYPE_LABEL, MEDIA_TYPES } from "../types/enums";
import { Button } from "./ui/button";
import { Icon } from "./ui/icon";
import { Field, Select, TextInput } from "./ui/form";
import { toast } from "./ui/toast";

const TRIGGER_LABEL: Record<SkillTrigger, string> = {
  auto: "自动·未生效",
  manual: "快捷按钮",
  at: "@指令",
};
const TRIGGER_STYLE: Record<SkillTrigger, string> = {
  auto: "bg-success-container text-on-success-container",
  manual: "bg-info-container text-on-info-container",
  at: "bg-secondary-container text-on-secondary-container",
};

const condText = (s: Skill) =>
  `${s.mediaType === "all" ? "任意类型" : MEDIA_TYPE_LABEL[s.mediaType]} × ${
    s.category === "all" ? "任意分类" : s.category
  }`;

function SkillEditor({
  skill,
  onSave,
  onReset,
  onCancel,
}: {
  skill: Skill;
  onSave: (s: Skill) => void;
  onReset: () => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<Skill>(skill);
  const set = (patch: Partial<Skill>) => setDraft((d) => ({ ...d, ...patch }));
  // 内置技能：只有 prompt 可改（后端同样只接受 prompt + enabled，见 02 §7.2）。
  // 界面必须把这条边界画出来——否则编辑框让人以为改了名称/条件也会生效，保存后静默无变化。
  const locked = skill.builtin;
  return (
    <div className="md-elev flex flex-col gap-3 rounded-2xl border border-primary/40 bg-card p-4">
      {locked && (
        <p className="text-xs leading-relaxed text-hint">
          内置技能只有提示词可改，名称/触发/适用条件固定（它们决定这条技能是什么，也决定「恢复默认」回到哪儿）。
        </p>
      )}
      <div className="grid grid-cols-2 gap-3">
        <Field label="名称">
          <TextInput
            value={draft.name}
            disabled={locked}
            placeholder="如：验证真伪"
            onChange={(e) => set({ name: e.target.value })}
          />
        </Field>
        <Field
          label="触发方式"
          hint={
            draft.trigger === "auto"
              ? "自动执行未实现（后端不会跑它），请改选一种由你发起的方式"
              : "按钮 / @指令 = 由你发起；加工要花你的额度并外发正文，故不做自动"
          }
        >
          <Select
            value={draft.trigger}
            disabled={locked}
            onChange={(e) => set({ trigger: e.target.value as SkillTrigger })}
          >
            {(draft.trigger === "auto"
              ? (["manual", "at", "auto"] as SkillTrigger[])
              : (["manual", "at"] as SkillTrigger[])
            ).map((t) => (
              <option key={t} value={t}>
                {TRIGGER_LABEL[t]}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="适用类型">
          <Select
            value={draft.mediaType}
            disabled={locked}
            onChange={(e) =>
              set({ mediaType: e.target.value as MediaType | "all" })
            }
          >
            <option value="all">任意类型</option>
            {MEDIA_TYPES.map((m) => (
              <option key={m} value={m}>
                {MEDIA_TYPE_LABEL[m]}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="适用分类">
          <Select
            value={draft.category}
            onChange={(e) =>
              set({ category: e.target.value as Category | "all" })
            }
          >
            <option value="all">任意分类</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </Select>
        </Field>
      </div>
      <Field label="Prompt 模板" hint="{{原文}} 为占位符；输出生成新版本结果">
        <textarea
          value={draft.prompt}
          rows={4}
          onChange={(e) => set({ prompt: e.target.value })}
          className="w-full resize-y rounded-lg border border-outline-variant px-2 py-1.5 text-sm leading-relaxed outline-none focus:border-primary focus:ring-1 focus:ring-primary/40"
        />
      </Field>
      <div className="flex justify-end gap-2">
        {locked && (
          <Button
            variant="ghost"
            className="mr-auto px-2.5 py-1 text-xs"
            title="把这条内置技能的提示词恢复成默认（后端权威默认，不是本地副本）"
            onClick={onReset}
          >
            恢复默认提示词
          </Button>
        )}
        <Button variant="ghost" onClick={onCancel}>
          取消
        </Button>
        <Button
          onClick={() => {
            if (!draft.name.trim() || !draft.prompt.trim()) {
              toast("名称与 Prompt 不能为空", "error");
              return;
            }
            onSave(draft);
          }}
        >
          保存技能
        </Button>
      </div>
    </div>
  );
}

function SkillRow({
  skill,
  onEdit,
  onToggle,
  onDelete,
}: {
  skill: Skill;
  onEdit: () => void;
  onToggle: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="md-elev flex items-center justify-between gap-3 rounded-2xl bg-card p-4">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-semibold text-on-surface">
            {skill.name}
          </span>
          <span
            className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] ${TRIGGER_STYLE[skill.trigger]}`}
          >
            {TRIGGER_LABEL[skill.trigger]}
          </span>
          {skill.builtin ? (
            <span className="shrink-0 rounded bg-surface-high px-1.5 py-0.5 text-[10px] text-on-surface-variant">
              内置
            </span>
          ) : (
            <span className="shrink-0 rounded bg-surface-high px-1.5 py-0.5 text-[10px] text-on-surface-variant">
              自定义
            </span>
          )}
          {!skill.enabled && (
            <span className="shrink-0 text-[10px] text-hint">已停用</span>
          )}
        </div>
        <p className="mt-1 truncate text-xs text-hint">
          适用：{condText(skill)} · 输出一律作为新版本结果，可在详情页版本切换查看
        </p>
      </div>
      <div className="flex shrink-0 gap-1.5 text-xs">
        <Button variant="ghost" className="px-2.5 py-1 text-xs" onClick={onEdit}>
          编辑
        </Button>
        <Button variant="ghost" className="px-2.5 py-1 text-xs" onClick={onToggle}>
          {skill.enabled ? "停用" : "启用"}
        </Button>
        {!skill.builtin && (
          <button
            className="md-press self-center rounded-full p-1.5 text-hint hover:text-error"
            onClick={onDelete}
            title="删除技能"
            aria-label="删除技能"
          >
            <Icon name="Trash2" size={14} />
          </button>
        )}
      </div>
    </div>
  );
}

export function SkillSection() {
  const { skills, load, save, remove, reset } = useSkills();
  const [draft, setDraft] = useState<Skill | null>(null); // 未落库的「新增」草稿行
  const [editingId, setEditingId] = useState<string | null>(null);

  useEffect(() => {
    void load();
  }, [load]);

  const isNew = draft !== null && draft.id === editingId;
  const rows: Skill[] = draft ? [...skills, draft] : skills;

  const onSave = async (next: Skill) => {
    // 新增草稿用空 id（后端分配 uuid；mock 由 store 补本地 id）。
    const saved = await save({ ...next, id: isNew ? "" : next.id });
    if (saved) {
      setDraft(null);
      setEditingId(null);
      toast("技能已保存", "info");
    }
  };

  const addNew = () => {
    const d: Skill = {
      id: `draft-${Date.now()}`,
      name: "",
      builtin: false,
      trigger: "manual",
      mediaType: "all",
      category: "all",
      prompt: "",
      enabled: true,
    };
    setDraft(d);
    setEditingId(d.id);
  };

  const cancelEdit = () => {
    setDraft(null);
    setEditingId(null);
  };

  return (
    <section className="flex flex-col gap-2.5">
      <div className="flex items-baseline justify-between">
        <h2 className="text-xs font-semibold text-hint">再加工技能（Skill）</h2>
        <Button variant="ghost" className="px-2.5 py-1 text-xs" onClick={addNew}>
          ＋ 新增技能
        </Button>
      </div>
      <p className="text-xs leading-relaxed text-hint">
        技能对已收集的片段再加工，产出「新版本结果」而非覆盖原值。只有两种发起方式：详情页的快捷按钮、收集框的 @指令——都由你主动点，加工要花额度并把正文外发，故不做后台自动。
      </p>
      {rows.map((s) =>
        editingId === s.id ? (
          <SkillEditor
            key={s.id}
            skill={s}
            onSave={onSave}
            onReset={async () => {
              if (await reset(s.id)) {
                setDraft(null);
                setEditingId(null);
                toast("已恢复默认提示词", "info");
              }
            }}
            onCancel={cancelEdit}
          />
        ) : (
          <SkillRow
            key={s.id}
            skill={s}
            onEdit={() => {
              setDraft(null);
              setEditingId(s.id);
            }}
            onToggle={() => void save({ ...s, enabled: !s.enabled })}
            onDelete={() => void remove(s.id)}
          />
        ),
      )}
    </section>
  );
}

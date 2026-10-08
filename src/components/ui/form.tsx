import type { InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";

const FIELD_CLS =
  "w-full rounded-lg border border-outline-variant bg-surface-lowest px-3 py-1.5 text-sm text-on-surface outline-none transition ease-emph placeholder:text-on-surface-variant/60 focus:border-primary focus:ring-2 focus:ring-primary/30 disabled:cursor-not-allowed disabled:border-outline-variant disabled:bg-surface-high disabled:text-hint";
const SELECT_CLS =
  "rounded-lg border border-outline-variant bg-surface-lowest px-3 py-1.5 text-sm text-on-surface outline-none transition ease-emph focus:border-primary focus:ring-2 focus:ring-primary/30 disabled:cursor-not-allowed disabled:border-outline-variant disabled:bg-surface-high disabled:text-hint";

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs font-medium text-on-surface-variant">
        {label}
      </span>
      {children}
      {hint && <span className="mt-1 block text-xs text-hint">{hint}</span>}
    </label>
  );
}

export function TextInput(props: InputHTMLAttributes<HTMLInputElement>) {
  const { className = "", ...rest } = props;
  return <input className={`${FIELD_CLS} ${className}`} {...rest} />;
}

export function TextArea(props: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  const { className = "", ...rest } = props;
  return <textarea className={`${FIELD_CLS} resize-y ${className}`} {...rest} />;
}

export function Select(props: SelectHTMLAttributes<HTMLSelectElement>) {
  const { className = "", ...rest } = props;
  return (
    <select
      className={`${SELECT_CLS} ${className}`}
      {...rest}
    />
  );
}

export function Switch({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={`relative h-7 w-12 shrink-0 rounded-full transition-colors duration-enter ease-emph ${
        checked ? "bg-primary" : "bg-surface-highest border border-outline"
      }`}
    >
      {/* 位移用 transform 而非 left：left 是布局属性（触发 layout+paint），且 -translate-y-1/2
          与 translate-x 由 Tailwind 的合成 transform 变量共同承担，两者不会互相覆盖。 */}
      <span
        className={`absolute left-1 top-1/2 flex h-5 w-5 -translate-y-1/2 items-center justify-center rounded-full shadow-e1 transition-[transform,background-color] duration-enter ease-spring ${
          checked ? "translate-x-[22px] bg-on-primary" : "translate-x-0 bg-outline"
        }`}
      />
    </button>
  );
}

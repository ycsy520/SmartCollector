import type { ReactNode } from "react";
import { safeHref } from "../lib/url";

/**
 * 极简 Markdown 渲染器（批次29 §A）——**只为渲染大模型回来的摘要区**。
 *
 * 为什么自研：库里没有 md 解析器、没有 sanitizer，且 `tauri.conf.json` 的 `csp` 为 `null`，
 * 把模型输出当 HTML 注入（`dangerouslySetInnerHTML`）等于开一个现成的 XSS 口。这里全程用 React
 * children（自动转义），**绝不 `dangerouslySetInnerHTML`**；链接 href 过 `safeHref` 白名单；图片语法
 * 不渲染（外链加载会绕过 csp 隔离并向第三方暴露访问）。刻意取极小语法集：够摘要用即可，不做表格/
 * 引用块/嵌套深列表/HTML 实体——那些既少见于摘要、又是注入面。
 */

// 内联 token 交替（顺序即优先级）：图片 → 链接 → 行内码 → 粗体 → 斜体。
const INLINE =
  /(!\[[^\]]*\]\([^)]*\))|(\[[^\]]+\]\([^)]+\))|(`[^`]+`)|(\*\*[^*]+\*\*|__[^_]+__)|(\*[^*\n]+\*|_[^_\n]+_)/g;

function renderInline(text: string, keyBase: number): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let i = 0;
  let m: RegExpExecArray | null;
  INLINE.lastIndex = 0;
  while ((m = INLINE.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const tok = m[0];
    const k = `${keyBase}-${i++}`;
    if (m[1]) {
      // 图片 ![alt](url)：按 alt 纯文本呈现，不外链加载。
      const alt = /^\[([^\]]*)\]/.exec(tok.slice(1))?.[1] ?? "";
      out.push(<span key={k}>{alt}</span>);
    } else if (m[2]) {
      // 链接 [text](url)：href 过白名单，不安全协议退化为纯文本。
      const mm = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(tok);
      const label = mm?.[1] ?? tok;
      const safe = safeHref(mm?.[2]);
      out.push(
        safe ? (
          <a
            key={k}
            href={safe}
            target="_blank"
            rel="noreferrer noopener"
            className="text-primary underline decoration-primary/40 underline-offset-2 hover:decoration-primary"
          >
            {label}
          </a>
        ) : (
          <span key={k}>{label}</span>
        ),
      );
    } else if (m[3]) {
      out.push(
        <code key={k} className="rounded bg-surface-high px-1 py-0.5 font-mono text-[0.85em] text-on-surface-variant">
          {tok.slice(1, -1)}
        </code>,
      );
    } else if (m[4]) {
      out.push(
        <strong key={k} className="font-semibold text-on-surface">
          {tok.slice(2, -2)}
        </strong>,
      );
    } else if (m[5]) {
      out.push(<em key={k}>{tok.slice(1, -1)}</em>);
    }
    last = m.index + tok.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

// 标题按层级归到现有字号阶梯，不新增尺寸（M3：只用文本色/字重区分层级）。
function Heading({ level, content, k }: { level: number; content: string; k: number }) {
  const cls =
    level <= 2
      ? "mt-2 mb-1 font-semibold text-on-surface"
      : "mt-1.5 mb-0.5 font-medium text-on-surface-variant";
  return <p className={`text-sm leading-relaxed ${cls}`}>{renderInline(content, k)}</p>;
}

export function Markdown({ text, className = "" }: { text: string; className?: string }) {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let para: string[] = [];
  let list: string[] | null = null;
  let ordered = false;
  let inCode = false;
  let codeLines: string[] = [];
  let key = 0;

  const flushPara = () => {
    if (!para.length) return;
    blocks.push(
      <p key={key++} className="my-1 leading-relaxed">
        {renderInline(para.join(" "), key)}
      </p>,
    );
    para = [];
  };
  const flushList = () => {
    if (!list) return;
    const items = list.map((t, idx) => (
      <li key={idx} className="my-0.5">
        {renderInline(t, key)}
      </li>
    ));
    blocks.push(
      ordered ? (
        <ol key={key++} className="my-1 list-decimal pl-5">
          {items}
        </ol>
      ) : (
        <ul key={key++} className="my-1 list-disc pl-5">
          {items}
        </ul>
      ),
    );
    list = null;
  };

  for (const line of lines) {
    if (line.trim().startsWith("```")) {
      if (inCode) {
        blocks.push(
          <pre
            key={key++}
            className="my-1.5 overflow-x-auto rounded-lg bg-surface-high/50 px-3 py-2 font-mono text-xs text-on-surface-variant"
          >
            {codeLines.join("\n")}
          </pre>,
        );
        codeLines = [];
        inCode = false;
      } else {
        flushPara();
        flushList();
        inCode = true;
      }
      continue;
    }
    if (inCode) {
      codeLines.push(line);
      continue;
    }
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      flushPara();
      flushList();
      const hk = key++;
      blocks.push(<Heading key={hk} level={h[1].length} content={h[2].trim()} k={hk} />);
      continue;
    }
    const ul = /^\s*[-*+]\s+(.*)$/.exec(line);
    const ol = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (ul || ol) {
      flushPara();
      const wantOrdered = !!ol;
      if (list && ordered !== wantOrdered) flushList(); // 列表类型切换：先收尾再开新列表
      if (!list) {
        list = [];
        ordered = wantOrdered;
      }
      list.push((ol?.[1] ?? ul?.[1] ?? "").trim());
      continue;
    }
    if (line.trim() === "") {
      flushPara();
      flushList();
      continue;
    }
    para.push(line.trim());
  }
  if (inCode && codeLines.length) {
    blocks.push(
      <pre
        key={key++}
        className="my-1.5 overflow-x-auto rounded-lg bg-surface-high/50 px-3 py-2 font-mono text-xs text-on-surface-variant"
      >
        {codeLines.join("\n")}
      </pre>,
    );
  }
  flushPara();
  flushList();

  return <div className={`text-sm leading-relaxed text-on-surface ${className}`}>{blocks}</div>;
}

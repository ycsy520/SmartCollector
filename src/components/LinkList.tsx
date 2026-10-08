import type { ExtractedLink } from "../types/ipc";
import { safeHref } from "../lib/url";

export function LinkList({ links }: { links: ExtractedLink[] }) {
  if (links.length === 0)
    return <p className="text-sm text-hint">无链接</p>;
  return (
    <ul className="space-y-1.5">
      {links.map((l) => {
        const href = safeHref(l.url);
        const label = l.text ?? l.url;
        return (
          <li key={l.url} className="truncate text-sm">
            {href ? (
              <a
                href={href}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary hover:underline"
                title={l.url}
              >
                {label}
              </a>
            ) : (
              <span
                className="text-hint"
                title="非 http(s) 协议，已拦截以防 XSS"
              >
                {label}（已拦截）
              </span>
            )}
            {l.text && href && (
              <span className="ml-2 truncate text-xs text-hint">{l.url}</span>
            )}
          </li>
        );
      })}
    </ul>
  );
}

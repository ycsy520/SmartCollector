// 外链协议白名单：仅 http(s) 可作为可点击 href，其余（javascript:/data:/vbscript: 等）一律拒绝。
// 系统边界校验——externalUrl 与 links 来自后端/外部抽取，前端渲染前过滤，杜绝 javascript: 型 XSS。
export function safeHref(url: string | undefined): string | undefined {
  if (!url) return undefined;
  return /^https?:\/\/\S+$/i.test(url.trim()) ? url.trim() : undefined;
}

/**
 * 从粘贴带进来的富文本 HTML 里抽 `<img>` 的图片地址，只留绝对 http(s)（`data:`/相对路径按契约丢弃）。
 * 用 DOMParser 而不是正则：解析 HTML 本就是它的职责，且解出来的文档没有浏览上下文，不会去取图片、不会跑脚本。
 */
export function imageUrlsFromHtml(html: string, cap = 5): string[] {
  if (!html || !/<img\b/i.test(html)) return [];
  const doc = new DOMParser().parseFromString(html, "text/html");
  const out: string[] = [];
  for (const img of doc.querySelectorAll("img")) {
    const url = safeHref(img.getAttribute("src") ?? "");
    if (url && !out.includes(url)) out.push(url);
    if (out.length >= cap) break;
  }
  return out;
}

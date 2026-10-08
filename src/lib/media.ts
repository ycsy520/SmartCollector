// 图片原样收藏（批次21-B，02 §1.4）的前端一侧：取字节 → 阈值 → 压缩 → data URL → `submit_image`；
// 以及把库里的相对文件名拼成能显示的 URL。
//
// 为什么压缩在前端：IPC 没有二进制通道，字节只能以 base64 走 JSON 参数，而主线程序列化大字符串
// 会卡住整个窗口。**压缩是不可逆的**，所以 `keepOriginalImage` 开着时这里一律原样直传。
// 后端 `services/media.rs` 还会再核一次大小与魔数——前端的阈值是体验，那道才是边界。
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauri } from "./invoke";

/** 超过这个字节数就重编码压小（`keepOriginalImage` 关掉时）。 */
export const COMPRESS_OVER_BYTES = 5 * 1024 * 1024;
/** 与后端 `MAX_IMAGE_BYTES` 同值：超过直接拒收，本地磁盘不是云盘。 */
export const MAX_IMAGE_BYTES = 30 * 1024 * 1024;

/** 剪贴板里有没有图（只认第一张）。粘贴来源可以是任何应用，故不看 MIME 只看 type 前缀。 */
export function imageFromDataTransfer(dt: DataTransfer | null): File | null {
  const items = dt?.items;
  if (!items) return null;
  for (const it of Array.from(items)) {
    if (it.kind === "file" && it.type.startsWith("image/")) {
      const f = it.getAsFile();
      if (f) return f;
    }
  }
  return null;
}

const readFile = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const fr = new FileReader();
    fr.onload = () => resolve(String(fr.result));
    fr.onerror = () => reject(new Error("读不到图片字节"));
    fr.readAsDataURL(blob);
  });

/** 重编码：解码 → OffscreenCanvas 缩放/转 JPEG。任一步不支持就退回原字节（不静默丢图）。 */
async function recompress(file: Blob): Promise<Blob | null> {
  if (typeof createImageBitmap !== "function") return null;
  try {
    const bmp = await createImageBitmap(file);
    // 只压尺寸不放大：超长截图压到 2000px 内已足够看清，字节数掉一个量级。
    const maxSide = 2000;
    const scale = Math.min(1, maxSide / Math.max(bmp.width, bmp.height));
    const w = Math.max(1, Math.round(bmp.width * scale));
    const h = Math.max(1, Math.round(bmp.height * scale));
    const canvas = new OffscreenCanvas(w, h);
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.drawImage(bmp, 0, 0, w, h);
    bmp.close?.();
    return await canvas.convertToBlob({ type: "image/jpeg", quality: 0.82 });
  } catch {
    // GIF 动图解码只拿首帧、WebP 边界解码失败等：宁可回传原字节，也不能把用户的图换成静图或报错丢弃。
    return null;
  }
}

/** `prepareImage` 的产物：能直接进 `submit_image` 的载荷 + 给 UI 看的体积口径。 */
export interface ImagePayload {
  /** 落库字节的 data URL 形态（IPC 没有二进制通道，只能这样过界）。 */
  data: string;
  /** 落库的**实际**字节数（压缩后即压缩后的体积），用于如实告知用户。 */
  bytes: number;
  /** 是否被重编码过——开「保留原图」或压缩没变小时为 false。 */
  compressed: boolean;
}

/** 粘贴的图片 → 待收载荷。超 30MB 抛错，由调用方把消息原样显示给用户。 */
export async function prepareImage(file: File, keepOriginal: boolean): Promise<ImagePayload> {
  if (file.size > MAX_IMAGE_BYTES) {
    throw new Error(`图片 ${(file.size / 1024 / 1024).toFixed(1)}MB，超过 30MB 上限`);
  }
  if (!keepOriginal && file.size > COMPRESS_OVER_BYTES) {
    const shrunk = await recompress(file);
    // 压完更大（已高度压缩的 PNG/截图常见）就用原图：压缩只为省体积，不为省而换掉清晰度。
    if (shrunk && shrunk.size < file.size)
      return { data: await readFile(shrunk), bytes: shrunk.size, compressed: true };
  }
  return { data: await readFile(file), bytes: file.size, compressed: false };
}

/** 体积口径给 UI 显示：不足 1MB 用 KB，免得一张 300KB 的图写成 "0.3 MB" 还四舍五入成 "0 MB"。 */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 KB";
  const kb = n / 1024;
  return kb < 1024 ? `${Math.max(1, Math.round(kb))} KB` : `${(kb / 1024).toFixed(1)} MB`;
}

// —— 浏览器预览（非 Tauri）没有 media 目录，字节只能待在内存里：刷新即失，只为看样式演示。
const mockImages = new Map<string, string>();
/** mock 模式登记一张图（key = 与真实模式同形的相对文件名）。 */
export function putMockImage(name: string, dataUrl: string): void {
  mockImages.set(name, dataUrl);
}

/**
 * 库里的 `mediaPath`（相对文件名）→ 可显示的 URL。
 * 真实模式走 asset 协议（scope 见 tauri.conf.json 的 `$APPDATA/media/**`）；
 * mediaDir 未回填（老配置/未初始化）时返回 undefined，由 UI 显示占位而不是裂图。
 */
export function imageSrc(mediaPath: string | undefined, mediaDir: string): string | undefined {
  if (!mediaPath) return undefined;
  if (!isTauri()) return mockImages.get(mediaPath);
  if (!mediaDir) return undefined;
  return convertFileSrc(`${mediaDir}/${mediaPath}`);
}

/**
 * 剥掉正文尾部那行 `【图片】<文件名>` 占位（批次23 的合并行带着它，为的是躲开按正文
 * sha256 生效的活跃去重索引——存储层留着，显示层不该让用户再看一遍文件名）。
 * 只在**有 mediaPath** 时剥：用户自己打了同样一行字、而这条并没有附图，那是正文，得留着。
 * 列表的 excerpt 是正文前 120 字，占位行可能被截断，故按"尾部以【图片】开头的一行"整行去掉。
 */
export function stripImageMarker(text: string, mediaPath: string | undefined): string {
  if (!mediaPath) return text;
  return text.replace(/\n*【图片】[^\n]*$/, "");
}

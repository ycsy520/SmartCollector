// 图片缩略图（批次21-B）：卡片/瓦片/详情共用的"有图放图，没图放占位"那一小块。
// 为什么单独一个组件：那三处的 src 都来自 asset 协议，取不到是常态（文件被外部删掉、scope
// 挡下、配置尚未回填 media 目录），而"裂图"是这里唯一不可接受的失败——破图标看起来像 bug，
// 会让人以为整个库坏了。首页的待收槽不放它：那里的 src 是刚读到的字节本身，
// 显示不出来说明读取就坏了，拿占位框盖住反而骗人。
import { useState } from "react";
import { Icon } from "./ui/icon";

export function MediaThumb({
  src,
  alt,
  imgCls,
  boxCls,
  iconSize = 16,
  title,
}: {
  /** `imageSrc()` 的结果；undefined = 这台设备给不出 URL。 */
  src?: string;
  alt: string;
  /** 图片态的类名（含尺寸、圆角、object-fit）。 */
  imgCls: string;
  /** 占位态的类名（须自带尺寸，否则占位框会塌成一条线）。 */
  boxCls: string;
  iconSize?: number;
  title?: string;
}) {
  const [broken, setBroken] = useState(false);
  if (!src || broken)
    return (
      <span
        aria-hidden
        title={title ?? "图片仍在本机 media 目录里，只是这台设备取不到预览"}
        className={`flex items-center justify-center rounded-lg border border-dashed border-outline-variant text-hint ${boxCls}`}
      >
        <Icon name="Image" size={iconSize} />
      </span>
    );
  return (
    <img
      src={src}
      alt={broken ? "" : alt}
      loading="lazy"
      onError={() => setBroken(true)}
      className={imgCls}
    />
  );
}

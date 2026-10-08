// 设置页「帮助」区：从零上手——是什么 / 三步跑通 / 快捷键 / 三层去向 / 隐私边界 / 常见问题。
// 文案即契约：只写代码兑现得了的能力，未实现的一律不吹——链接自动抓取（submit_link 未做）、
// "AI 学习分拣习惯"（habits 表无生产写入、真机恒空）、技能自动触发（只有按钮/@指令）都不写。
import type { ReactNode } from "react";
import { useSettings } from "../stores/settings";
import { formatShortcut } from "../lib/shortcut";

function Card({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="md-elev rounded-2xl bg-card p-5">
      <h2 className="mb-2 text-xs font-semibold text-hint">{title}</h2>
      {children}
    </section>
  );
}

// 键帽：<kbd> 语义 + 与输入框同源的描边/底色令牌，浅深两态都成立。
function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-block rounded-md border border-outline-variant bg-surface px-1.5 py-0.5 font-mono text-[11px] leading-none text-on-surface">
      {children}
    </kbd>
  );
}

const STEPS: { head: string; body: ReactNode }[] = [
  {
    head: "收集",
    body: (
      <>
        把文本粘进首页输入框（或按
        <span className="mx-1 text-on-surface">收集加速键</span>
        把当前剪贴板填成草稿），按「收进来」或 <Kbd>Ctrl</Kbd>
        <span className="mx-0.5">+</span>
        <Kbd>Enter</Kbd>。
      </>
    ),
  },
  {
    head: "自动整理",
    body: (
      <>
        需先在「大模型配置」填密钥并打开总开关：后台自动分类、打标签、写摘要、提取链接并向量化。
        <span className="text-on-surface">没启用大模型时仍会存原文</span>
        ，只是摘要留空、卡片标「未整理」，日后启用会自动补。
      </>
    ),
  },
  {
    head: "分拣 → 归档 → 检索",
    body: (
      <>
        在「碎片」区逐条放行或丢弃；放行的进「归档库」可被搜到，丢弃的进「回收站」。
      </>
    ),
  },
];

const LAYERS: { head: string; body: string }[] = [
  { head: "碎片（待分拣）", body: "刚收集、等你决定去留的条目。" },
  { head: "归档库", body: "放行后的长期留存，可被关键词与语义检索命中。" },
  { head: "回收站", body: "丢弃后进这里，30 天内可恢复，过期自动彻底删除。" },
];

const PRIVACY: string[] = [
  "不后台监听剪贴板，也不会自动收走你复制过的内容——只有你主动动作时才读一次。",
  "含身份证 / 银行卡 / 手机号 / 口令的内容会被本地检出并标记，不发给模型。",
  "图片按原样收藏，不经模型、不耗 token。",
  "API 密钥存在系统钥匙串，不写进数据库，也不落明文日志。",
];

const FAQ: { q: string; a: ReactNode }[] = [
  {
    q: "摘要怎么是空的？",
    a: "大模型未启用 / 未配密钥、内容被判无需整理、或命中隐私闸门——原文都照常保存，不是丢了。",
  },
  {
    q: "收集加速键按了没反应？",
    a: "到设置→系统看是否与应用内快捷键冲突、或被别的程序占用，可换个键位。",
  },
  {
    q: "删掉的能找回吗？",
    a: "回收站保留 30 天，期内可恢复；过期自动彻底删除，无法找回。",
  },
];

export function HelpSection() {
  const pasteShortcut = useSettings((s) => s.config.pasteShortcut);
  return (
    <div className="space-y-4">
      <Card title="这是什么">
        <p className="text-xs leading-relaxed text-on-surface-variant">
          粘贴即收集的桌面信息库。把内容丢进来，后台大模型自动分类、打标签、摘要、提取链接并向量化；
          之后用关键词或语义在你自己的库里把它找回来。数据始终留在本机。
        </p>
      </Card>

      <Card title="从零开始的三步">
        <ol className="space-y-2">
          {STEPS.map((s, i) => (
            <li key={i} className="flex gap-2 text-xs leading-relaxed text-on-surface-variant">
              <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-secondary-container text-[10px] font-medium text-on-secondary-container">
                {i + 1}
              </span>
              <span>
                <span className="font-medium text-on-surface">{s.head}：</span>
                {s.body}
              </span>
            </li>
          ))}
        </ol>
      </Card>

      <Card title="快捷键">
        <ul className="space-y-1.5 text-xs text-on-surface-variant">
          <li className="flex items-center gap-2">
            <Kbd>Ctrl</Kbd>+<Kbd>Enter</Kbd>
            <span>收集当前输入</span>
          </li>
          <li className="flex items-center gap-2">
            <Kbd>@</Kbd>
            <span>在输入框里附加技能</span>
          </li>
          <li className="flex items-center gap-2">
            <Kbd>{formatShortcut(pasteShortcut)}</Kbd>
            <span>收集加速键：唤起窗口 + 读一次剪贴板填成草稿（可在设置→系统改键）</span>
          </li>
          <li className="flex items-start gap-2">
            <span className="shrink-0">
              <Kbd>J</Kbd>/<Kbd>K</Kbd> · <Kbd>Enter</Kbd> · <Kbd>Space</Kbd>
            </span>
            <span>键盘流分拣：上下移动 / 打开 / 勾选；配合 <Kbd>Shift</Kbd> 连选、<Kbd>Ctrl</Kbd>+<Kbd>A</Kbd> 全选、<Kbd>Delete</Kbd> 丢弃</span>
          </li>
          <li className="flex items-center gap-2">
            <Kbd>Esc</Kbd>
            <span>关闭浮层 / 详情（最小化请用标题栏按钮或托盘，Esc 不最小化）</span>
          </li>
        </ul>
      </Card>

      <Card title="信息的三层去向">
        <dl className="space-y-1.5 text-xs leading-relaxed text-on-surface-variant">
          {LAYERS.map((l) => (
            <div key={l.head} className="flex gap-2">
              <dt className="shrink-0 font-medium text-on-surface">{l.head}</dt>
              <dd className="min-w-0">{l.body}</dd>
            </div>
          ))}
        </dl>
        <p className="mt-2 text-xs leading-relaxed text-hint">
          滞留碎片满 14 天未处理会自动归档，避免堆积。
        </p>
      </Card>

      <Card title="隐私与边界">
        <ul className="list-disc space-y-1.5 pl-4 text-xs leading-relaxed text-on-surface-variant marker:text-hint">
          {PRIVACY.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      </Card>

      <Card title="常见问题">
        <dl className="space-y-2 text-xs leading-relaxed">
          {FAQ.map((f) => (
            <div key={f.q}>
              <dt className="font-medium text-on-surface">{f.q}</dt>
              <dd className="text-on-surface-variant">{f.a}</dd>
            </div>
          ))}
        </dl>
      </Card>
    </div>
  );
}

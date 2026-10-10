// 设置页「关于」区：介绍 / 版本 / 检查更新 / 隐私与数据 / 第三方许可 / 版权与仓库。
// 文案即契约——每条声明都得能被代码兑现，兑现不了的直接写"没有"：
// 无遥测、无云端同步、无自动安装更新，收集全靠用户明示动作。
// 唯一可能的额外出网在这里，且**只在点击时**发一次匿名 GET（进入本页不发任何请求，不轮询）。
import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { AppMark } from "./AppMark";
import { Button } from "./ui/button";
import { Icon } from "./ui/icon";
import { toast } from "./ui/toast";
import { isTauri } from "../lib/invoke";

const REPO = "ycsy520/SmartCollector";
const REPO_URL = `https://github.com/${REPO}`;
const RELEASE_API = `https://api.github.com/repos/${REPO}/releases/latest`;

// 浏览器演示环境读不到 tauri.conf.json 的版本，只能显示这份常量（真窗口下一律取 getVersion()）。
const MOCK_VERSION = "0.1.0";
// 上次检查结果只存本机 localStorage（与主题偏好同一惯例：设备级，不进配置数据库）。
const CHECK_KEY = "sc.updateCheck";

type CheckResult =
  | { kind: "latest"; latest: string }
  | { kind: "newer"; latest: string }
  | { kind: "unreachable"; reason: string };

const CHECK_TEXT: Record<CheckResult["kind"], string> = {
  latest: "已是最新版本",
  newer: "发现新版本",
  unreachable: "无法确认可用更新",
};

// 逐段比数字：GitHub 的 tag 可能是 v1.2.3，本 crate 版本是 1.2.3。
function isNewer(latest: string, current: string): boolean {
  const seg = (v: string) => v.replace(/^v/, "").split(".").map((n) => Number(n) || 0);
  const a = seg(latest);
  const b = seg(current);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) > (b[i] ?? 0);
  }
  return false;
}

// 复制仓库地址。真窗口的 origin 是 http://tauri.localhost，属**非安全上下文**，
// navigator.clipboard 可能整个不存在，所以留一条 execCommand 兜底（它不受安全上下文限制）。
async function copyRepoUrl(): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(REPO_URL);
      return true;
    }
  } catch {
    /* 落到兜底路径 */
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = REPO_URL;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}

function Disclosure({
  label,
  open,
  onToggle,
  children,
}: {
  label: string;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <div className="border-t border-outline-variant/60 pt-2">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="md-press flex w-full items-center gap-1.5 py-1.5 text-left text-sm text-on-surface-variant outline-none hover:text-on-surface focus-visible:ring-2 focus-visible:ring-primary/60"
      >
        <Icon
          name="ChevronDown"
          size={15}
          className={`transition-transform duration-hover ease-emph ${open ? "rotate-180" : ""}`}
        />
        {label}
      </button>
      {open && <div className="caption-in pb-1">{children}</div>}
    </div>
  );
}

const PRIVACY: { head: string; body: string }[] = [
  {
    head: "收集只由你明示触发",
    body: "不做任何后台剪贴板监听或轮询，复制过的内容不会被自动收走。只有输入框粘贴、托盘「收集剪贴板」或页面上的收集按钮才会读取一次剪贴板。",
  },
  {
    head: "内容只存本机",
    body: "片段、标签、摘要与向量全部写在本机的 SQLite 数据库文件里（应用数据目录）。没有云端同步、没有备份上传、没有账号体系。",
  },
  {
    head: "外发只发生在你配置的模型端点",
    body: "分类 / 标签 / 摘要 / 向量化会把正文发给你在「设置 → 大模型」里填写的 Base URL；不配置就一次也不发。除你填的端点外，本应用不向任何服务器提交你的内容。",
  },
  {
    head: "敏感内容由本地规则先拦下",
    body: "疑似密钥、证件号形态的内容由纯本地规则识别，不进入模型上下文，也不做 AI 判断。检测本身不联网。",
  },
  {
    head: "API Key 只进系统凭据管理器",
    body: "密钥写入 Windows 凭据管理器，不落数据库、不写明文日志、界面上只显示尾号。",
  },
  {
    head: "没有遥测",
    body: "无使用统计上报、无崩溃收集、无广告、无埋点。本页「检查更新」是唯一一次需要你亲自点击的额外出网（只问 GitHub 有没有新版本，不带任何本地数据）。",
  },
  {
    head: "删除是真的删除",
    body: "回收站保留 30 天后硬删，「清空回收站」立即硬删；整个数据库文件你也可以随时自行删除。",
  },
];

// 直接依赖（非穷尽）：完整清单以 Cargo.toml / package.json 为准，这里只列到人能看懂的那几项。
const LICENSES: { name: string; use: string; lic: string }[] = [
  { name: "Tauri 2（含 api / plugins）", use: "桌面外壳：窗口、托盘、快捷键、IPC", lic: "MIT / Apache-2.0" },
  { name: "React 19、Zustand 5", use: "界面与前端状态", lic: "MIT" },
  { name: "lucide-react", use: "图标", lic: "ISC" },
  { name: "Tailwind CSS 3.4、Vite 8、TypeScript", use: "样式与构建（不随产物分发）", lic: "MIT / Apache-2.0" },
  { name: "SQLite（rusqlite bundled）+ FTS5", use: "本地数据库与全文检索", lic: "Public Domain" },
  { name: "sqlite-vec", use: "本地向量检索虚表", lic: "MIT" },
  { name: "reqwest + rustls", use: "访问你配置的模型端点", lic: "MIT / Apache-2.0" },
  { name: "keyring", use: "系统凭据管理器存取密钥", lic: "MIT" },
  { name: "arboard", use: "主动收集时读取剪贴板", lic: "MIT" },
  { name: "serde、thiserror、chrono、sha2、uuid、r2d2", use: "序列化、错误、时间、哈希、ID、连接池", lic: "MIT / Apache-2.0" },
];

export function AboutSection() {
  const live = isTauri();
  const [version, setVersion] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<CheckResult | null>(null);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    if (!live) {
      setVersion(MOCK_VERSION);
      return;
    }
    let alive = true;
    getVersion()
      .then((v) => alive && setVersion(v))
      // 读不到版本只影响这一行显示，不能用报错打扰用户，也不能显示假版本号。
      .catch(() => alive && setVersion(null));
    return () => {
      alive = false;
    };
  }, [live]);

  // 上次检查的结果与时间从本机读回（纯本地，不发请求）。
  useEffect(() => {
    try {
      const raw = localStorage.getItem(CHECK_KEY);
      if (!raw) return;
      const saved = JSON.parse(raw) as { at: string; result: CheckResult };
      if (saved?.result?.kind && saved.at) {
        setResult(saved.result);
        setCheckedAt(saved.at);
      }
    } catch {
      /* 存不下或读不到就当没检查过 */
    }
  }, []);

  const remember = (at: string, r: CheckResult) => {
    try {
      localStorage.setItem(CHECK_KEY, JSON.stringify({ at, result: r }));
    } catch {
      /* 隐私模式下写不进 localStorage 也不影响结果展示 */
    }
  };

  const check = async () => {
    setChecking(true);
    let r: CheckResult;
    // 版本号还没读到时不能比：拿 0.0.0 去比会把任何 tag 报成"发现新版本"，是假好消息。
    if (!version) {
      r = { kind: "unreachable", reason: "本机版本号尚未读取完成，无法比较" };
    } else {
      try {
        // 不加自定义请求头：那会把这请求变成需要 CORS 预检的请求。
        const res = await fetch(RELEASE_API, { method: "GET" });
        if (!res.ok) {
          r = {
            kind: "unreachable",
            reason:
              res.status === 404
                ? "该仓库还没有已发布的版本"
                : `GitHub 返回 HTTP ${res.status}`,
          };
        } else {
          const data = (await res.json()) as { tag_name?: unknown };
          const tag = typeof data.tag_name === "string" ? data.tag_name : "";
          r = !tag
            ? { kind: "unreachable", reason: "发布记录里没有版本号" }
            : isNewer(tag, version)
              ? { kind: "newer", latest: tag }
              : { kind: "latest", latest: tag };
        }
      } catch {
        r = { kind: "unreachable", reason: "网络不可达" };
      }
    }
    const at = new Date().toISOString();
    setResult(r);
    setCheckedAt(at);
    setChecking(false);
    remember(at, r);
  };

  const doCopy = async () => {
    if (await copyRepoUrl()) toast("仓库地址已复制，请在浏览器打开", "info");
    else toast("复制失败：本窗口不允许写入剪贴板，请手动选中地址复制", "error");
  };

  const shown = version ?? "版本号读取失败";
  const toggle = (id: string) => setOpen((cur) => (cur === id ? null : id));
  // 存回来的时间戳可能畸形（别的版本写过、手改过 localStorage）：畸形就当没检查过，不画 Invalid Date。
  const checked = new Date(checkedAt ?? NaN);
  const checkedLabel = Number.isNaN(checked.getTime())
    ? null
    : checked.toLocaleString(undefined, { hour12: false });

  return (
    <section className="md-elev rounded-2xl bg-card p-5">
      <h2 className="mb-3 text-xs font-semibold text-hint">关于</h2>

      <div className="flex items-start gap-3">
        <AppMark size={36} className="mt-0.5" />
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-semibold text-on-surface">Smart Collector</span>
            <span className="rounded-full bg-secondary-container px-2 py-0.5 text-[11px] font-medium text-on-secondary-container">
              {shown}
            </span>
            {!live && (
              <span className="rounded-full border border-outline-variant px-2 py-0.5 text-[11px] text-hint">
                浏览器演示环境
              </span>
            )}
          </div>
          <p className="mt-1.5 text-xs leading-relaxed text-on-surface-variant">
            桌面端信息收集工具：粘进来一段文字，后台流水线自动分类、打标签、摘要、提取链接并向量化；
            之后用全文检索与向量混合检索在你自己的库里把它找回来。数据留在本机。
          </p>
        </div>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button variant="text" icon="RefreshCw" onClick={check} disabled={checking}>
          {checking ? "检查中…" : "检查更新"}
        </Button>
        <Button variant="text" icon="Link" onClick={doCopy}>
          复制仓库地址
        </Button>
      </div>

      {result && !checking && (
        <p className="caption-in mt-1 flex items-start gap-1.5 text-xs leading-relaxed text-hint">
          <Icon
            name={result.kind === "newer" ? "Sparkles" : result.kind === "latest" ? "Check" : "Info"}
            size={13}
            className="mt-0.5"
          />
          <span>
            {result.kind === "newer" ? (
              <>
                {CHECK_TEXT.newer}：{result.latest}（当前 {shown}）。本应用没有内置自动下载安装，
                请自行到发布页取用。
              </>
            ) : (
              <>
                {CHECK_TEXT[result.kind]}
                {result.kind === "latest" ? `（当前 ${shown}）` : `：${result.reason}`}。
              </>
            )}
            {checkedLabel && (
              <span className="text-hint"> 上次检查 {checkedLabel}。</span>
            )}
          </span>
        </p>
      )}
      {!result && !checking && (
        <p className="mt-1 text-xs leading-relaxed text-hint">
          「检查更新」只向 GitHub 问一次有没有新版本发布，不携带任何本机数据；
          它不会自动下载或安装任何东西。
        </p>
      )}

      <p className="mt-3 select-all break-all font-mono text-xs text-on-surface-variant">
        {REPO_URL}
      </p>

      <div className="mt-3">
        <Disclosure label="隐私与数据声明" open={open === "privacy"} onToggle={() => toggle("privacy")}>
          <ul className="flex flex-col gap-2 pb-1">
            {PRIVACY.map((p) => (
              <li key={p.head} className="text-xs leading-relaxed">
                <span className="font-semibold text-on-surface">{p.head}</span>
                <span className="mt-0.5 block text-on-surface-variant">{p.body}</span>
              </li>
            ))}
          </ul>
        </Disclosure>

        <Disclosure label="政策与版权" open={open === "policy"} onToggle={() => toggle("policy")}>
          <div className="space-y-2 text-xs leading-relaxed text-on-surface-variant">
            <p>
              <span className="font-semibold text-on-surface">许可。</span>
              本项目以 MIT 许可证开源，源码与许可声明见上方仓库地址的 LICENSE 文件；
              你可以自由使用、修改与再分发，仅需保留原作者版权声明。
            </p>
            <p>
              <span className="font-semibold text-on-surface">责任范围。</span>
              收集到的内容完全由你本人决定与负责；模型输出由第三方服务商生成，可能存在错误，
              归档前请自行核对。删除后不可恢复。
            </p>
            <p>© 2026 ycsy520 · 本项目依 MIT 许可证发布。</p>
          </div>
        </Disclosure>

        <Disclosure label="第三方开源许可" open={open === "licenses"} onToggle={() => toggle("licenses")}>
          <dl className="flex flex-col gap-1.5 pb-1">
            {LICENSES.map((l) => (
              <div key={l.name} className="flex flex-wrap items-baseline gap-x-2 text-xs leading-relaxed">
                <dt className="font-semibold text-on-surface">{l.name}</dt>
                <dd className="text-on-surface-variant">{l.use}</dd>
                <dd className="ml-auto shrink-0 text-hint">{l.lic}</dd>
              </div>
            ))}
          </dl>
          <p className="mt-1 text-xs leading-relaxed text-hint">
            上表为主要直接依赖，非穷尽；完整清单见仓库内 Cargo.toml 与 package.json。
            各组件的原始版权声明随产物分发。
          </p>
        </Disclosure>
      </div>
    </section>
  );
}

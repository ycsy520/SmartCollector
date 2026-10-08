//! Markdown 导出（02 §8 / Keep 对标报告 §5）。**只读**聚合：把库里的片段渲染成 Obsidian 友好的
//! `.md`，每条一个文件 + 一份 `_index.md`（不做巨型单文件：不可 diff、不可增量）。
//!
//! 落点固定 `app_data_dir/exports/<UTC 时间戳>/`——不弹保存对话框（那要引 `tauri-plugin-dialog`，
//! 装依赖需批准），零新依赖。**不做**相关碎片 `[[]]` 双链（默认否，见决策点）。
//!
//! 安全：文件名里的文字来自用户内容，`slug` 只保留字母数字与 `-` `_`（点号与路径分隔符一律剔除，
//! 否则 `..` 能逃出导出目录）；目录名只由我们生成的时间戳构成，用户输入不参与。

use std::collections::HashSet;
use std::path::Path;

use crate::dto::export::ExportOutcome;
use crate::dto::fragment::FragmentDetail;
use crate::error::AppError;

/// 文件名里 excerpt 部分的最大字数（Keep 对标报告 §5 定的 12）。
const SLUG_CHARS: usize = 12;

/// YAML 双引号标量：JSON 字符串是 YAML 的合法子集，借 `serde_json` 转义即可，不引 YAML 依赖。
fn scalar(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn seq(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| scalar(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// 正文首行（导出的标题与文件名都从它派生；批次1 已决定 UI 不再消费 title）。
fn first_line(d: &FragmentDetail) -> String {
    d.content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 标题优先取 AI 摘要首句（导出的是"整理后的价值"），无结果时退回正文首行。
fn heading(d: &FragmentDetail) -> String {
    match d.result.as_ref().and_then(|r| r.summary.split_once(['。', '！', '？', '\n'])) {
        Some((head, _)) if !head.trim().is_empty() => head.trim().to_string(),
        _ => {
            let line = first_line(d);
            let cut = line.chars().take(40).collect::<String>();
            if line.chars().count() > cut.chars().count() {
                format!("{cut}…")
            } else {
                cut
            }
        }
    }
}

/// 只留字母数字（含 CJK）与 `-` `_`；其余（含 `/ \ : .` 与空白）折成分隔 `-`。
/// **点号被剔除是刻意的**：否则 `..` 能成为合法 slug 并让 `dir.join(name)` 逃出导出目录。
/// 空结果回落 `fragment`，保证任何内容都能得到一个安全文件名。
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() || matches!(c, '-' | '_') {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            out.extend(c.to_lowercase());
            pending_dash = false;
            if out.chars().count() >= SLUG_CHARS {
                break;
            }
        } else {
            pending_dash = true;
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '_').to_string();
    if out.is_empty() {
        "fragment".into()
    } else {
        out
    }
}

/// `YYYY-MM-DD-<slug>.md`；同名冲突（不同片段前 12 字相同）时追加短 id 后缀，永不覆盖。
pub fn file_name_for(d: &FragmentDetail, taken: &mut HashSet<String>) -> String {
    // 日期只取数字与 `-`：时间戳由我们自己写，但绝不让冒号（Windows 文件名非法）流进名字。
    let date: String = d
        .created_at
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-')
        .take(10)
        .collect();
    let base = format!("{}-{}", if date.is_empty() { "fragment" } else { &date }, slug(&first_line(d)));
    let mut name = format!("{base}.md");
    if taken.contains(&name) {
        let short: String = d.id.chars().take(8).collect();
        name = format!("{base}-{short}.md");
    }
    // 同一次导出里仍撞车（id 前 8 位也相同的极小概率）：加序号兜底，绝不覆盖已有文件。
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}-{n}.md");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

/// 单条片段的 Markdown 全文（front matter + 正文）。
pub fn render_markdown(d: &FragmentDetail, include_versions: bool) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", scalar(&d.id)));
    out.push_str(&format!("source: {}\n", scalar(&d.source)));
    out.push_str(&format!("mediaType: {}\n", scalar(&d.media_type)));
    // 图片（批次21-B）写**相对 media 目录**的文件名，不写绝对路径：导出包搬到别的机器，
    // 只要连同 media 目录一起搬就还对得上；绝对路径出了这台机器就是废话。
    if let Some(mp) = &d.media_path {
        out.push_str(&format!("mediaPath: {}\n", scalar(mp)));
    }
    out.push_str(&format!("layer: {}\n", scalar(&d.layer)));
    out.push_str(&format!("status: {}\n", scalar(&d.status)));
    if let Some(ab) = &d.archived_by {
        out.push_str(&format!("archivedBy: {}\n", scalar(ab)));
    }
    if let Some(r) = &d.result {
        out.push_str(&format!("category: {}\n", scalar(&r.category)));
        if let Some(sub) = &r.subcategory {
            out.push_str(&format!("subcategory: {}\n", scalar(sub)));
        }
        out.push_str(&format!("tags: {}\n", seq(&r.tags)));
        out.push_str(&format!("links: {}\n", seq(&r.links.iter().map(|l| l.url.clone()).collect::<Vec<_>>())));
        out.push_str(&format!("processedAt: {}\n", scalar(&r.processed_at)));
        if let Some(m) = &r.model_used {
            out.push_str(&format!("modelUsed: {}\n", scalar(m)));
        }
        if r.degraded {
            out.push_str("degraded: true\n");
        }
    }
    out.push_str(&format!("createdAt: {}\n", scalar(&d.created_at)));
    out.push_str(&format!("updatedAt: {}\n", scalar(&d.updated_at)));
    out.push_str("---\n\n");

    out.push_str(&format!("# {}\n\n", heading(d)));
    if let Some(r) = &d.result {
        out.push_str(&format!("## 摘要\n\n{}\n\n", r.summary));
        out.push_str(&format!(
            "## 标签与分类\n\n- 分类：{}\n- 标签：{}\n\n",
            r.category,
            if r.tags.is_empty() { "（无）".into() } else { r.tags.join("、") }
        ));
        if !r.links.is_empty() {
            out.push_str("## 链接\n\n");
            for l in &r.links {
                let text = l.text.clone().unwrap_or_else(|| l.url.clone());
                out.push_str(&format!("- [{}]({})\n", text.replace('[', "(").replace(']', ")"), l.url));
            }
            out.push('\n');
        }
    }
    if let Some(note) = &d.note {
        if !note.trim().is_empty() {
            out.push_str(&format!("## 我的附言\n\n{note}\n\n"));
        }
    }
    out.push_str(&format!("## 原文\n\n{}\n", d.content));
    if include_versions && !d.prior_results.is_empty() {
        out.push_str("\n## 历史版本\n");
        for (i, v) in d.prior_results.iter().enumerate() {
            let model = v.model_used.clone().unwrap_or_else(|| "未知模型".into());
            out.push_str(&format!("\n### v{} · {} · {}\n\n{}\n", i + 1, model, v.processed_at, v.summary));
        }
    }
    out
}

/// 目录页：一行一条，链到刚写出的相对文件名。
pub fn render_index(docs: &[(&str, &FragmentDetail)]) -> String {
    let mut out = String::from("# 导出清单\n\n");
    for (name, d) in docs {
        let cat = d.result.as_ref().map(|r| r.category.clone()).unwrap_or_else(|| "未整理".into());
        // 标题可能含方括号，会截断 md 链接语法，与正文链接同样中和掉。
        let title = heading(d).replace('[', "(").replace(']', ")");
        out.push_str(&format!("- [{title}]({name}) · {cat} · {}\n", d.created_at));
    }
    out
}

/// 写入 `dir`（调用方负责给出 `exports/<时间戳>` 这样的安全路径），返回已写文件名列表。
/// 中途失败即中止并上抛 `E_INTERNAL`——已写出的文件留在原地，不假装成功。
pub fn write_all(dir: &Path, docs: &[FragmentDetail], include_versions: bool) -> Result<ExportOutcome, AppError> {
    let mut taken = HashSet::new();
    let mut files = Vec::with_capacity(docs.len());
    let mut pairs: Vec<(String, FragmentDetail)> = Vec::with_capacity(docs.len());
    for d in docs {
        let name = file_name_for(d, &mut taken);
        std::fs::write(dir.join(&name), render_markdown(d, include_versions))
            .map_err(|e| AppError::Internal(format!("写入导出文件失败：{e}")))?;
        files.push(name.clone());
        pairs.push((name, d.clone()));
    }
    let borrowed: Vec<(&str, &FragmentDetail)> =
        pairs.iter().map(|(n, d)| (n.as_str(), d)).collect();
    std::fs::write(dir.join("_index.md"), render_index(&borrowed))
        .map_err(|e| AppError::Internal(format!("写入导出清单失败：{e}")))?;
    files.push("_index.md".into());
    Ok(ExportOutcome { dir: dir.display().to_string(), files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::fragment::{FragmentResult, ManualFlags};
    use crate::db::results::ExtractedLink;

    fn detail(content: &str) -> FragmentDetail {
        FragmentDetail {
            id: "aaaaaaaa-1111".into(),
            content: content.into(),
            title: None,
            external_url: None,
            source: "manual".into(),
            status: "done".into(),
            created_at: "2026-03-04T09:12:00.000Z".into(),
            updated_at: "2026-03-04T09:13:00.000Z".into(),
            result: None,
            layer: "archived".into(),
            media_type: "text".into(),
            archived_by: None,
            reviewed: true,
            trashed_at: None,
            note: None,
            edit_log: vec![],
            media_path: None,
            prior_results: vec![],
            flags: vec![],
            manual: None,
        }
    }

    fn with_result(mut d: FragmentDetail) -> FragmentDetail {
        d.result = Some(FragmentResult {
            category: "技术".into(),
            subcategory: None,
            tags: vec!["向量".into(), "检索".into()],
            summary: "sqlite-vec 提供 KNN。它的维度由配置决定。\n".into(),
            links: vec![ExtractedLink { url: "https://example.com/a".into(), text: Some("文档 [x]".into()) }],
            degraded: false,
            model_used: Some("deepseek-chat".into()),
            processed_at: "2026-03-04T09:13:00.000Z".into(),
        });
        d.manual = Some(ManualFlags { title: None, category: None, tags: None });
        d
    }

    #[test]
    fn front_matter_and_sections_match_design_shape() {
        let md = render_markdown(&with_result(detail("sqlite-vec 落地记录\n第二行正文")), false);
        assert!(md.starts_with("---\nid: \"aaaaaaaa-1111\"\n"), "{}", md);
        assert!(md.contains("category: \"技术\"\n"));
        assert!(md.contains("tags: [\"向量\", \"检索\"]\n"));
        assert!(md.contains("links: [\"https://example.com/a\"]\n"));
        assert!(md.contains("modelUsed: \"deepseek-chat\"\n"));
        assert!(md.contains("createdAt: \"2026-03-04T09:12:00.000Z\"\n"));
        assert!(md.contains("# sqlite-vec 提供 KNN\n"), "标题取摘要首句");
        assert!(md.contains("## 摘要\n\nsqlite-vec 提供 KNN。它的维度由配置决定。"));
        assert!(md.contains("- 分类：技术\n- 标签：向量、检索"));
        assert!(md.contains("- [文档 (x)](https://example.com/a)"), "链接文案里的方括号被中和");
        assert!(md.ends_with("## 原文\n\nsqlite-vec 落地记录\n第二行正文\n"), "原文原样在末尾");
        assert!(!md.contains("## 历史版本"));
        // 未整理片段（无结果）也要出得来：标题退正文首行、分类区块整段缺席
        let bare = render_markdown(&detail("纯正文没有 AI 结果"), false);
        assert!(bare.contains("# 纯正文没有 AI 结果"));
        assert!(!bare.contains("category:"));
        assert!(!bare.contains("## 摘要"));
    }

    #[test]
    fn image_export_carries_relative_media_path() {
        // 批次21-B：图片只原样收藏。导出带**相对**文件名（绝对路径换台机器就是废话），
        // 而非图片片段不得凭空多出这一行。
        let mut img = detail("【图片】3f2a.png");
        img.media_type = "image".into();
        img.media_path = Some("3f2a.png".into());
        let md = render_markdown(&img, false);
        assert!(md.contains("mediaType: \"image\"\n"));
        assert!(md.contains("mediaPath: \"3f2a.png\"\n"), "{md}");
        let text = render_markdown(&detail("普通正文"), false);
        assert!(text.contains("mediaType: \"text\"\n"));
        assert!(!text.contains("mediaPath:"), "非图片不该有 mediaPath");
    }

    #[test]
    fn note_and_versions_are_opt_in() {
        let mut d = with_result(detail("正文"));
        d.note = Some("我自己补的一句判断".into());
        d.prior_results = vec![FragmentResult {
            category: "技术".into(),
            subcategory: None,
            tags: vec![],
            summary: "第一版摘要".into(),
            links: vec![],
            degraded: true,
            model_used: Some("skill:verify".into()),
            processed_at: "2026-03-04T09:00:00.000Z".into(),
        }];
        let without = render_markdown(&d, false);
        assert!(without.contains("## 我的附言\n\n我自己补的一句判断"));
        assert!(!without.contains("## 历史版本"), "默认只导当前版本");
        let with = render_markdown(&d, true);
        assert!(with.contains("### v1 · skill:verify · 2026-03-04T09:00:00.000Z\n\n第一版摘要"));
    }

    #[test]
    fn slug_neutralises_path_and_odd_content() {
        assert_eq!(slug("../../etc/passwd"), "etc-passwd", "点号与斜杠全折成分隔");
        assert_eq!(slug(""), "fragment");
        assert_eq!(slug("！！！"), "fragment");
        assert_eq!(slug("  中文 标题 测试  "), "中文-标题-测试");
        assert_eq!(slug("a".repeat(80).as_str()).chars().count(), SLUG_CHARS);
        // 文件名里绝不含分隔符与冒号（Windows 冒号非法）
        let mut taken = HashSet::new();
        let name = file_name_for(&detail("../../evil/x:名字"), &mut taken);
        assert!(name.starts_with("2026-03-04-"), "{name}");
        assert!(!name.contains('/') && !name.contains('\\') && !name.contains(':'), "{name}");
    }

    #[test]
    fn file_names_are_unique_within_one_export() {
        let mut taken = HashSet::new();
        // 两条正文的前 12 字完全相同（slug 截断点之后才分岔），必须靠短 id 后缀分开
        let a = file_name_for(&detail("同样的开头文字前十二个字第一条内容"), &mut taken);
        let mut b = detail("同样的开头文字前十二个字第二条内容");
        b.id = "bbbbbbbb-2222".into();
        let b = file_name_for(&b, &mut taken);
        assert_ne!(a, b, "前 12 字相同必须加后缀");
        assert!(b.ends_with("-bbbbbbbb.md"), "{b}");
    }

    #[test]
    fn write_all_emits_one_file_per_fragment_plus_index() {
        let dir = std::env::temp_dir().join(format!("sc-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut second = detail("第二条正文");
        second.id = "cccccccc-3333".into();
        let docs = vec![with_result(detail("第一条正文")), second];
        let out = write_all(&dir, &docs, false).unwrap();
        assert_eq!(out.files.len(), 3, "两条 + _index.md");
        assert_eq!(out.files[2], "_index.md");
        let first = std::fs::read_to_string(dir.join(&out.files[0])).unwrap();
        assert!(first.contains("id: \"aaaaaaaa-1111\""));
        let index = std::fs::read_to_string(dir.join("_index.md")).unwrap();
        assert!(index.contains(&format!("- [sqlite-vec 提供 KNN]({}) · 技术", out.files[0])), "{index}");
        assert!(index.contains(&format!("- [第二条正文]({}) · 未整理", out.files[1])), "无结果的不冒充有分类");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

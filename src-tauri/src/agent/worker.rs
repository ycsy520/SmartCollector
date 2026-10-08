//! worker：后台排空 `fragment_status` 队列 → 跑 `AgentPipeline` → 写 `processing_results`
//! → 落向量（sqlite-vec，尽力而为）→ 更新状态机 → 发事件（02 §0、03 §2/§5）。
//! 兼任全应用唯一的后台常驻循环：顺带跑低频的 14 天超时归档扫描（02 §2「非命令」），
//! 不再另起线程（奥卡姆）。
//!
//! 运行在**独立 std 线程**，绝不用 `tauri::async_runtime::spawn`：pipeline 经 `HttpTransport`
//! （reqwest blocking，内部会建自己的 tokio 运行时），若跑在 tauri 的 async 线程上会触发
//! nested-runtime panic。纯 std 线程 + 固定轮询即可，无新依赖（奥卡姆）。
//!
//! 分层：`process_one`（纯逻辑，注入 `&dyn LlmClient` + 结果回调，可离线单测）
//! 与 `run_loop`/`drain_once`（真实网络装配 + 事件发射）分离，沿用全仓「协议 ↔ 传输」可测边界。

use chrono::{SecondsFormat, Utc};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

use super::{AgentPipeline, LlmClient};
use crate::config::{secrets, AppConfig};
use crate::db::{config_store, fragments, jobs, results, usage::DbUsageRecorder, Pool};
use crate::events::{
    FragmentPurged, FragmentStatus, FragmentUpdated, FRAGMENT_PURGED, FRAGMENT_STATUS,
    FRAGMENT_UPDATED,
};
use crate::llm::openai::HttpTransport;
use crate::llm::{LlmRouter, RouterClient};
use crate::vector::embedding::{Embedder, HttpEmbeddingTransport, OpenAiEmbedder};
use crate::vector::sqlite_vec::SqliteVecStore;
use crate::vector::VectorStore;

/// 空闲轮询间隔。后台排空对秒级延迟无感，取值兼顾响应与空转开销。
const POLL_MS: u64 = 800;
/// 单批领取上限（03 §5 ①，`claim_next` 内部再 clamp 1..=50）。
const BATCH: usize = 5;
/// 租约时长（秒）。须大于单片段最坏处理耗时（4 次 chat + 1 次 embed，各含超时/退避），
/// 取 5 分钟留足余量；崩溃遗留的 running 由 `reclaim_expired_leases` 收回。
const LEASE_SECS: i64 = 300;
/// 超时归档扫描间隔。14 天阈值对小时级延迟无感，取值只需保证「开机即扫、此后每半天一次」。
const SWEEP_MS: u64 = 6 * 60 * 60 * 1000;

/// chat 主端点已配置、总开关打开、且能取到密钥，才排空队列；否则保持 pending（不写入全兜底的垃圾结果）。
/// `llm_enabled` 是用户可在设置里随时翻的总闸：关掉 = 复用"没密钥"的同一条降级路径（不领任务），
/// 但绝不清空已配端点/密钥，打开即刻恢复。刻意放在此闸门最前，令 worker/command/检索三处口径一致。
fn llm_ready(cfg: &AppConfig) -> bool {
    cfg.llm_enabled && cfg.has_primary_llm() && secrets::api_key_for(&cfg.llm_primary.provider).is_some()
}

/// 供 lib.rs setup 调用：把排空循环挂到独立线程，随进程存活（关窗即随 main 退出，崩溃靠租约收回）。
pub fn spawn(pool: Pool, app: AppHandle) {
    std::thread::spawn(move || run_loop(pool, app));
}

fn run_loop(pool: Pool, app: AppHandle) {
    // blocking 客户端在线程起始处构建一次并复用（内含连接池）；此处已是纯 std 线程，无嵌套运行时风险。
    let chat = HttpTransport::new();
    let embed_t = HttpEmbeddingTransport::new();
    let worker_id = format!("worker-{}", uuid::Uuid::new_v4());
    // 首轮即扫（离线 14 天的条目不该等到下次心跳才归位）。刻意用"下次扫描时刻"而非
    // `now - 周期`：`Instant` 是开机单调时钟，开机不足 6 小时时回拨会溢出 panic、worker 线程静默死亡。
    let mut next_sweep = std::time::Instant::now();
    loop {
        if let Ok(conn) = pool.get() {
            if std::time::Instant::now() >= next_sweep {
                next_sweep = std::time::Instant::now() + std::time::Duration::from_millis(SWEEP_MS);
                sweep_expired(&conn, &app);
                // 30 天到期清除：6 小时一次的节奏对"数据真的没了"这件事足够，且绝不在每轮空转里做。
                sweep_purge(&pool, &conn, &app);
            }
            drain_once(&conn, &pool, &app, &worker_id, &chat, &embed_t);
        }
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
    }
}

/// 超时归档（02 §2「非命令」）：14 天无人分拣即替你收。放在 `drain_once` 的 LLM 闸门**之前**，
/// 未配置模型/密钥时也必须能归档——它纯本地 UPDATE，零 token。失败静默：谓词幂等，下次扫描重来。
fn sweep_expired(conn: &Connection, app: &AppHandle) {
    let cutoff = crate::db::iso_days_ago(fragments::AUTO_ARCHIVE_DAYS);
    if let Ok(ids) = fragments::auto_archive_expired(conn, &cutoff) {
        for id in ids {
            let _ = app.emit(FRAGMENT_UPDATED, FragmentUpdated { fragment_id: id });
        }
    }
}

/// 到期清除（02 §2「非命令」）：回收站躺过 30 天的条目**物理**删除，令页面上的倒计时成为事实。
/// 与 14 天归档同为"不惩罚遗忘"的一面，但这条不可逆，故只在明确进过回收站的 layer='trash' 上动手。
/// 失败静默：谓词幂等，下次扫描重来。放在 LLM 闸门之前（纯本地，零 token）。
fn sweep_purge(pool: &Pool, conn: &Connection, app: &AppHandle) {
    if let Ok(ids) = crate::services::purge::purge_expired(pool, conn) {
        for id in ids {
            let _ = app.emit(FRAGMENT_PURGED, FragmentPurged { fragment_id: id });
        }
    }
}

/// 领取并处理一批。崩溃恢复常开（未配置也收回过期租约，令其回到 pending 等下次配置）；
/// 领取前设未配置闸门：无 key/无模型 → 不领任务、不产结果。
fn drain_once(
    conn: &Connection,
    pool: &Pool,
    app: &AppHandle,
    worker_id: &str,
    chat: &HttpTransport,
    embed_t: &HttpEmbeddingTransport,
) {
    let _ = jobs::reclaim_expired_leases(conn);

    let cfg = match config_store::load(conn) {
        Ok(c) => c,
        Err(_) => return,
    };
    if !llm_ready(&cfg) {
        return;
    }

    let lease = (Utc::now() + chrono::Duration::try_seconds(LEASE_SECS).unwrap())
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let claimed = match jobs::claim_next(conn, worker_id, &lease, BATCH) {
        Ok(v) => v,
        Err(_) => return,
    };
    if claimed.is_empty() {
        return;
    }

    // 计量去向（批次17）：chat 四任务与结果编码都记账；持池而非连接，因为要跨 `with_usage` 的 't。
    let recorder = DbUsageRecorder { pool: pool.clone() };
    let api_key = secrets::api_key_for(&cfg.llm_primary.provider).unwrap_or_default();
    let fallback_key = cfg
        .llm_fallback
        .as_ref()
        .and_then(|e| secrets::api_key_for(&e.provider));
    let router = LlmRouter::new(
        chat,
        cfg.llm_primary.clone(),
        cfg.llm_fallback.clone(),
        api_key,
        fallback_key,
        cfg.agent_retry.clone(),
    )
    .with_usage(&recorder);
    // pipeline 内四任务共用一个 client：client 挂的 task 只定**超时档位**（取 Medium 给长文留余量），
    // 计量归属按各任务自带的名（classify/tag/extract_links/summarize）记，见 LlmClient::complete。
    let client = RouterClient { router: &router, task: "summarize" };

    // 向量落库尽力而为：embedding 供应商独立于 chat，缺配或失败只降级检索，不影响处理完成。
    let mut embed_store: Option<(SqliteVecStore, OpenAiEmbedder)> = build_vec_store(pool, &cfg)
        .map(|store| {
            let key = secrets::api_key_for(&cfg.llm_embedding.provider).unwrap_or_default();
            (store, OpenAiEmbedder::new(cfg.llm_embedding.clone(), key, embed_t).with_usage(&recorder))
        });

    for id in claimed {
        let _ = app.emit(
            FRAGMENT_STATUS,
            FragmentStatus { fragment_id: id.clone(), status: "running", error_code: None },
        );
        let mut on_result = |fid: &str, content: &str, version: i64| {
            if let Some((store, embedder)) = embed_store.as_mut() {
                if let Ok(vec) = embedder.embed(content) {
                    let _ = store.upsert(fid, version, &cfg.llm_embedding.model, &vec);
                }
            }
        };
        let (status, error_code) = process_one(conn, &id, &client, &mut on_result);
        let _ = app.emit(
            FRAGMENT_STATUS,
            FragmentStatus { fragment_id: id.clone(), status: status.as_str(), error_code },
        );
        if status == jobs::Status::Done {
            let _ = app.emit(FRAGMENT_UPDATED, FragmentUpdated { fragment_id: id.clone() });
        }
    }
}

/// embedding 端点配置齐全（有模型 + 有 key）时构建向量库，否则 None（检索降级）。
/// 闸门与 command 层语义路共用 `vector::semantic::embedding_ready`，避免两处判定漂移。
fn build_vec_store(pool: &Pool, cfg: &AppConfig) -> Option<SqliteVecStore> {
    if !crate::vector::semantic::embedding_ready(cfg) {
        return None;
    }
    SqliteVecStore::new(pool.clone(), &cfg.llm_embedding.model, cfg.llm_embedding.dim as usize).ok()
}

/// 领取后处理单个片段：读取正文 → 跑 pipeline → 写结果 → 触发向量回调 → running→done。
/// 任何硬错误（读库失败 / 写结果失败）走 `mark_failed`，交队列自动重试；返回新状态与错误码。
/// `on_result(fragment_id, content, version)` 在结果成功落库后调用（worker 用于落向量，测试用桩验证）。
fn process_one(
    conn: &Connection,
    fragment_id: &str,
    client: &dyn LlmClient,
    on_result: &mut dyn FnMut(&str, &str, i64),
) -> (jobs::Status, Option<&'static str>) {
    let frag = match fragments::get(conn, fragment_id) {
        Ok(Some(f)) => f,
        // 已软删（无正文可处理）：释放租约置 done，不留 running 悬挂。
        Ok(None) => {
            let _ = jobs::mark_done(conn, fragment_id);
            return (jobs::Status::Done, None);
        }
        Err(_) => return fail(conn, fragment_id, "E_DB_READ"),
    };
    // 批次21-B 的最后一道闸：图片那行"正文"只是一行占用文件名，发给模型必然换来一篇
    // 凭空编造的摘要（比空结果更糟，且界面分辨不出）。录入路径本就落 skipped（领取查询
    // WHERE status='pending' 命不中），这里守的是 insert→mark_skipped 之间那个窗口，
    // 以及任何把图片状态改回 pending 的意外——外发一旦发出就收不回，故在唯一的出口再拦一次。
    if frag.media_type == "image" {
        let _ = jobs::skip_claimed(conn, fragment_id);
        return (jobs::Status::Skipped, None);
    }
    // 隐私闸门（§2.2）的最后一道复查。录入路径本就落 skipped，这里守的是**重试**那条路：
    // `retry_fragment` 允许 skipped→pending，若只在前端提示、不在出口拦，一键就能把身份证
    // 原文推给模型和 embedding。认的是 `outbound_ok` 列——命令参数活不到这个异步出口，
    // 用户的明示授权必须落库才在这里认得出（没授权才 skip，有授权就照常放行）。
    if crate::db::triage::detect_sensitive(&frag.content).is_some()
        && !jobs::outbound_allowed(conn, fragment_id)
    {
        let _ = jobs::skip_claimed(conn, fragment_id);
        return (jobs::Status::Skipped, None);
    }
    let out = AgentPipeline::run(client, &frag.content);
    match results::insert_result(
        conn,
        fragment_id,
        &out.category,
        out.subcategory.as_deref(),
        &out.tags,
        &out.summary,
        &out.links,
        out.degraded,
        Some(&out.model_used),
    ) {
        Ok(res) => {
            on_result(fragment_id, &frag.content, res.version);
            let _ = jobs::mark_done(conn, fragment_id);
            (jobs::Status::Done, None)
        }
        Err(e) => fail(conn, fragment_id, e.code()),
    }
}

/// 标记失败（未超上限自动回 pending，超限置 failed）；mark_failed 自身出错则按 failed 报告。
fn fail(conn: &Connection, fragment_id: &str, code: &'static str) -> (jobs::Status, Option<&'static str>) {
    let status = jobs::mark_failed(conn, fragment_id, code).unwrap_or(jobs::Status::Failed);
    (status, Some(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_util::scripted_seq;
    use crate::db::fragments::NewFragment;
    use crate::db::jobs::Status;
    use crate::vector::MemoryStore;
    use std::cell::RefCell;

    fn add(conn: &Connection, text: &str) -> String {
        fragments::insert(
            conn,
            &NewFragment { content: text, title: None, source: "manual", external_url: None, media_type: None, note: None, media_path: None, id: None },
        )
        .unwrap()
        .id
    }

    #[test]
    fn llm_ready_false_for_unconfigured_default() {
        // 默认配置无模型无 base_url → 未就绪，worker 应空转不污染队列。
        assert!(!llm_ready(&AppConfig::default()));
    }

    // 总开关必须被 worker 闸门实际读到（非惰性键）：配齐模型+密钥时就绪，关掉开关立即不领任务。
    #[test]
    fn llm_ready_respects_master_switch() {
        let mut cfg = AppConfig::default();
        cfg.llm_primary.provider = "sc_worker_test".into();
        cfg.llm_primary.base_url = "https://api.example.com/v1".into();
        cfg.llm_primary.model = "m".into();
        std::env::set_var("SC_WORKER_TEST_API_KEY", "sk-placeholder-not-real");
        assert!(llm_ready(&cfg), "配齐+开关开=就绪");
        cfg.llm_enabled = false;
        assert!(!llm_ready(&cfg), "关总闸=不领任务，密钥/配置原样留着");
        std::env::remove_var("SC_WORKER_TEST_API_KEY");
    }

    #[test]
    fn process_one_writes_result_and_fires_callback() {
        let conn = crate::db::test_conn();
        let id = add(
            &conn,
            "sqlite-vec 让向量检索跑在 SQLite 里，不必另起服务进程。\
             落地要关心虚表维度、换模型后全量重建、向量与结果版本的一致性；\
             Windows 上还先要确认 MSVC 工具链能把扩展编过，否则整条向量路都起不来。",
        );
        // 领取，置 running（process_one 要求 running 才能 mark_done）
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();

        let client = scripted_seq(vec![
            r#"{"ok":true,"data":{"category":"技术","subcategory":"数据库"}}"#.into(),
            r#"{"ok":true,"data":{"tags":["向量","SQLite"]}}"#.into(),
            r#"{"ok":true,"data":{"summary":"sqlite-vec 摘要"}}"#.into(),
            r#"{"ok":true,"data":{"links":[]}}"#.into(),
        ]);

        let mut store = MemoryStore::new();
        let mut seen: Option<(String, i64)> = None;
        let (status, err) = process_one(&conn, &id, &client, &mut |fid, content, version| {
            let vec = vec![0.1f32; 3];
            assert!(!content.is_empty());
            store.upsert(fid, version, "m", &vec).unwrap();
            seen = Some((fid.to_string(), version));
        });

        assert_eq!(status, Status::Done);
        assert_eq!(err, None);
        assert_eq!(seen.as_ref().map(|(f, _)| f.as_str()), Some(id.as_str()));
        assert_eq!(seen.as_ref().unwrap().1, 1); // 首个结果版本
        // 状态机已落 done
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(Status::Done));
        // 结果已写库
        let latest = results::latest(&conn, &id).unwrap().unwrap();
        assert_eq!(latest.category, "技术");
        assert_eq!(latest.summary, "sqlite-vec 摘要");
        // 向量回调确实写入 store
        assert!(!store.search(&vec![0.1; 3], 5).unwrap().is_empty());
    }

    #[test]
    fn process_one_releases_lease_when_fragment_gone() {
        let conn = crate::db::test_conn();
        let id = add(&conn, "会被删除的片段");
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();
        fragments::soft_delete(&conn, &id).unwrap(); // get 将返回 None

        let client = scripted_seq(vec![]);
        let mut called = false;
        let (status, _) = process_one(&conn, &id, &client, &mut |_f, _c, _v| called = true);
        assert_eq!(status, Status::Done);
        assert!(!called); // 无正文则不触发结果回调
    }

    // 批次21-B：图片是"原样收藏、不经模型"。即便某条图片行不知为何被排进队列（录入路径
    // insert→mark_skipped 之间的窗口、或将来有人改了状态），也只能落 skipped，绝不外发——
    // 拿一行占位文件名去问模型，得到的必然是编造的摘要。
    #[test]
    fn process_one_never_sends_an_image_to_the_model() {
        let conn = crate::db::test_conn();
        let id = fragments::insert(
            &conn,
            &NewFragment {
                content: "【图片】3f2a-9c1e.png",
                title: None,
                source: "manual",
                external_url: None,
                media_type: Some("image"),
                note: None,
                media_path: Some("3f2a-9c1e.png"),
                id: None,
            },
        )
        .unwrap()
        .id;
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();

        // 空脚本：任何一次 complete() 都会拿到空串并走兜底，故用"有没有结果行"当外发证据。
        let client = scripted_seq(vec![]);
        let mut called = false;
        let (status, _) = process_one(&conn, &id, &client, &mut |_f, _c, _v| called = true);
        assert_eq!(status, Status::Skipped);
        assert!(!called);
        assert!(results::latest(&conn, &id).unwrap().is_none(), "图片不得产生任何 AI 结果");
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(Status::Skipped));
    }

    // 审查 P0-1（§2.2）：入队闸门"有入口无出口"的那道缺口，最后一道闸在 worker。
    // 它只认库里的授权位，绝不信"能被领取 = 已授权"——领取条件里没有隐私这一项。
    #[test]
    fn process_one_refuses_sensitive_without_authorization() {
        let conn = crate::db::test_conn();
        let id = add(&conn, "有事打我 13800138000 这台是备用机");
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();

        let client = scripted_seq(vec![]);
        let mut called = false;
        let (status, _) = process_one(&conn, &id, &client, &mut |_f, _c, _v| called = true);
        assert_eq!(status, Status::Skipped);
        assert!(!called);
        assert!(results::latest(&conn, &id).unwrap().is_none(), "未授权的隐私文本不得产生任何 AI 结果");
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(Status::Skipped));
    }

    #[test]
    fn process_one_honors_explicit_authorization() {
        let conn = crate::db::test_conn();
        // 正文要超过 MIN_SUMMARIZE_CHARS(100)，否则摘要任务本就跳过模型，四段脚本会错位。
        let id = add(
            &conn,
            "这台备用机平时只用来收验证码，有事直接打我 13800138000 就行；工作机在会议时段一律静音，\
             找不到人时先走企业微信。另外这张卡的有效期和校验码记在同一页笔记里，翻到就要顺手更新，\
             别只改了手机号忘了改卡号，去年就因为漏改被扣了一笔年费，教训挺深的，以后每季度核对一次。",
        );
        // 用户在弹窗里确认过的那次重试留下的授权位（retry 走的是同一列，这里直接置位）
        conn.execute(
            "UPDATE fragment_status SET outbound_ok = 1 WHERE fragment_id = ?1",
            rusqlite::params![&id],
        )
        .unwrap();
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();

        let client = scripted_seq(vec![
            r#"{"ok":true,"data":{"category":"技术","subcategory":"数据库"}}"#.into(),
            r#"{"ok":true,"data":{"tags":["联系方式"]}}"#.into(),
            r#"{"ok":true,"data":{"summary":"备用机号码"}}"#.into(),
            r#"{"ok":true,"data":{"links":[]}}"#.into(),
        ]);
        let (status, _) = process_one(&conn, &id, &client, &mut |_f, _c, _v| {});
        assert_eq!(status, Status::Done, "明示授权后按正常流程处理");
        assert_eq!(results::latest(&conn, &id).unwrap().unwrap().summary, "备用机号码");
    }

    #[test]
    fn process_one_degraded_still_done() {
        let conn = crate::db::test_conn();
        let id = add(&conn, "正文在此。这是第二句，用来喂抽取式兜底。");
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();
        // 模型全程输出非 JSON → 各任务兜底，pipeline 仍产出 done（degraded），交用户手动重试
        let client = scripted_seq(vec!["not json".into()]);
        let called = RefCell::new(false);
        let (status, err) = process_one(&conn, &id, &client, &mut |_f, _c, _v| {
            *called.borrow_mut() = true;
        });
        assert_eq!(status, Status::Done);
        assert_eq!(err, None);
        assert!(*called.borrow());
        let latest = results::latest(&conn, &id).unwrap().unwrap();
        assert!(latest.degraded);
        assert_eq!(latest.category, "其他");
    }

    /// P8 端到端验收（chat 侧）：真调 DeepSeek 跑完整 pipeline 经 `process_one` 落结果。
    /// 需真实密钥，默认忽略。跑法：`.env` 填 `DEEPSEEK_API_KEY` 后
    /// `cargo test --lib -- --ignored real_process_one_writes_pipeline_result --nocapture`。
    /// （向量落库的真链路已由 `sqlite_vec::real_embed_store_search_qwen` 单独覆盖。）
    #[test]
    #[ignore = "需要 DEEPSEEK_API_KEY 真实密钥与网络"]
    fn real_process_one_writes_pipeline_result() {
        crate::config::secrets::load_dotenv();
        let cfg = AppConfig::deepseek();
        let key = secrets::api_key_for(&cfg.llm_primary.provider)
            .expect("请先在 .env 设置 DEEPSEEK_API_KEY");
        let conn = crate::db::test_conn();
        let id = add(
            &conn,
            "Tauri 2 把权限模型改成 capability 白名单，ipc 与 core 拆分，升级时要迁移 capabilities 目录。\
             另外系统托盘、全局快捷键与单实例这几条插件也要在 default.json 里逐条授权，\
             漏一条就表现为按钮点了没反应而不是报错。",
        );
        jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();

        let t = HttpTransport::new();
        let router = LlmRouter::new(&t, cfg.llm_primary.clone(), None, key, None, cfg.agent_retry.clone());
        let client = RouterClient { router: &router, task: "summarize" };

        let (status, err) = process_one(&conn, &id, &client, &mut |_f, _c, _v| {});
        assert_eq!(status, Status::Done);
        assert_eq!(err, None);
        let r = results::latest(&conn, &id).unwrap().unwrap();
        assert!(!r.summary.trim().is_empty(), "摘要不应为空");
        println!("worker e2e -> category={} degraded={} summary={}", r.category, r.degraded, r.summary);
    }
}

//! 彻底删除（02 §2.8 `purge_fragment` 命令 + §2「非命令」30 天到期清除）。
//!
//! 为什么要单独一层：一条片段的数据**分处两处存储**——主库（fragments/FTS/墓碑 + CASCADE 的关联行）
//! 与 sqlite-vec 虚表，批次21-B 起还多第三处：磁盘上的图片文件。只做 SQL 删除会留下孤儿向量
//! 与孤儿图片（虚表和文件无人清理），只清非主数据则主数据还在。故本层唯一的职责是"三处都删干净"，
//! SQL 在 `db::fragments`，向量在 `vector::semantic`，文件在 `services::media`。

use rusqlite::Connection;

use crate::db::{config_store, fragments, Pool};
use crate::error::AppError;
use crate::services::media;
use crate::vector::semantic;
use crate::vector::VectorStore;

/// 无痕删除一条：主表行 + FTS + 墓碑 + CASCADE 关联行，再补删向量行与图片文件。
/// 向量清理失败**静默跳过**（不阻断）：主数据已不可恢复，虚表里那条要么下次全量重建时被收走，
/// 要么因缺 embedding 配置而本就不存在。报错只会让"删除"这个动作半途而废，更糟。
/// 图片文件同理：删不掉（已被手工移走/权限异常）也不回头报错——库里行已经没了，
/// 为一个清不掉的副产品把"彻底删除"做成失败，用户只会看到一个既没删干净又拒绝承认的状态。
pub fn purge_one(pool: &Pool, conn: &Connection, id: &str) -> Result<(), AppError> {
    // 先问出文件名：行一旦删掉就再也问不出来了（media_path 只存在主表）。
    let name = fragments::get(conn, id)?.and_then(|f| f.media_path);
    fragments::purge(conn, id)?;
    purge_vector(pool, conn, id);
    purge_media_file(name.as_deref());
    Ok(())
}

/// 到期清除（02 §2「非命令」）：回收站躺过 30 天的条目无痕删除，返回被清除的 id。
/// 先取 id 再逐条清向量与文件——SQL 删除已在 `fragments::purge_expired` 内完成，
/// 它把文件名一并带出来，正因如此不需要本层再扫一次库（两处到期谓词会漂移）。
pub fn purge_expired(pool: &Pool, conn: &Connection) -> Result<Vec<String>, AppError> {
    let cutoff = crate::db::iso_days_ago(fragments::PURGE_AFTER_DAYS);
    let rows = fragments::purge_expired(conn, &cutoff)?;
    for (id, name) in &rows {
        purge_vector(pool, conn, id);
        purge_media_file(name.as_deref());
    }
    Ok(rows.into_iter().map(|(id, _)| id).collect())
}

/// 删图片文件。媒体目录未初始化（理论上只在测试里）就跳过——不知在哪，总好过乱删。
fn purge_media_file(name: Option<&str>) {
    let Some(name) = name else { return };
    if let Ok(dir) = media::media_dir() {
        media::delete(&dir, name);
    }
}

/// 有向量库可删时才建句柄：配置里有 embedding 模型与维度即可（删除不联网，**不要求 API key**）。
/// 缺配置=库里没有向量，直接跳过。也供批次6-①「编辑正文后删旧向量」复用——同为"不外发、只清账"。
pub(crate) fn purge_vector(pool: &Pool, conn: &Connection, id: &str) {
    let Ok(cfg) = config_store::load(conn) else { return };
    if let Some(mut store) = semantic::purge_handle(pool, &cfg) {
        let _ = store.delete(id);
    }
}

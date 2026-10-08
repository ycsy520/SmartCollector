//! 大模型使用统计 command（02 §7.5）。纯读，不改状态、不发事件。
//! 单层直连 db 层 `usage::stats`：本模块只做窗口参数的校验与钳制（commands 层职责）。
use rusqlite::Connection;
use tauri::State;

use crate::db::usage;
use crate::dto::usage::UsageStats;
use crate::error::AppError;
use crate::state::AppState;

/// 未传 `range_days` 时的默认窗口（一个月，够看趋势又不至于把首次使用前的老数据全灌进来）。
const DEFAULT_DAYS: i64 = 30;
/// 窗口上限：一年。前端下拉不会超此值，钳制只为防 invoke 被手改后拉全表算崩。
const MAX_DAYS: i64 = 365;

/// 02 §7.5：`range_days` = `None`→30、`0`→全部历史、其余 clamp 1..=365。
pub(crate) fn get_usage_stats_impl(conn: &Connection, range_days: i64) -> Result<UsageStats, AppError> {
    usage::stats(conn, range_days)
}

fn normalize(range_days: Option<i64>) -> i64 {
    match range_days.unwrap_or(DEFAULT_DAYS) {
        0 => 0,
        n if n < 0 => DEFAULT_DAYS,
        n => n.clamp(1, MAX_DAYS),
    }
}

#[tauri::command]
pub fn get_usage_stats(app: State<'_, AppState>, range_days: Option<i64>) -> Result<UsageStats, AppError> {
    let days = normalize(range_days);
    let conn = app.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
    get_usage_stats_impl(&conn, days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_days_normalization() {
        assert_eq!(normalize(None), DEFAULT_DAYS);
        assert_eq!(normalize(Some(0)), 0, "0 必须原样透传，代表全部历史");
        assert_eq!(normalize(Some(7)), 7);
        assert_eq!(normalize(Some(10_000)), MAX_DAYS);
        assert_eq!(normalize(Some(-3)), DEFAULT_DAYS);
    }
}

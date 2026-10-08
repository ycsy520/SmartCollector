//! 共享状态。P7 先落 DbPool（command 依赖）；P8 装配时再扩
//! Mutex<AppConfig> 内存态、AgentPipeline、剪贴板句柄、后台 worker JoinHandle。
use crate::db::Pool;

pub struct AppState {
    pub db: Pool,
}

// 前后端契约 DTO：均 derive(Serialize, Deserialize)。
// TODO(P7): 字段与 02 清单逐字段一致，前端 src/types/ipc.ts 与之手工同步。
pub mod export;
pub mod fragment;
pub mod search;
pub mod settings;
pub mod usage;

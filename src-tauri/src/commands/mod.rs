// Command 层：薄封装，只做参数校验 + DTO 转换，不含业务逻辑（业务在 db / agent）。
// `#[tauri::command]` 生成的 `__cmd__*` 项随函数定义在各子模块，故 lib.rs 的 invoke_handler
// 按 `commands::<子模块>::<命令>` 路径引用（`pub use` 只转发函数名、不转发宏项）。
pub mod curation;
pub mod export;
pub mod fragment;
pub mod media;
pub mod search;
pub mod settings;
pub mod usage;

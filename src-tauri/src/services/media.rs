//! 图片落盘（批次21-B，02 §1.4）：收藏的图片只**原样存文件**，不做任何识别。
//!
//! 为什么单独一层：一条图片片段的数据**分处两处**——主库那行占位（`media_path` 存相对文件名）
//! 与磁盘上的真文件。DB 层不该碰文件系统（保持单测脱离 IO），command 层不该懂编码与格式判定，
//! 故"字节 → 文件名 → 落盘/清除/计量"全收在这里。
//!
//! 目录：默认 `<app_data_dir>/media/`，由 `setup` 调 [`set_default_dir`] 定死（与 tauri.conf.json 的
//! `assetProtocol.scope = ["$APPDATA/media/**"]` 同处）；用户可改到别处（批次22-A，[`resolve_dir`] +
//! [`migrate`]），改到静态 scope 之外的目录时由 command 层运行期放行（`asset_protocol_scope`）。
//! 库里只存**相对文件名**——整个数据目录搬家后图片仍找得回来，绝对路径也不会因为换机器/换用户而说谎。

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::error::AppError;

/// 解码后单张上限（30 MB）。超限直接拒收：本地磁盘不是云盘，且 base64 走 JSON 参数，
/// 再大的图会把主线程卡在序列化上（前端在发之前就压缩并拒收，此处是写路径边界的第二道）。
pub const MAX_IMAGE_BYTES: u64 = 30 * 1024 * 1024;

/// 允许的图片格式（= 文件后缀 = 魔数白名单）。不在此列一律拒收：落盘的扩展名会被
/// `<img>` 与导出后的 Markdown 直接消费，收下一个 `.svg`（可含脚本）或 `.html` 就是自埋 XSS。
const FORMATS: &[(&[u8], &str)] = &[
    (&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], "png"),
    (&[0xFF, 0xD8, 0xFF], "jpg"),
    (&[b'G', b'I', b'F', b'8'], "gif"),
];

/// 合法后缀全集 = [`FORMATS`] 三个 + `webp`。webp 不在 FORMATS 里：它的魔数是偏移 8..12 的
/// `WEBP`（头 4 字节 `RIFF` 与 wav 相同），不是前缀，套不进 `starts_with`。
/// 后缀白名单必须含它，否则一张真 webp 写得进、删不掉、也不算占用。
const EXTS: &[&str] = &["png", "jpg", "gif", "webp"];

static MEDIA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);
static MEDIA_DEFAULT: OnceLock<PathBuf> = OnceLock::new();

/// 由 `setup` 在启动时定死**默认**目录（幂等：只有第一次生效）。默认目录 = 未自定义时的落盘处，
/// 也是 tauri.conf.json 静态 scope（`$APPDATA/media/**`）覆盖的唯一目录。
pub fn set_default_dir(dir: PathBuf) {
    let first = MEDIA_DEFAULT.set(dir.clone()).is_ok();
    let mut g = match MEDIA_DIR.write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if first || g.is_none() {
        *g = Some(dir);
    }
}

/// 用户设定值（`config.media_dir`）→ 生效目录：空/仅空白 = 默认目录；
/// 绝对路径 = 它本身；相对路径回落默认（校验本应在 command 层挡掉，这里只是不让一条脏值把启动带崩）。
pub fn resolve_dir(custom: &str) -> Option<PathBuf> {
    let c = custom.trim();
    if !c.is_empty() {
        let p = PathBuf::from(c);
        if p.is_absolute() {
            return Some(p);
        }
    }
    current_default_dir()
}

fn current_default_dir() -> Option<PathBuf> {
    MEDIA_DEFAULT.get().cloned()
}

/// 覆盖本进程的生效目录（改存放目录成功后调用；批次22-A 之前这里是 OnceLock、启动即定死）。
pub fn set_media_dir(dir: PathBuf) {
    let mut g = match MEDIA_DIR.write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *g = Some(dir);
}

/// 本进程的媒体目录。未初始化即报错——不静默回落到当前工作目录，
/// 那会把图片写到用户想都想不到地方，还会绕过 asset 协议的 scope。
pub fn media_dir() -> Result<PathBuf, AppError> {
    let g = match MEDIA_DIR.read() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    g.clone()
        .or_else(current_default_dir)
        .ok_or_else(|| AppError::Internal("媒体目录尚未初始化".into()))
}

/// 把已收藏的图片搬到新目录，返回搬动张数（批次22-A）。
///
/// 只搬"我们的文件"（名字过 [`is_safe_name`]）：目录里若混进用户手工放的别的文件，不属本次职责，
/// 更不能被我们搬走。任一步失败就把已搬过去的原样搬回并报错——**宁可改目录失败**，
/// 也不能让图片一半在新目录、一半在旧目录（库里只有相对文件名，两边各有一半时必然有一边全裂）。
pub fn migrate(from: &Path, to: &Path) -> Result<u64, AppError> {
    if from == to {
        return Ok(0);
    }
    std::fs::create_dir_all(to).map_err(|e| AppError::Internal(format!("无法创建图片目录：{e}")))?;
    let entries = std::fs::read_dir(from).map_err(|e| AppError::Internal(format!("无法读取旧图片目录：{e}")))?;
    let mut moved: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let src = entry.path();
        let name = match src.file_name().and_then(|n| n.to_str()) {
            Some(n) if is_safe_name(n) => n.to_string(),
            _ => continue,
        };
        if let Err(e) = move_file(&src, &to.join(&name)) {
            for back in moved.iter().rev() {
                let _ = move_file(&to.join(back), &from.join(back));
            }
            return Err(e);
        }
        moved.push(name);
    }
    Ok(moved.len() as u64)
}

/// 单个文件搬家：同盘 `rename` 是 O(1)；跨盘（`D:` → `E:` 这类）rename 会失败，退化成复制+删除。
fn move_file(src: &Path, dst: &Path) -> Result<(), AppError> {
    let io = |e: std::io::Error| AppError::Internal(format!("移动图片失败：{e}"));
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst).map_err(io)?;
            std::fs::remove_file(src).map_err(io)
        }
    }
}

/// 按魔数判定格式并返回扩展名；未知格式 `None`。
///
/// 不信前端报的 MIME/后缀：粘贴来源可以是任何应用，声明与实际字节不一致才是常态。
/// webp 是 `RIFF????WEBPVP8`，头 4 字节与 wav 相同，故单独查第 8..12 字节的 `WEBP`。
pub fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    FORMATS
        .iter()
        .find(|(magic, _)| bytes.starts_with(magic))
        .map(|(_, ext)| *ext)
}

/// 文件名是否安全：`<小写十六进制/UUID 形态>.<白名单后缀>`。
/// 挡住 `..`、路径分隔符、绝对路径、空后缀——这些一旦拼进 `dir.join(name)` 就是任意文件读写/删除。
pub fn is_safe_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else {
        return false;
    };
    // 只允许一个点：`a.png../../x` 的 stem 里含点，直接拒
    if stem.is_empty() || stem.contains('.') || stem.contains('/') || stem.contains('\\') {
        return false;
    }
    if !stem
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return false;
    }
    EXTS.contains(&ext)
}

/// base64 → 字节。手写而非引 `base64` crate（01 §2.1：不提前引入未使用依赖，本函数只服务一条录入路径）。
/// 容忍换行/空格（`toDataURL` 不会产出，但手工构造与剪贴板 HTML 里的 data URI 会折行），
/// 拒绝任何非法字符——解码失败即 `E_INPUT_INVALID`，不返回半张图。
pub fn decode_base64(input: &str) -> Result<Vec<u8>, AppError> {
    let bad = || AppError::InputInvalid("图片数据已损坏".into());
    // 先按 base64 长度粗筛（4 字符≈3 字节），避免为一句胡话分配几十 MB
    if input.len() as u64 > MAX_IMAGE_BYTES * 4 / 3 + 1024 {
        return Err(AppError::InputTooLarge);
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut padded = false;
    for &b in input.as_bytes() {
        if matches!(b, b'\n' | b'\r' | b' ' | b'\t') {
            continue;
        }
        if b == b'=' {
            padded = true; // 填充位之后的实质字符在下一轮被拒
            continue;
        }
        if padded {
            return Err(bad());
        }
        let idx = match b {
            b'A'..=b'Z' => u32::from(b - b'A'),
            b'a'..=b'z' => u32::from(b - b'a') + 26,
            b'0'..=b'9' => u32::from(b - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            // URL-safe 字母表（`-_`）：部分剪贴板实现给出的是这一套
            b'-' => 62,
            b'_' => 63,
            _ => return Err(bad()),
        };
        acc = (acc << 6) | idx;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if out.is_empty() {
        return Err(bad());
    }
    Ok(out)
}

/// 写一张图：确保目录存在后落盘。已存在同名文件一律覆盖——文件名含片段 id，
/// 撞上只可能是同一片段重写，不可能是别人的图。
pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), AppError> {
    if !is_safe_name(name) {
        return Err(AppError::InputInvalid(format!("非法图片文件名: {name}")));
    }
    std::fs::create_dir_all(dir).map_err(|e| AppError::Internal(e.to_string()))?;
    std::fs::write(dir.join(name), bytes).map_err(|e| AppError::Internal(e.to_string()))
}

/// 删一张图。文件本来不在（从未落盘/已被手工清掉）不报错：调用方的意图是"别留下它"，
/// 已达成的状态不该反过来阻断删除动作。
pub fn delete(dir: &Path, name: &str) {
    if !is_safe_name(name) {
        return;
    }
    let _ = std::fs::remove_file(dir.join(name));
}

/// 媒体目录占用（字节, 张数）。目录不存在 = `(0, 0)`（还没收藏过图，不是错误）。
/// 只统计白名单后缀：目录里若有别的文件（手工放的东西），不计进"图片占用"这个口径。
pub fn dir_usage(dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut count = 0u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !EXTS.contains(&ext.as_str()) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        bytes = bytes.saturating_add(size);
        count += 1;
    }
    (bytes, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-media-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sniffs_four_formats_and_rejects_unknown() {
        assert_eq!(sniff_ext(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0]), Some("png"));
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("jpg"));
        assert_eq!(sniff_ext(b"GIF89a...."), Some("gif"));
        assert_eq!(sniff_ext(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        // RIFF 但非 WEBP（wav）不得被认成图片
        assert_eq!(sniff_ext(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_ext(b"<svg onload=alert(1)>"), None);
        assert_eq!(sniff_ext(&[]), None);
    }

    #[test]
    fn names_are_traversal_safe() {
        assert!(is_safe_name("3f2a.png"));
        assert!(is_safe_name("3f2a-9c1e.webp"));
        assert!(!is_safe_name("../../etc/passwd.png"));
        assert!(!is_safe_name("a/../b.png"));
        assert!(!is_safe_name("a.png.exe"));
        assert!(!is_safe_name("a.svg"));
        assert!(!is_safe_name(".png"));
        assert!(!is_safe_name("a.png/"));
        assert!(!is_safe_name("C:\\evil\\a.png"));
    }

    #[test]
    fn base64_roundtrip_and_garbage_rejected() {
        // "iVBORw0KGgo=" 是 PNG 头 8 字节的标准 base64
        let png_head = [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let enc = "iVBORw0KGgo=";
        let dec = decode_base64(enc).unwrap();
        assert_eq!(dec, png_head.to_vec());
        assert_eq!(sniff_ext(&dec), Some("png"));
        // 折行容忍
        assert_eq!(decode_base64("iVBORw0K\nGgo=").unwrap(), png_head.to_vec());
        // URL-safe 字母表等价（`-_` ↔ `+/`）
        assert_eq!(decode_base64("__8=").unwrap(), decode_base64("//8=").unwrap());
        assert!(matches!(decode_base64("!!!!"), Err(AppError::InputInvalid(_))));
        assert!(matches!(decode_base64("A==A"), Err(AppError::InputInvalid(_))));
        assert!(matches!(decode_base64(""), Err(AppError::InputInvalid(_))));
    }

    #[test]
    fn oversized_input_rejected_before_allocating() {
        let huge = "A".repeat((MAX_IMAGE_BYTES as usize / 3) * 4 + 8192);
        assert!(matches!(
            decode_base64(&huge),
            Err(AppError::InputTooLarge)
        ));
    }

    #[test]
    fn write_delete_and_usage_track_the_same_file() {
        let dir = tmp_dir("lifecycle");
        let name = "sc-test-1.png";
        let bytes = [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(dir_usage(&dir), (0, 0));
        write(&dir, name, &bytes).unwrap();
        assert!(dir.join(name).is_file());
        let (b, c) = dir_usage(&dir);
        assert_eq!(c, 1);
        assert_eq!(b, bytes.len() as u64);
        // 同名重写=覆盖，不翻倍
        write(&dir, name, &bytes).unwrap();
        assert_eq!(dir_usage(&dir).1, 1);
        // 非白名单后缀不计入图片占用
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(dir_usage(&dir).1, 1);
        delete(&dir, name);
        assert!(!dir.join(name).is_file());
        // 删不存在的文件不报错（意图已达成）
        delete(&dir, name);
        // 非法名既写不进也不删得掉（越界路径保护）
        assert!(matches!(write(&dir, "../out.png", &bytes), Err(AppError::InputInvalid(_))));
        delete(&dir, "../notes.txt");
        assert!(dir.join("notes.txt").is_file(), "非法名不得波及目录外的文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_reports_zero_usage_not_error() {
        let dir = std::env::temp_dir().join(format!("sc-media-never-{}", uuid::Uuid::new_v4()));
        assert_eq!(dir_usage(&dir), (0, 0));
    }

    #[test]
    fn migrate_moves_only_our_images() {
        let a = tmp_dir("mig-a");
        let b = tmp_dir("mig-b");
        let png = [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        write(&a, "frag-1.png", &png).unwrap();
        write(&a, "frag-2.jpg", &[0xFF, 0xD8, 0xFF]).unwrap();
        std::fs::write(a.join("readme.txt"), b"not ours").unwrap();
        assert_eq!(migrate(&a, &a).unwrap(), 0, "同目录 no-op，不该自我搬动");
        assert_eq!(migrate(&a, &b).unwrap(), 2);
        assert!(b.join("frag-1.png").is_file() && b.join("frag-2.jpg").is_file());
        assert!(!a.join("frag-1.png").is_file(), "搬走不是复制：留在两处会让占用翻倍");
        assert!(a.join("readme.txt").is_file(), "不是我们收的文件就不该被搬走");
        assert_eq!(migrate(&a, &b).unwrap(), 0, "搬空后再搬一次是 0 张，不是错误");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }
}

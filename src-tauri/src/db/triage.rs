//! 入队前纯启发式分拣（02 §1.1 采集行为、03 §2 状态机 skipped）。
//!
//! 目的：在**零 LLM token** 的前提下，把两类不值得进流水线的片段直接落 `skipped`——
//!   1. 垃圾（`Junk`）：结构性零价值噪声——空/纯符号 emoji、纯界面文案（本 App 自身
//!      文案被剪贴板回捕的高置信特征）。
//!   2. 仅记录（`Raw`）：有价值但无需 AI 整理——裸链接（书签）、超短记录（词/成语/
//!      短句，长度无法与界面噪声区分，交用户裁决）。
//! 命中即不领取、不处理；用户仍可在详情页「重新处理」强制入队，或直接放行归档。
//! 判定纯依赖字符类别，绝不 panic，与内容长度线性相关。

/// 短记录阈值（字符数 ≤ 此值视为「仅记录」，交用户裁决，不消耗 token）。
pub const SHORT_RECORD_CHARS: usize = 8;

/// 跳过的理由。`skipped` 状态的三种来源之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// 空白 / 纯符号 / 纯 emoji：结构性零价值。
    Empty,
    /// 裸链接（整段是一个 URL）：书签，记录即目的。
    BareLink,
    /// 命中本 App 自身界面文案：剪贴板回捕的高置信垃圾。
    UiCopy,
    /// 超短记录（词 / 成语 / 短句）：可能有意，但无需 AI 整理。
    ShortRecord,
    /// 本地规则检出隐私信息（身份证 / 卡号 / 手机号 / 口令密钥）：
    /// **一律不发送模型、不做 embedding**。与前三者不同，它不是"低价值"，而是"有价值但不该外传"。
    Sensitive,
}

impl SkipReason {
    /// 是否应标「垃圾」徽标（否则仅记录，保留归档价值）。
    /// `Sensitive` 刻意**不**在此列：把隐私判成垃圾会诱导用户丢弃本该保密处理的条目。
    pub fn is_junk(self) -> bool {
        matches!(self, SkipReason::Empty | SkipReason::UiCopy)
    }
}

/// 检出的隐私类型（只用于措辞与统计，绝不含命中片段本身——标签不得成为新的泄露面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveKind {
    /// 18 位身份证号（含 GB 11643 校验位与出生日期合法性）
    IdCard,
    /// 13–19 位且过 Luhn 的卡号
    BankCard,
    /// 中国大陆手机号
    Phone,
    /// 私钥块 / 常见 token 前缀 / 「密码：xxx」式口令
    Secret,
}

/// 身份证权重与校验位映射（GB 11643-1999）。
const ID_WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
const ID_CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];

/// 口令关键词：值紧跟冒号/等号时才认定（避免"密码管理很有心得"这类叙述误伤）。
const SECRET_WORDS: &[&str] = &[
    "密码", "口令", "密钥", "私钥", "访问凭证", "凭证", "暗号",
    "password", "passwd", "pwd", "secret", "token", "apikey", "api_key", "api key", "access key",
];

fn is_id_card(run: &str) -> bool {
    let b = run.as_bytes();
    if b.len() != 18 || !b[..17].iter().all(u8::is_ascii_digit) {
        return false;
    }
    // 出生日期合法性：只挡明显胡编的串，降误报
    let year: u32 = run[6..10].parse().unwrap_or(0);
    let month: u32 = run[10..12].parse().unwrap_or(0);
    let day: u32 = run[12..14].parse().unwrap_or(0);
    if !(1900..=2100).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    let sum: u32 = b[..17].iter().enumerate().map(|(i, c)| u32::from(c - b'0') * ID_WEIGHTS[i]).sum();
    // 校验位允许小写 x
    run.chars().nth(17).map_or(false, |c| c == ID_CHECK[(sum % 11) as usize] || (c == 'x' && ID_CHECK[(sum % 11) as usize] == 'X'))
}

/// Luhn（银行卡号校验）：输入为纯数字串。
fn luhn_ok(run: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for c in run.chars().rev() {
        let mut d = match c.to_digit(10) {
            Some(d) => d,
            None => return false,
        };
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum % 10 == 0
}

/// 扫描文本中的**极大数字串**：两侧紧邻 ASCII 字母或数字的（嵌在哈希/订单号/句子里的片段）整串丢弃，
/// 否则一串长哈希里随便截出 16 位就能过 Luhn，误报会毁掉正常条目。
fn digit_runs(content: &str) -> Vec<String> {
    let bytes = content.as_bytes();
    let wordish = |i: usize| bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric());
    let mut runs = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if !wordish(start.wrapping_sub(1)) && !wordish(i) {
            runs.push(content[start..i].to_string());
        }
    }
    runs
}

/// 是否含 token 前缀或私钥块（纯本地字面判定）。前缀后须跟足量实体字符，
/// 免得"sk- 开头的 key"这种讨论语句被当成凭证本体。
fn has_token_shape(lower: &str) -> bool {
    const PREFIXES: &[(&str, usize)] = &[
        ("-----begin", 0), // PEM 头自身即结论（后面紧跟算法名与空格，不能按尾随长度判）
        ("ghp_", 16),
        ("gho_", 16),
        ("github_pat_", 16),
        ("akia", 12),
        ("xoxb-", 12),
        ("xoxp-", 12),
        ("ssh-rsa ", 20),
        ("bearer ", 20),
        ("sk-", 16),
    ];
    PREFIXES.iter().any(|(p, min_tail)| {
        lower.find(p).is_some_and(|at| {
            let tail: String = lower[at + p.len()..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect();
            tail.chars().count() >= *min_tail
        })
    })
}

/// 关键词起点前一字符不是字母/数字/下划线，才算独立词（挡掉 "tokenize" 里的 "token"）。
fn is_standalone(bytes: &[u8], at: usize) -> bool {
    match at.checked_sub(1).map(|i| bytes[i]) {
        Some(b) => !(b.is_ascii_alphanumeric() || b == b'_'),
        None => true,
    }
}

/// 「密码：xxxx」式口令：关键词后紧跟冒号/等号，值 ≥4 字符且**含 ≥4 个 ASCII 可见字符**。
/// 值全是汉字（"密码：明天开会"）判为叙述而非口令——口令必可键入。
fn has_credential_pair(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    SECRET_WORDS.iter().any(|w| {
        lower.match_indices(w).any(|(i, matched)| {
            if !is_standalone(bytes, i) {
                return false;
            }
            let after = lower[i + matched.len()..].trim_start();
            let Some(sep) = after.chars().next() else { return false };
            if !matches!(sep, ':' | '：' | '=' | '＝') {
                return false;
            }
            let value: String = after[sep.len_utf8()..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .filter(|c| !matches!(c, '"' | '\'' | '`' | '。' | '，' | '、' | '；'))
                .collect();
            value.chars().count() >= 4
                && value.bytes().filter(|b| b.is_ascii_graphic()).count() >= 4
        })
    })
}

/// 隐私检出（**纯本地、零依赖、零网络**）：`Some(kind)` → 该片段绝不发送模型、不做 embedding。
///
/// 权衡（不对称代价决定的取值）：误判的代价是"一条正常记录没被 AI 整理，用户可点「重新处理」复原"；
/// 漏判的代价是"身份证/口令进了第三方上下文，不可撤回"。故本函数**宁可误报**。
/// 明确不检：邮箱（公开标识符，且误报会毁掉大量正常条目）、住址、姓名、6 位验证码（误报率过高）。
pub fn detect_sensitive(content: &str) -> Option<SensitiveKind> {
    let runs = digit_runs(content);
    if runs.iter().any(|r| is_id_card(r)) {
        return Some(SensitiveKind::IdCard);
    }
    if runs.iter().any(|r| (13..=19).contains(&r.len()) && luhn_ok(r)) {
        return Some(SensitiveKind::BankCard);
    }
    if runs.iter().any(|r| {
        r.len() == 11 && r.starts_with('1') && r.as_bytes()[1].is_ascii_digit() && (b'3'..=b'9').contains(&r.as_bytes()[1])
    }) {
        return Some(SensitiveKind::Phone);
    }
    let lower = content.to_lowercase();
    if has_token_shape(&lower) || has_credential_pair(&lower) {
        return Some(SensitiveKind::Secret);
    }
    None
}

/// 隐私命中原因的短语标签（供 flag 文案，不含任何命中内容）。
pub fn sensitive_label(kind: SensitiveKind) -> &'static str {
    match kind {
        SensitiveKind::IdCard => "身份证号",
        SensitiveKind::BankCard => "银行卡号",
        SensitiveKind::Phone => "手机号",
        SensitiveKind::Secret => "口令/密钥",
    }
}

/// 本 App 自身会渲染进剪贴板的高频界面文案（精确匹配，忽略首尾空白）。
/// 仅作回捕黑名单，命中判垃圾；词条须与 `src/` 真实文案一致（不收录臆测串，免得误杀真实短记录）。
/// 随界面演进可增补，无需改判定逻辑。
const UI_COPY: &[&str] = &[
    // 导航 / 页面标题（2026-10-01 层改名后「缓冲区」不再渲染，故移除；「待分拣」在下一节动作里）
    "收集", "归档", "检索", "回收站", "设置", "归档库", "设置中心",
    // 分拣动作
    "放行", "丢弃", "恢复", "彻底删除", "放行归档", "重新处理", "移回待分拣",
    "换个方向再加工", "再加工", "清除筛选", "全部", "待分拣",
    // 状态 / 徽章 / 占位
    "排队中", "处理中", "完成", "失败", "未整理", "替你收的",
    // 空态提示（剪贴板误捕整块界面文案时的高频命中；「缓冲区空空如也」已随改名废弃）
    "没有待分拣的条目",
];

/// 整段是否为裸链接：无空白、以 http(s):// 开头。
fn is_bare_link(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && !trimmed.chars().any(char::is_whitespace)
        && (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
}

/// 纯启发式判定：`Some(reason)` → 应跳过 AI 处理并落 `skipped`；`None` → 正常入队。
/// 顺序即优先级：**隐私最先**（它的代价不可逆，且后面几条都可能被含数字/口令的隐私条目绕过），
/// 界面文案先于超短记录（二者常等长，须先识别为可丢弃的垃圾而非待归档记录）。
pub fn assess(content: &str) -> Option<SkipReason> {
    if detect_sensitive(content).is_some() {
        return Some(SkipReason::Sensitive);
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Some(SkipReason::Empty);
    }
    if UI_COPY.iter().any(|s| *s == trimmed) {
        return Some(SkipReason::UiCopy);
    }
    if is_bare_link(trimmed) {
        return Some(SkipReason::BareLink);
    }
    // 无任何字母/数字/汉字/假名（unicode Word 类别）→ 纯标点/符号/emoji，零价值。
    if !trimmed.chars().any(|c| c.is_alphabetic() || c.is_numeric()) {
        return Some(SkipReason::Empty);
    }
    if trimmed.chars().count() <= SHORT_RECORD_CHARS && !trimmed.contains(['\n', '\r']) {
        return Some(SkipReason::ShortRecord);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_id_card_with_valid_checksum_only() {
        // 11010119900307721 + 校验位 3（GB 11643 加权 mod 11 手算）
        assert_eq!(
            detect_sensitive("身份证 110101199003077213 备用"),
            Some(SensitiveKind::IdCard)
        );
        // 校验位不对 → 不认作身份证（可能是订单号，代价不对称但仍要避免乱挂标）
        assert_ne!(
            detect_sensitive("订单 110101199003077214"),
            Some(SensitiveKind::IdCard)
        );
    }

    #[test]
    fn detects_phone_and_bank_card() {
        assert_eq!(detect_sensitive("有事打我 13800138000"), Some(SensitiveKind::Phone));
        assert_eq!(
            detect_sensitive("卡号 4242424242424242 到期日"),
            Some(SensitiveKind::BankCard)
        );
    }

    #[test]
    fn detects_secrets_but_not_prose() {
        assert_eq!(detect_sensitive("wifi 密码：Xk9#tR2mLq"), Some(SensitiveKind::Secret));
        assert_eq!(
            detect_sensitive("ghp_AAAAAAAAAAAAAAAAAAAA"),
            Some(SensitiveKind::Secret)
        );
        assert_eq!(
            detect_sensitive("-----BEGIN RSA PRIVATE KEY-----\nMIIEow..."),
            Some(SensitiveKind::Secret)
        );
        // 误伤防线：值全是汉字的叙述、嵌在更长标识符里的数字、前缀后没有实体
        assert_eq!(detect_sensitive("密码：明天开会"), None);
        assert_eq!(detect_sensitive("abc13800138000"), None);
        assert_eq!(detect_sensitive("讨论 sk- 开头的密钥格式"), None);
        assert_eq!(detect_sensitive("tokenize 这个词的用法"), None);
        assert_eq!(detect_sensitive("https://example.com/a"), None);
    }

    #[test]
    fn sensitive_takes_priority_and_is_not_junk() {
        // 隐私优先于其余判定（含"仅记录"这一无害分类），且不得被当作垃圾
        assert_eq!(assess("13800138000"), Some(SkipReason::Sensitive));
        assert_eq!(assess("备用机 13800138000，密码：Xk9#tR2mLq"), Some(SkipReason::Sensitive));
        assert!(!SkipReason::Sensitive.is_junk());
        assert_eq!(sensitive_label(SensitiveKind::IdCard), "身份证号");
    }

    #[test]
    fn detection_is_linear_and_never_panics_on_odd_input() {
        // 边界：空串、超长纯数字、混合全角数字（非 ASCII，不该计入）
        assert_eq!(detect_sensitive(""), None);
        // 500 位纯数字不是卡号（长度不符），不该因"有一串数字"就挂标
        assert_eq!(detect_sensitive(&"7".repeat(500)), None);
        assert_eq!(detect_sensitive("１３８００１３８０００"), None);
    }

    #[test]
    fn ui_copy_is_junk_even_when_short() {
        // 界面标签与成语同长（均 4 字），必须优先判为可丢弃垃圾而非待归档记录。
        assert_eq!(assess("清除筛选"), Some(SkipReason::UiCopy));
        assert_eq!(assess("  没有待分拣的条目  "), Some(SkipReason::UiCopy));
        assert_eq!(assess("移回待分拣"), Some(SkipReason::UiCopy));
        assert!(assess("清除筛选").unwrap().is_junk());
    }

    #[test]
    fn bare_link_is_record_not_junk() {
        assert_eq!(assess("https://github.com/torvalds/linux"), Some(SkipReason::BareLink));
        assert!(!assess("https://example.com/a").unwrap().is_junk());
        // 带说明文字的长内容不因含 URL 而跳过
        assert_eq!(assess("看看这篇：https://example.com/very/long/path/that/matters 讲向量检索"), None);
    }

    #[test]
    fn blank_and_symbol_only_are_junk() {
        assert_eq!(assess("   \n\t "), Some(SkipReason::Empty));
        assert_eq!(assess("！！。…～——"), Some(SkipReason::Empty));
        assert_eq!(assess("😀😀🎉"), Some(SkipReason::Empty));
    }

    #[test]
    fn short_word_is_record_not_junk() {
        assert_eq!(assess("悬丝"), Some(SkipReason::ShortRecord)); // 词/成语
        assert_eq!(assess("刻舟求剑"), Some(SkipReason::ShortRecord));
        assert!(!assess("悬丝").unwrap().is_junk());
    }

    #[test]
    fn substantial_content_is_processed() {
        assert_eq!(assess("洪武十三年，空印案起，凡印文与册不符者皆论死。"), None);
        assert_eq!(assess("Rust ownership and borrowing rules"), None);
        // 恰在阈值外的短句应处理
        assert_eq!(assess("这是一句超过八个字符的短记录"), None);
    }
}

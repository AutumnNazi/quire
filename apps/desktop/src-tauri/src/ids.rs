//! 剪藏 id 的生成。

const BASE36: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// 毫秒时间戳占 9 位:当前约 1.79e12,而 36^9 ≈ 1.0e14,够用到 公元 2286 年。
const TS_WIDTH: usize = 9;
/// 随机盐占 7 位:4 字节熵(36^6 ≈ 2.2e9 < 2^32 < 36^7),同一毫秒内也不会撞。
const SALT_WIDTH: usize = 7;

pub const ID_LEN: usize = TS_WIDTH + SALT_WIDTH;

fn to_base36(n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut buf = Vec::new();
    let mut n = n;
    while n > 0 {
        buf.push(BASE36[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 字符全是 ASCII")
}

/// 定长 base36。定长是 id 能按字典序排序的前提——见 [`new_id`] 的说明。
fn to_base36_padded(n: u64, width: usize) -> String {
    let s = to_base36(n);
    if s.len() >= width {
        return s;
    }
    let mut out = String::with_capacity(width);
    for _ in 0..(width - s.len()) {
        out.push('0');
    }
    out.push_str(&s);
    out
}

/// 生成 16 字符的剪藏 id:9 位毫秒时间戳 + 7 位随机盐。
///
/// 时间前缀带来一个具体好处:第二周上 FTS5 之后,全文检索的结果天然按相关度或 id
/// 倒序输出就等于按剪藏时间倒序,不用额外排 `clipped_at` 列。文件名也因此自排序,
/// 在文件管理器里看就是新到旧。
///
/// 盐值是显式传入而非内部生成,是为了让时间序这个性质能被测试直接断言——
/// 靠 sleep 等待来测时间序的测试要么慢要么 flaky。
pub fn new_id(now_ms: i64, salt: u32) -> String {
    format!(
        "{}{}",
        to_base36_padded(now_ms.max(0) as u64, TS_WIDTH),
        to_base36_padded(salt as u64, SALT_WIDTH)
    )
}

/// 生成文件名前缀里的日期部分:`2026-09-28`。
///
/// 单独抽出来是因为日期要按**本地时区**算——UTC 会让凌晨剪的东西落到前一天,
/// 用户翻文件时会觉得"我明明刚剪的怎么是昨天"。传入本地日期而不是 UTC 日期。
pub fn date_prefix(year: i32, month: u32, day: u32) -> String {
    format!("{:04}-{:02}-{:02}", year, month, day)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn id长度固定() {
        // 定长是字典序等于时间序的前提,长度一旦漂移排序就废了
        assert_eq!(new_id(0, 0).len(), ID_LEN);
        assert_eq!(new_id(1_700_000_000_000, 0).len(), ID_LEN);
        assert_eq!(new_id(4_294_967_295, 4_294_967_295).len(), ID_LEN);
    }

    #[test]
    fn 时间越晚字典序越大() {
        // 第二周 FTS5 直接靠 id 排序拿时间序,这条断了检索结果顺序就是乱的
        let a = new_id(1_000_000_000_000, 4_294_967_295);
        let b = new_id(1_000_000_000_001, 0);
        let c = new_id(1_000_000_001_000, 0);
        assert!(a < b, "相邻毫秒必须严格递增");
        assert!(b < c, "跨秒也必须递增");
    }

    #[test]
    fn 同一毫秒内靠盐值区分且不越界() {
        // 盐值取满量程也不能溢出到时间前缀的位数,把时间序顶掉
        let a = new_id(1_700_000_000_000, 0);
        let b = new_id(1_700_000_000_000, 4_294_967_295);
        assert_ne!(a, b);
        assert!(a < b);
        assert_eq!(b.len(), ID_LEN);
    }

    #[test]
    fn 只含base36字符集() {
        let id = new_id(1_789_123_456_789, 0xDEAD_BEEF);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(id.chars().all(|c| BASE36.contains(&(c as u8).to_ascii_lowercase())));
    }

    #[test]
    fn 负数时间戳不panic() {
        // 极端输入不该让整个剪藏流程崩掉,退化成 0 即可
        assert_eq!(new_id(-5, 0).len(), ID_LEN);
    }

    #[test]
    fn 日期前缀补零到两位() {
        // 不补零的话 "2026-1-5" 的字典序排在 "2026-10-1" 前面,文件排序会乱
        assert_eq!(date_prefix(2026, 1, 5), "2026-01-05");
        assert_eq!(date_prefix(2026, 12, 31), "2026-12-31");
    }
}

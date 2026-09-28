//! 文件名生成与安全校验。
//!
//! 剪藏的文件名会直接暴露在用户的文件管理器里,也是详情页的寻址凭据,
//! 所以两件事必须同时成立:名字好看可读,且**不能被用来穿越出 vault 目录**。
//!
//! 这个模块也承担了"URL → 文件名"这条链路的第一步,见 [`host_of`]。

/// 域名转文件名词缀的截断长度。留出余量给日期和 id,让完整文件名稳在
/// Windows 的 255 字符上限内,也给路径总长留出空间。
const SLUG_MAX: usize = 60;

/// 从 URL 里取出主机名,失败则退回整串 URL。
///
/// 刻意不用 `url` crate:它会把 ICU 拖进依赖树,而那套东西在自包含链接下
/// 需要额外的数据文件。这里要处理的 URL 全是浏览器传过来的规范形式,
/// 手工解析的边界情况也够覆盖,不值得为它背一个重依赖。
pub fn host_of(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // protocol-relative 的 "//" 也要剥,否则 find('/') 会在下标 0 命中,主机名直接变空串
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // 去掉 userinfo(user:pass@host 这种形式)与端口,只留主机名
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        url.to_string()
    } else {
        host
    }
}

/// 把站点标识转成安全的文件名词缀:`example.com` → `example-com`。
///
/// 只保留 ASCII 字母数字,其余一律换成连字符——中文站点会退化成全连字符,
/// 但反正完整 id 已经在保证唯一性,后缀只是给人看的提示。
pub fn site_slug(site: &str) -> String {
    let mut out = String::with_capacity(site.len());
    for c in site.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return "clipped".to_string();
    }
    trimmed.chars().take(SLUG_MAX).collect()
}

/// 组装最终文件名:`2026-09-28-m8x2k9a4-example-com.md`。
///
/// 格式选的是"日期-id-站点":在文件管理器里按名称排序就等于按剪藏时间倒序,
/// 而 id 自带时间序,用户重命名文件也不会把顺序搅乱。
pub fn filename_for(date: &str, id: &str, site: &str) -> String {
    format!("{}-{}-{}.md", date, id, site_slug(site))
}

/// 校验前端传来的文件名能否安全地落到磁盘上。
///
/// 详情页按文件名取正文,而文件名来自前端。少了这层校验,一个
/// `../../.ssh/id_rsa` 就能让本地 HTTP 服务把 vault 之外的文件读出来。
/// 白名单比黑名单可靠:只放行已知安全的字符,新的攻击手法不会绕过。
pub fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() || name.len() > 200 {
        return false;
    }
    // 拒绝任何分隔符与父目录标记,连子目录都不支持——剪藏文件一律平铺
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    name.ends_with(".md")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 常见域名转成可读后缀() {
        assert_eq!(site_slug("example.com"), "example-com");
        assert_eq!(site_slug("sub.domain.co.uk"), "sub-domain-co-uk");
        assert_eq!(site_slug("Example.COM"), "example-com");
    }

    #[test]
    fn 非法字符被替换且不产生连续连字符() {
        assert_eq!(site_slug("a/b\\c:d*e?f"), "a-b-c-d-e-f");
        // 连续的非字母数字只留一个连字符,否则文件名会出现 "a---b"
        assert_eq!(site_slug("a...b"), "a-b");
        assert_eq!(site_slug("--a--b--"), "a-b");
    }

    #[test]
    fn 非ASCII站点名有兜底() {
        // 中文域名转不出 ASCII,不能返回空串——空串会让文件名退化成 "date-id-.md"
        assert_eq!(site_slug("例子.中国"), "clipped");
        assert_eq!(site_slug(""), "clipped");
    }

    #[test]
    fn 超长域名被截断() {
        let long = "a".repeat(500);
        assert_eq!(site_slug(&long).len(), SLUG_MAX);
    }

    #[test]
    fn 文件名按日期与id组装() {
        assert_eq!(
            filename_for("2026-09-28", "m8x2k9a4", "example.com"),
            "2026-09-28-m8x2k9a4-example-com.md"
        );
    }

    #[test]
    fn 正常文件名通过安全校验() {
        assert!(is_safe_filename("2026-09-28-m8x2k9a4-example-com.md"));
    }

    #[test]
    fn 路径穿越被拒绝() {
        // 这是本模块存在的主要理由:详情接口按文件名取正文,
        // 不校验的话一个 ../ 就能读出 vault 之外任意文件
        assert!(!is_safe_filename("../../.ssh/id_rsa"));
        assert!(!is_safe_filename("..\\..\\Windows\\System32\\config\\SAM"));
        assert!(!is_safe_filename("sub/dir/file.md"));
        assert!(!is_safe_filename("sub\\dir\\file.md"));
        assert!(!is_safe_filename("....//....//etc/passwd.md"));
    }

    #[test]
    fn 非markdown文件与空名被拒绝() {
        assert!(!is_safe_filename("evil.exe"));
        // 双重后缀仍然以 .md 结尾,白名单规则下是允许的——这里不做扩展名语义判断
        assert!(is_safe_filename("note.txt.md.md"));
        assert!(!is_safe_filename(""));
        assert!(!is_safe_filename(&"a".repeat(300)));
    }

    #[test]
    fn 空格与中文被拒绝() {
        // 白名单之外一律拒绝,哪怕这些字符实际上无害——
        // 这里宁可误伤,也不能给穿越留口子
        assert!(!is_safe_filename("我的 笔记.md"));
    }

    #[test]
    fn 从URL提取主机名() {
        assert_eq!(host_of("https://example.com/post/1"), "example.com");
        assert_eq!(host_of("https://Example.COM"), "example.com");
        assert_eq!(host_of("https://example.com:8443/a/b?x=1#y"), "example.com");
        assert_eq!(host_of("https://example.com"), "example.com");
    }

    #[test]
    fn 带userinfo和端口的URL只取主机名() {
        // user:pass@ 这种形式会明文出现在文件名里,必须剥掉
        assert_eq!(host_of("https://user:secret@example.com/x"), "example.com");
    }

    #[test]
    fn 无协议的URL不被丢弃() {
        // 扩展抓到的地址偶尔是 protocol-relative,不能因此产出空文件名
        assert_eq!(host_of("//example.com/x"), "example.com");
        assert_eq!(host_of("example.com/x"), "example.com");
    }
}

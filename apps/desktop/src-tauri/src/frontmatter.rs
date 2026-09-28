//! Frontmatter 的读写。
//!
//! 刻意不引 YAML crate:格式是我们自己写死的,解析器本身就是一份长期契约——
//! 用户会在任何文本编辑器里手改这些文件,十年后 Quire 还得读得懂自己的老数据。
//! 自己实现比迁就第三方 crate 的私有行为更可控。
//!
//! 支持的标量子集:`"..."` / `'...'` 字符串、裸串、`true`/`false`、整数、
//! `[a, b]` 列表、裸键(`key:`)。不认识的写法一律当纯字符串,不报错也不丢数据。

use std::collections::BTreeMap;

/// 一个 frontmatter 标量的内部表示。刻意不做嵌套结构——剪藏元数据用不上,
/// 而支持嵌套就意味着要实现完整 YAML,那还不如用真 YAML 库。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Bool(bool),
    Int(i64),
    List(Vec<String>),
    /// 写成 `key:`,值为空。区分于空字符串,是为了往返后能还原原样。
    Empty,
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }
}

/// 一篇剪藏的元数据。
///
/// `extra` 保留 Quire 不认识的键。用户手写的自定义字段不该因为版本升级而消失——
/// 数据是用户的,不是应用的。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frontmatter {
    pub id: String,
    pub title: String,
    pub url: String,
    pub site: String,
    pub author: Option<String>,
    pub clipped_at: String,
    pub published_at: Option<String>,
    pub excerpt: Option<String>,
    pub cover: Option<String>,
    pub tags: Vec<String>,
    pub read: bool,
    pub archived: bool,
    pub extra: BTreeMap<String, Value>,
}

/// 拆出 frontmatter 段和正文段。支持 CRLF:用户从 Windows 记事本存的文件带 `\r`。
pub fn split(input: &str) -> Option<(&str, &str)> {
    let rest = input
        .strip_prefix("---\r\n")
        .or_else(|| input.strip_prefix("---\n"))?;

    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\r').trim_end() == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

fn parse_value(raw: &str) -> Value {
    let s = raw.trim();
    if s.is_empty() {
        return Value::Empty;
    }
    match s {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    // 引号字符串要求首尾都闭合,避免把 `"abc` 当成带引号的值
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return Value::Str(unescape(&s[1..s.len() - 1]));
    }
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return Value::Str(s[1..s.len() - 1].to_string());
    }
    if s.len() >= 2 && s.starts_with('[') && s.ends_with(']') {
        let items = s[1..s.len() - 1]
            .split(',')
            .map(|x| x.trim().trim_matches('"').trim())
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
            .collect();
        return Value::List(items);
    }
    if let Ok(n) = s.parse::<i64>() {
        return Value::Int(n);
    }
    Value::Str(s.to_string())
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            // 未知转义序列原样保留,不做"尽力猜测",避免静默改写用户数据
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn render_value(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("\"{}\"", escape(s)),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::List(items) => {
            if items.is_empty() {
                "[]".to_string()
            } else {
                let inner = items
                    .iter()
                    .map(|i| format!("\"{}\"", escape(i)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", inner)
            }
        }
        Value::Empty => String::new(),
    }
}

impl Frontmatter {
    /// 从 frontmatter 段解析出已知字段,其余原样进 `extra`。
    pub fn parse(block: &str) -> Self {
        let mut fm = Frontmatter::default();
        for line in block.lines() {
            let line = line.trim_end_matches('\r');
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, raw)) = trimmed.split_once(':') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let value = parse_value(raw);

            // 已知键落到强类型字段,未知键原样留存
            match key {
                "id" => fm.id = value.as_str().unwrap_or_default().to_string(),
                "title" => fm.title = value.as_str().unwrap_or_default().to_string(),
                "url" => fm.url = value.as_str().unwrap_or_default().to_string(),
                "site" => fm.site = value.as_str().unwrap_or_default().to_string(),
                "author" => fm.author = opt_str(&value),
                "clipped_at" => fm.clipped_at = value.as_str().unwrap_or_default().to_string(),
                "published_at" => fm.published_at = opt_str(&value),
                "excerpt" => fm.excerpt = opt_str(&value),
                "cover" => fm.cover = opt_str(&value),
                "tags" => fm.tags = value.as_list().map(|v| v.to_vec()).unwrap_or_default(),
                "read" => fm.read = value.as_bool().unwrap_or(false),
                "archived" => fm.archived = value.as_bool().unwrap_or(false),
                _ => {
                    fm.extra.insert(key.to_string(), value);
                }
            }
        }
        fm
    }

    /// 序列化成 frontmatter 段(不含 `---` 分隔符)。键顺序固定,保证同样输入产出同样字节。
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut push = |k: &str, v: &str| {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push('\n');
        };
        push("id", &format!("\"{}\"", escape(&self.id)));
        push("title", &format!("\"{}\"", escape(&self.title)));
        push("url", &format!("\"{}\"", escape(&self.url)));
        push("site", &format!("\"{}\"", escape(&self.site)));
        push("author", &self.author.as_deref().map(|s| format!("\"{}\"", escape(s))).unwrap_or_default());
        push("clipped_at", &format!("\"{}\"", escape(&self.clipped_at)));
        push("published_at", &self.published_at.as_deref().map(|s| format!("\"{}\"", escape(s))).unwrap_or_default());
        push("excerpt", &self.excerpt.as_deref().map(|s| format!("\"{}\"", escape(s))).unwrap_or_default());
        push("cover", &self.cover.as_deref().map(|s| format!("\"{}\"", escape(s))).unwrap_or_default());
        push("tags", &render_value(&Value::List(self.tags.clone())));
        push("read", &self.read.to_string());
        push("archived", &self.archived.to_string());
        for (k, v) in &self.extra {
            push(k, &render_value(v));
        }
        out
    }

    /// 拼成完整的 Markdown 文件内容。
    pub fn to_markdown(&self, body: &str) -> String {
        format!("---\n{}---\n\n{}", self.render(), body)
    }
}

/// 空的 `key:` 视作"没写",和写了空字符串是两回事。
fn opt_str(v: &Value) -> Option<String> {
    match v {
        Value::Empty => None,
        Value::Str(s) if s.is_empty() => None,
        Value::Str(s) => Some(s.clone()),
        other => Some(format!("{other:?}")),
    }
}

#[cfg(test)]
// 测试名用中文描述行为本身,比 snake_case 的机翻名字好读;
// 但 rustc 的命名检查会把它们全报一遍,这里按模块豁免。
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn sample() -> Frontmatter {
        Frontmatter {
            id: "m8x2k9a4".into(),
            title: "深入理解 Rust 类型系统".into(),
            url: "https://example.com/post/1".into(),
            site: "example.com".into(),
            author: Some("张三".into()),
            clipped_at: "2026-09-28T14:30:00+08:00".into(),
            published_at: Some("2026-09-20T08:00:00+08:00".into()),
            excerpt: Some("一段摘要,用来在列表里当副标题。".into()),
            cover: None,
            tags: vec!["rust".into(), "编程".into()],
            read: false,
            archived: false,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn 完整往返保持一致() {
        let fm = sample();
        let md = fm.to_markdown("# 正文\n\n内容。");
        let (block, body) = split(&md).expect("应能拆出 frontmatter");
        let parsed = Frontmatter::parse(block);
        assert_eq!(parsed, fm);
        assert_eq!(body, "\n# 正文\n\n内容。");
    }

    #[test]
    fn 含引号反斜杠和换行的标题不破坏格式() {
        // 这类字符在网页标题里真实出现过(代码块文章的标题常带引号与反斜杠),
        // 不转义的话一行标题就能把整个 frontmatter 结构冲掉
        let mut fm = sample();
        fm.title = r#"他说"这是"真的\并且\很复杂"#.into();
        fm.excerpt = Some("第一行\n第二行\t带制表符".into());
        let md = fm.to_markdown("body");
        let (block, _) = split(&md).expect("含换行的标题不能让结构崩掉");
        let parsed = Frontmatter::parse(block);
        assert_eq!(parsed.title, fm.title);
        assert_eq!(parsed.excerpt, fm.excerpt);
    }

    #[test]
    fn 缺省可选字段解析为None而非空串() {
        let block = "title: \"无作者\"\nauthor: \ncover: \n";
        let fm = Frontmatter::parse(block);
        assert_eq!(fm.author, None);
        assert_eq!(fm.cover, None);
    }

    #[test]
    fn 用户手写的未知字段原样保留() {
        // 未来版本加的字段,或用户自己加的标注,都不能因为当前版本不认就丢失
        let block = "title: \"t\"\nrating: 5\nmood: \"good\"\nsource_app: \"手动\"\n";
        let fm = Frontmatter::parse(block);
        assert_eq!(fm.extra.get("rating"), Some(&Value::Int(5)));
        assert_eq!(fm.extra.get("mood"), Some(&Value::Str("good".into())));
        assert_eq!(fm.extra.get("source_app"), Some(&Value::Str("手动".into())));

        let block2 = Frontmatter::parse(&fm.render());
        assert_eq!(block2.extra, fm.extra);
    }

    #[test]
    fn CRLF文件能正常拆解() {
        // 用户从 Windows 记事本存的文件是 CRLF,不能因为这个就读不出元数据
        let md = "---\r\nid: \"abc\"\r\ntitle: \"标题\"\r\n---\r\n\r\n正文\r\n";
        let (block, body) = split(md).expect("CRLF 应能拆解");
        let fm = Frontmatter::parse(block);
        assert_eq!(fm.id, "abc");
        assert_eq!(fm.title, "标题");
        assert!(body.starts_with("\r\n正文"));
    }

    #[test]
    fn 无frontmatter的文件原样返回正文() {
        // 用户可能直接把一个 .md 拖进 vault,没有 frontmatter 也不该报错
        assert!(split("# 就一个标题\n\n正文").is_none());
        assert!(split("").is_none());
        // 有开头分隔符但没闭合,视为无 frontmatter
        assert!(split("---\ntitle: \"没闭合\"").is_none());
    }

    #[test]
    fn 注释行和空行被跳过() {
        let block = "# 这行是注释\n\ntitle: \"有效\"\n";
        let fm = Frontmatter::parse(block);
        assert_eq!(fm.title, "有效");
    }

    #[test]
    fn 单引号字符串按字面处理不转义() {
        let fm = Frontmatter::parse("title: 'C:\\path\\to\\file'\n");
        assert_eq!(fm.title, r"C:\path\to\file");
    }

    #[test]
    fn 列表元素正确解析() {
        let fm = Frontmatter::parse("tags: [\"rust\", \"编程\"]\n");
        assert_eq!(fm.tags, vec!["rust".to_string(), "编程".to_string()]);
    }
}

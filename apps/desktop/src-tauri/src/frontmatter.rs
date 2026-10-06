//! Frontmatter 的读写。
//!
//! 刻意不引 YAML crate:格式是我们自己写死的,解析器本身就是一份长期契约——
//! 用户会在任何文本编辑器里手改这些文件,十年后 Quire 还得读得懂自己的老数据。
//! 自己实现比迁就第三方 crate 的私有行为更可控。
//!
//! 支持的标量子集:`"..."` / `'...'` 字符串、裸串、`true`/`false`、整数、小数、
//! `[a, b]` 列表、裸键(`key:`)。不认识的写法一律当纯字符串,不报错也不丢数据。

use std::collections::BTreeMap;

/// 一个 frontmatter 标量的内部表示。刻意不做嵌套结构——剪藏元数据用不上,
/// 而支持嵌套就意味着要实现完整 YAML,那还不如用真 YAML 库。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Bool(bool),
    Int(i64),
    /// 小数。以前没有这个变体,`0.42` 会被当成字符串存,再写回去就变成
    /// `"0.42"`——用户文件里的数值每过一次 Quire 就多一层引号。
    Float(f64),
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

    /// 整数也算数:`progress: 1` 应当读成 1.0 而不是"没写过"。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(n) => Some(*n as f64),
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
    /// 用户自己写的批注。空串表示"没写",**不写进文件**,省得每篇
    /// 都挂一行 `note: ` 的噪音。
    pub note: String,
    pub read: bool,
    pub archived: bool,
    /// 用户手动标的"这个值得回头看"。**和归档是两码事**:归档是"读完了
    /// 挪到一边",收藏是"一直留着,别混在未读堆里"。用户存到几百篇之后,
    /// 真正的问题不是找不到,是"哪几篇值得再看一遍找不到"——收藏治的是这个
    pub starred: bool,
    /// 读到哪儿了,0.0–1.0。**存 0 就不往文件里写**,省得每篇都挂一行噪音。
    pub progress: f32,
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

/// 块标量的缩进。渲染时也用它,两边必须一致,否则往返一圈批注就歪了。
const BLOCK_INDENT: &str = "  ";

/// 读块序列:`tags:` 下面那些 `- 重要` 行。返回空列表表示"没写"。
///
/// **只认 `-` 开头的行。** 这就是安全的全部理由:块序列的行必然以 `-` 起头,
/// 而下一行的 `read: false` 不带 `-`,循环自然停在那里,后面的键不会被吃掉。
///
/// 缩进量不挑——YAML 允许块序列与键齐平,也允许缩进多格,两种都是标准写法。
/// `-` 后面可以跟引号串,跟块标量一个规矩地解掉引号
fn parse_block_sequence<'a, I>(lines: &mut std::iter::Peekable<I>) -> Vec<String>
where
    I: Iterator<Item = &'a str>,
{
    let mut items: Vec<String> = Vec::new();
    while let Some(next) = lines.peek() {
        let next = next.trim_end_matches('\r');
        let trimmed = next.trim_start();
        // 空行属于块序列内部(YAML 允许),但只在还有内容时才算,
        // 免得把文件末尾的空行当成一个空标签
        if trimmed.is_empty() {
            if items.is_empty() {
                break;
            }
            lines.next();
            continue;
        }
        let Some(rest) = trimmed.strip_prefix('-') else {
            break;
        };
        lines.next();
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        items.push(
            match (rest.len() >= 2, rest.starts_with('"'), rest.ends_with('"')) {
                (true, true, true) => unescape(&rest[1..rest.len() - 1]),
                (true, _, _) if rest.starts_with('\'') && rest.ends_with('\'') => {
                    rest[1..rest.len() - 1].to_string()
                }
                _ => rest.to_string(),
            },
        );
    }
    items
}

/// 读 `note: |` / `note: |-` 这样的块标量,把属于块的那些行吃掉。
///
/// 返回 `None` 表示**不是**块标量,调用方该走普通的单行解析。
///
/// 只认字面块 `|` 和去尾换行变体 `|-`。折叠块 `>` 不支持:写进去再读
/// 出来会变成另一种排版,用户看到自己写的内容被改了那是我们的锅。
///
/// `|` 和 `|-` 在这里读出来**是一样的**——末尾的换行一律不要。
/// 批注是界面上敲出来的文本,结尾空行没有含义,真按 YAML 的 chomping
/// 规则留着,反倒会让"看着一样的东西存出来不一样"
fn parse_block_scalar<'a, I>(marker: &str, lines: &mut std::iter::Peekable<I>) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    if marker != "|" && marker != "|-" {
        return None;
    }
    // 块在**缩进回落到第 0 列**时结束——frontmatter 里的键都在第 0 列。
    // 空行算块的一部分(用户分段是自然的),但它后面若接一个第 0 列的键,
    // 块就在这里收尾
    let mut collected: Vec<&str> = Vec::new();
    while let Some(next) = lines.peek() {
        let next = next.trim_end_matches('\r');
        if next.trim().is_empty() {
            lines.next();
            collected.push("");
            continue;
        }
        if !next.starts_with(BLOCK_INDENT) {
            break;
        }
        lines.next();
        collected.push(next.strip_prefix(BLOCK_INDENT).unwrap_or(next));
    }
    let mut text = collected.join("\n");
    while text.ends_with('\n') {
        text.pop();
    }
    Some(text)
}

/// 写块标量的内容部分(不含 `note: ` 前缀,也不含结尾换行)。内容每行缩进两格
fn render_block_scalar(s: &str) -> String {
    let mut out = String::from("|");
    for line in s.trim_end_matches('\n').split('\n') {
        out.push('\n');
        out.push_str(BLOCK_INDENT);
        out.push_str(line);
    }
    out
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
    // 只认"确实带小数点或指数"的写法。`NaN` / `inf` 一律当字符串,
    // 它们序列化回去会变成没法再解析回来的东西
    if (s.contains('.') || s.contains('e') || s.contains('E')) && s.parse::<f64>().is_ok() {
        if let Ok(f) = s.parse::<f64>() {
            if f.is_finite() {
                return Value::Float(f);
            }
        }
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
        Value::Float(f) => format!("{f}"),
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
        let mut lines = block.lines().peekable();
        while let Some(line) = lines.next() {
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

            // 块状标签列表要在进通用解析之前截住。`tags:` 右边是空的,
            // 真正的内容在下面那些 `- xxx` 行里,不截住的话
            // `parse_value("")` 给出空列表,而 parse 又永不报错——
            // 用户文件里明明有标签,Quire 读出来是空的,下次改标志就写回
            // `tags: []`,标签**永久消失**。这是全仓库唯一的静默数据丢失路径。
            //
            // `tags: [...]` 内联写法不走这里,那是 Quire 自己写出来的形状
            if key == "tags" && matches!(parse_value(raw), Value::Empty) {
                fm.tags = parse_block_sequence(&mut lines);
                continue;
            }

            // 批注的块标量要在进通用解析之前截住:块里的每一行都是内容,
            // 不是 `键: 值`。放进下面那个循环里,`作者: 我` 这种行会被
            // 当成新字段,用户的批注就被拆散了
            if key == "note" {
                fm.note = match parse_block_scalar(raw.trim(), &mut lines) {
                    Some(text) => text,
                    None => opt_str(&parse_value(raw)).unwrap_or_default(),
                };
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
                "starred" => fm.starred = value.as_bool().unwrap_or(false),
                // 钳到 0..=1。文件是用户的,手改成 `progress: 明天` 或
                // `progress: 3.7` 都得能扛住,不能让一个坏值顺着列表流到界面上
                "progress" => fm.progress = value.as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32,
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
        push(
            "author",
            &self
                .author
                .as_deref()
                .map(|s| format!("\"{}\"", escape(s)))
                .unwrap_or_default(),
        );
        push("clipped_at", &format!("\"{}\"", escape(&self.clipped_at)));
        push(
            "published_at",
            &self
                .published_at
                .as_deref()
                .map(|s| format!("\"{}\"", escape(s)))
                .unwrap_or_default(),
        );
        push(
            "excerpt",
            &self
                .excerpt
                .as_deref()
                .map(|s| format!("\"{}\"", escape(s)))
                .unwrap_or_default(),
        );
        push(
            "cover",
            &self
                .cover
                .as_deref()
                .map(|s| format!("\"{}\"", escape(s)))
                .unwrap_or_default(),
        );
        push("tags", &render_value(&Value::List(self.tags.clone())));
        // 没写批注就留个裸键,和 author / cover 缺席时一个写法。
        // 写了才落盘,免得每篇都挂一行空噪音
        if self.note.is_empty() {
            push("note", "");
        } else if self.note.contains('\n') {
            push("note", &render_block_scalar(&self.note));
        } else {
            push("note", &format!("\"{}\"", escape(&self.note)));
        }
        push("read", &self.read.to_string());
        push("archived", &self.archived.to_string());
        // 和 progress 同一个路子:没收藏就不写这行,免得每篇都挂一行 `starred: false`
        if self.starred {
            push("starred", "true");
        }
        if self.progress > 0.0 {
            push("progress", &format!("{:.2}", self.progress));
        }
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

    /// 收藏是**只写 true** 的那一类。绝大多数剪藏没收藏,
    /// 每篇都挂一行 `starred: false` 是纯噪音,用户打开文件会以为那是个正经字段
    #[test]
    fn 没收藏就不写这一行() {
        let fm = sample();
        assert!(!fm.render().contains("starred"), "没收藏不该写这行");
    }

    #[test]
    fn 收藏了就落盘() {
        let fm = Frontmatter {
            starred: true,
            ..sample()
        };
        assert!(fm.render().contains("starred: true"), "收藏了得写进文件");
    }

    #[test]
    fn 收藏能读回来() {
        let fm = Frontmatter::parse("id: x
title: \"t\"
starred: true
");
        assert!(fm.starred, "读不回来的话收藏就是个假功能");
    }

    /// 老用户的文件里根本没有这一行。**必须当没收藏,而不是当出错**
    #[test]
    fn 老文件没这一行也不算收藏() {
        let fm = Frontmatter::parse("id: x
title: \"t\"
read: false
");
        assert!(!fm.starred);
    }

    /// 收藏的往返不能动别的字段。这是「不许弄坏用户文件」那条底线
    #[test]
    fn 收藏往返其余字段不变() {
        let original = sample();
        let mut starred = original.clone();
        starred.starred = true;
        let text = starred.to_markdown("# 正文
");
        let (block, body) = split(&text).unwrap();
        let back = Frontmatter::parse(block);
        assert!(back.starred);
        assert_eq!(back.title, original.title);
        assert_eq!(back.url, original.url);
        assert_eq!(back.tags, original.tags);
        assert_eq!(back.note, original.note);
        // split 返回的正文段本来就带前导换行(现有的往返测试也是这个形状)
        assert_eq!(body, "
# 正文
", "正文得逐字节不变");
    }

    /// 收藏这一行要排在 `read` / `archived` 旁边,键顺序固定——
    /// 不然同样的数据每次导出产出不同字节
    #[test]
    fn 收藏那一行位置固定() {
        let fm = Frontmatter {
            starred: true,
            ..sample()
        };
        let r = fm.render();
        let at_star = r.find("starred:").unwrap();
        let at_read = r.find("read:").unwrap();
        assert!(at_read < at_star, "收藏排在已读后面");
    }


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
            note: String::new(),
            read: false,
            archived: false,
            starred: false,
            progress: 0.0,
            extra: BTreeMap::new(),
        }
    }

    // ── 批注 ──
    //
    // 批注是用户自己写的话,会换行、会带冒号、会带缩进。所以它不能只走
    // 引号字符串那一套:一行 `"第一行\n第二行"` 机器读得懂,人打开
    // .md 只看到一长条带 `\n` 的乱码。写成 YAML 块标量 `note: |`,
    // 任何 YAML 工具都认,人也能直接读——这是「数据是你的」这条承诺
    // 在格式上的落点,不是随手挑的写法。

    #[test]
    fn 单行批注往返不变() {
        let mut fm = sample();
        fm.note = "回头再看看这个思路".into();
        let md = fm.to_markdown("b");
        let (block, _) = split(&md).unwrap();
        assert_eq!(Frontmatter::parse(block).note, "回头再看看这个思路");
    }

    #[test]
    fn 多行批注渲染成块标量() {
        let mut fm = sample();
        fm.note = "第一行\n第二行".into();
        let rendered = fm.render();
        assert!(rendered.contains("\nnote: |\n"), "多行得用块标量:{rendered}");
        assert!(rendered.contains("  第一行\n  第二行\n"), "内容缩进两格");
    }

    #[test]
    fn 多行批注往返不变() {
        let mut fm = sample();
        fm.note = "第一行\n第二行\n第三行".into();
        let md = fm.to_markdown("b");
        let (block, _) = split(&md).unwrap();
        assert_eq!(Frontmatter::parse(block).note, fm.note);
    }

    /// 用户自己拿文本编辑器、或者在 Obsidian 里手写批注是常事。
    /// 手写出来的块标量必须读得懂,不然 Quire 就成了那种"只能自己写、
    /// 自己读"的数据格式
    #[test]
    fn 手写的块标量能读进来() {
        let block = "title: \"t\"\nnote: |\n  第一行\n  第二行\nread: false\n";
        assert_eq!(Frontmatter::parse(block).note, "第一行\n第二行");
    }

    /// 批注里出现 `作者: 我` 这种行,不能被当成 frontmatter 的新键。
    /// 当成了新键,用户的批注就被拆得七零八落,还凭空多出一堆字段
    #[test]
    fn 块标量里的冒号不当成新键() {
        let block =
            "title: \"t\"\nnote: |\n  作者: 我\n  链接: https://a.com\nread: false\n";
        let fm = Frontmatter::parse(block);
        assert_eq!(fm.note, "作者: 我\n链接: https://a.com");
        assert!(!fm.extra.contains_key("作者"), "批注里的行不是字段");
        assert!(!fm.extra.contains_key("链接"));
    }

    /// `|-` 是不带尾部换行的块标量。用户从别处粘一段进来,末尾
    /// 那个换行不该被当成内容的一部分
    #[test]
    fn 减号块标量不带尾部换行() {
        let block = "title: \"t\"\nnote: |-\n  只有一行\nread: false\n";
        assert_eq!(Frontmatter::parse(block).note, "只有一行");
    }

    /// 块标量里允许有空行。用户写批注时分段是自然的,
    /// 把空行当"块结束了"会让后半截批注变成一堆野字段
    #[test]
    fn 块标量里的空行不结束块() {
        let block = "title: \"t\"\nnote: |\n  上面一段\n\n  下面一段\nread: false\n";
        assert_eq!(Frontmatter::parse(block).note, "上面一段\n\n下面一段");
    }

    #[test]
    fn 空批注往返不变() {
        let mut fm = sample();
        fm.note = String::new();
        let md = fm.to_markdown("b");
        let (block, _) = split(&md).unwrap();
        assert_eq!(Frontmatter::parse(block).note, "");
        assert!(!fm.render().contains("\nnote: |\n"), "空批注不该占块标量");
    }

    #[test]
    fn 批注里的引号和反斜杠能原样存回() {
        let mut fm = sample();
        fm.note = r#"他说"这是重点"\而且很复杂"#.into();
        let md = fm.to_markdown("b");
        let (block, _) = split(&md).unwrap();
        assert_eq!(Frontmatter::parse(block).note, fm.note);
    }

    #[test]
    fn 批注行首的空格原样保留() {
        let mut fm = sample();
        fm.note = "正常行\n    缩进四格\n再一行".into();
        let md = fm.to_markdown("b");
        let (block, _) = split(&md).unwrap();
        assert_eq!(Frontmatter::parse(block).note, fm.note);
    }

    /// 手写的块标量 + Quire 写出来的文件,字段顺序可能不一样。
    /// 渲染一次再解析,批注一个字都不能变
    #[test]
    fn 手写块标量渲染后仍是同一个批注() {
        let block = "note: |\n  手写的批注\ntitle: \"t\"\nread: true\n";
        let mut fm = Frontmatter::parse(block);
        fm.note.push_str("\n再补一句");
        let md = fm.to_markdown("b");
        let (again, _) = split(&md).unwrap();
        assert_eq!(Frontmatter::parse(again).note, "手写的批注\n再补一句");
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
    #[test]
    fn 阅读进度能往返() {
        let mut fm = sample();
        fm.progress = 0.42;
        let back = Frontmatter::parse(&fm.render());
        assert!((back.progress - 0.42).abs() < 1e-6);
    }

    #[test]
    fn 没写过进度就是零() {
        // 老剪藏里根本没有这个字段。读不出来不能当成 100%(那等于"已读完"),
        // 也不能当成中间值
        let fm = Frontmatter::parse(
            "id: \"a\"
title: \"t\"
",
        );
        assert_eq!(fm.progress, 0.0);
    }

    // ── 块状标签列表 ──
    //
    // `tags:` 下面缩进写 `- 重要`,是 YAML 列表**最标准**的写法,Obsidian
    // 存出来的就是这个形状,用户从别的工具导入的笔记也多半是这个形状。
    //
    // 不认它,`parse_value("")` 会给一个空列表,而 `parse` 又是**永不报错**
    // 的——于是用户文件里明明有标签,Quire 读出来是空的。用户下一次点
    // 「标记已读」,`tags: []` 就被写回磁盘,标签**永久消失**,而且
    // `scan()` 的 unreadable 机制完全兜不住(它只报解析失败,不报解析错)。

    #[test]
    fn 缩进块状标签能读出来() {
        let block = "title: \"t\"\ntags:\n  - 重要\n  - 待读\nread: false\n";
        assert_eq!(Frontmatter::parse(block).tags, vec!["重要", "待读"]);
    }

    /// YAML 允许块序列和键齐平写,这也算标准。不是所有人手写都会缩进
    #[test]
    fn 齐平的块状标签也能读() {
        let block = "title: \"t\"\ntags:\n- 重要\n- 待读\nread: false\n";
        assert_eq!(Frontmatter::parse(block).tags, vec!["重要", "待读"]);
    }

    #[test]
    fn 块状标签里的引号和空格原样保留() {
        let block = "title: \"t\"\ntags:\n  - \"带 空格\"\n  - '单引号'\n  - 裸串\n";
        assert_eq!(Frontmatter::parse(block).tags, vec!["带 空格", "单引号", "裸串"]);
    }

    /// 用户手写的标签里带引号是很正常的(`- "他说\"这是重点\""`),
    /// 解不开的话标签栏上就是一串带反斜杠的乱码。`set_tags` 写出去的标签
    /// 早就洗掉了引号,可**读**的这条路要扛得住用户自己写的
    #[test]
    fn 块状标签里的转义要解开() {
        let block = "tags:\n  - \"他说\\\"这是重点\\\"\"\n  - \"换行\\n也在里面\"\n";
        assert_eq!(
            Frontmatter::parse(block).tags,
            vec!["他说\"这是重点\"", "换行\n也在里面"]
        );
    }

    /// 块状标签读出来之后,写回去还是那几个。渲染成内联还是块状都行,
    /// **数据不能少**——那才是要命的地方
    #[test]
    fn 块状标签往返不丢() {
        let block = "title: \"t\"\ntags:\n  - 重要\n  - 待读\n";
        let fm = Frontmatter::parse(block);
        let again = Frontmatter::parse(&fm.render());
        assert_eq!(again.tags, vec!["重要", "待读"]);
    }

    /// 块状列表不能把后面那个键吃掉。吃掉了的话 `read` 会变成空,
    /// 未读队列就乱了
    #[test]
    fn 块状标签不吃掉后面的键() {
        let block = "title: \"t\"\ntags:\n  - 重要\nread: true\narchived: true\n";
        let fm = Frontmatter::parse(block);
        assert_eq!(fm.tags, vec!["重要"]);
        assert!(fm.read, "read 还在");
        assert!(fm.archived, "archived 还在");
    }

    #[test]
    fn 空的块状标签就是空列表() {
        let block = "title: \"t\"\ntags:\nread: false\n";
        assert!(Frontmatter::parse(block).tags.is_empty());
    }

    /// 原来就支持的内联写法一个字都不能退化
    #[test]
    fn 内联标签写法照旧() {
        assert_eq!(
            Frontmatter::parse("tags: [\"rust\", \"编程\"]\n").tags,
            vec!["rust", "编程"]
        );
        assert!(Frontmatter::parse("tags: []\n").tags.is_empty());
        assert!(Frontmatter::parse("title: \"t\"\n").tags.is_empty());
    }

    /// 缩进很深也能认。YAML 允许任意缩进量
    #[test]
    fn 缩进几格都认() {
        let block = "tags:\n      - 重要\n        - 待读\n";
        assert_eq!(Frontmatter::parse(block).tags, vec!["重要", "待读"]);
    }

    #[test]
    fn 进度是零就不写进文件() {
        // 每篇都多一行 progress: 0 是纯噪音,用户打开文件会以为那是个字段
        assert!(!sample().render().contains("progress"));
        let mut fm = sample();
        fm.progress = 0.5;
        assert!(fm.render().contains("progress: 0.5"));
    }

    #[test]
    fn 乱写的进度不会变成奇怪的数() {
        // 文件是用户的,手改成 progress: 明天 或 progress: 3.7 都得能扛住
        assert_eq!(
            Frontmatter::parse(
                "id: \"a\"
progress: abc
"
            )
            .progress,
            0.0
        );
        assert_eq!(
            Frontmatter::parse(
                "id: \"a\"
progress: 3.7
"
            )
            .progress,
            1.0,
            "超过 1 的当读完"
        );
        assert_eq!(
            Frontmatter::parse(
                "id: \"a\"
progress: -1
"
            )
            .progress,
            0.0,
            "负数当没读过"
        );
    }


}

//! 从别的稍后读工具把数据搬进来。
//!
//! **为什么自己解析 CSV 而不用 `csv` crate**:整个导入只用到 RFC 4180 的一个
//! 子集,而这个子集的坑全在**引号**上——字段里的逗号、字段里的换行、`""` 表示
//! 一个引号。Pocket 导出的标题里这三种全都有("C++ 入门,第二版"、"第一行\n第二行")。
//! 按逗号硬切的写法会在这些标题上炸掉,而且炸得很难看:一篇文章被切成三篇,
//! 用户看不出发生了什么,只看到库里的东西全都错位了。
//!
//! **导入不做去重之外的事**。它只负责把"别的工具的一行"变成"剪藏库里的一篇",
//! 判重、编号、摘要兜底都交给 [`crate::vault::Vault`],那条路已经有测试兜着。
//! 导入器再自己实现一遍,等于给同一个规则写两份实现,迟早会不一致。

use serde::Serialize;

use crate::frontmatter::Frontmatter;

/// 一行原始记录。**字段按名字取,不按下标**——CSV 的列序在不同导出里完全
/// 不一样,按下标取等于赌用户导出的那一版列序没变过
pub type Row = std::collections::HashMap<String, String>;

/// 一次导入的结果。三类分开报,因为它们对用户是完全不同的三件事:
///  - `imported` 真导进来了
///  - `duplicates` 库里已经有了,跳过(不是失败,用户文件夹里重复很正常)
///  - `failed` 这一行读不出来,得说清是哪一行
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataImportReport {
    pub imported: Vec<String>,
    pub duplicates: Vec<String>,
    pub failed: Vec<ImportFailure>,
    /// 格式压根不认,或者文件读不出来。**只有一个值**,因为整批都停了,
    /// 和"某些行不行"是完全不同的两回事
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    /// 第几行,1 起,含表头。用户拿这个数去他的表格软件里查
    pub row: usize,
    pub reason: String,
}

/// 把 CSV 文本拆成表头 + 带行号的数据行。
///
/// 表头大小写不敏感、忽略首尾空白:Excel 和各家工具导出的
/// `Title` / `title` / ` TITLE ` 都当成同一个字段名。用户不该因为
/// 某个工具多打了一个空格就导不进来。
///
/// **空行直接丢掉**,不算失败。用户表格末尾常有若干空行,那不是数据。
///
/// 行号是 1 起**含表头**的:用户在他表格软件里看到的行号就是这样,
/// 报"第 37 行没导进去"时他不用自己加一
pub fn parse_csv(text: &str) -> Result<(Vec<String>, Vec<RowWithLine>), String> {
    let records = split_records(text);
    let mut it = records.into_iter();
    let Some(header) = it.next() else {
        // 一个空文件。全是空行也算——用户可能导了个空表格
        return Ok((Vec::new(), Vec::new()));
    };
    if header.is_empty() {
        return Err("文件是空的,一行表头都没有".to_string());
    }
    let columns: Vec<String> = header.iter().map(|h| normalize_header(h)).collect();
    let mut rows = Vec::new();
    // +2:表头占了第 1 行,而 `enumerate` 从 0 起
    for (i, record) in it.enumerate() {
        if record.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let mut row = Row::new();
        for (idx, value) in record.iter().enumerate() {
            // 列数不够时,多出来的字段当没有。**不补空串**:补了的话用户会
            // 看到"标签:空"这种他压根没写过的字段,以为工具把他的数据改了
            let Some(name) = columns.get(idx) else { break };
            if name.is_empty() {
                continue;
            }
            row.insert(name.clone(), value.clone());
        }
        rows.push(RowWithLine { row, line: i + 2 });
    }
    Ok((columns, rows))
}

/// 带原始行号的行。**解析阶段就得记住行号**:等用户看到"第 37 行没导进去"
/// 这句话的时候,`record` 早就出了作用域,而那时他正需要这个数
pub struct RowWithLine {
    pub row: Row,
    pub line: usize,
}

fn normalize_header(name: &str) -> String {
    name.trim().trim_start_matches('\u{feff}').to_ascii_lowercase()
}

/// 按 RFC 4180 拆记录。
///
/// 规则就三条,但每一条都有真实数据踩中过:
///  - `"` 开头的字段里,`""` 是一个字面的引号
///  - 引号字段里可以塞换行,那是**一个**字段里的换行,不是记录边界
///  - 引号只在字段开头有意义。`他说"好"` 这种不带引号包裹的字段,
///    里面的引号是字面量,不能当成转义——按"遇到引号就切换模式"写会从这里开始
///    一路错到行尾,后面整行都解析错了
fn split_records(text: &str) -> Vec<Vec<String>> {
    // BOM。Excel 存 UTF-8 CSV 时会塞一个,不去掉的话第一个列名会变成
    // `\ufefftitle`,而列名匹配是按名字取的,于是**每一行**都读不出标题
    let text = text.trim_start_matches('\u{feff}');
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    // 引号只在**字段开头**才开启,这是上面第三条规则的直接后果
    let mut at_field_start = true;
    let mut in_quotes = false;

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                // 两个引号 = 一个字面引号;一个引号 = 字段结束
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        match ch {
            '"' if at_field_start => {
                in_quotes = true;
                at_field_start = false;
            }
            ',' => {
                record.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' => {
                // `\r\n` 算一个换行。单独的 `\r`(老 Mac 存的 CSV)也算,
                // 忽略它——留着的话每个字段末尾都会挂一个 `\r`,
                // 前缀比较判重就永远对不上
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                at_field_start = true;
            }
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                at_field_start = true;
            }
            _ => {
                field.push(ch);
                at_field_start = false;
            }
        }
    }
    // 文件末尾没有换行时,最后一条记录还在手上
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

/// 从一行里取第一个命中的字段。**列名别名全在这里**,因为各家工具叫法不同:
/// Pocket 叫 `time_added`,别的工具叫 `date` 或 `created`,对用户来说
/// 这些都是"什么时候加的收藏"
pub fn pick<'a>(row: &'a Row, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| row.get(*n)).map(|s| s.as_str())
}

/// Pocket 的标签是一个字段里用 `|` 分开的:`阅读|coding`。
/// **不能只按逗号切**——按逗号切的话带逗号的标签会碎成两个
pub fn split_pocket_tags(raw: &str) -> Vec<String> {
    raw.split('|')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// 通用场景:标签可能是一个字段里用逗号分,也可能是分行的多值字段
pub fn split_tags(raw: &str) -> Vec<String> {
    let parts: Vec<&str> = if raw.contains('|') && !raw.contains(',') {
        raw.split('|').collect()
    } else {
        raw.split(',').collect()
    };
    parts
        .into_iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// 一行变成一篇剪藏的**材料**。真正的落盘在 [`crate::vault::Vault`] 那边做,
/// 这里只负责"这行各字段叫什么、值是什么"
#[derive(Debug, Clone)]
pub struct Candidate {
    pub url: String,
    pub title: String,
    pub tags: Vec<String>,
    pub clipped_at: String,
    pub body: String,
    pub archived: bool,
    /// 它在源文件里是第几行,1 起含表头。**JSON 里的下标也要能报**——
    /// 用户看到"第 12 条没导进去",得有个东西能让他回到那一条
    pub line: usize,
}

/// Pocket 的 CSV。列名是**官方导出固定的那几个**,但用户可能自己改过表头,
/// 所以每个字段都配了别名——宁可多认几个,也不要把用户导不进来的东西
/// 归到"格式不认识"里去,那是最让人火大的失败方式
pub fn from_pocket_csv(text: &str) -> Result<Vec<Candidate>, String> {
    let (_cols, rows) = parse_csv(text)?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let url = pick(&r.row, &["url"]).unwrap_or("").trim().to_string();
        let title = pick(&r.row, &["title"]).unwrap_or("").trim().to_string();
        // Pocket 的 status 只有 unread 和 archive 两种,别的当没标
        let status = pick(&r.row, &["status"]).unwrap_or("").trim().to_ascii_lowercase();
        let tags_raw = pick(&r.row, &["tags", "tag"]).unwrap_or("");
        let added = pick(&r.row, &["time_added", "timeadded", "added", "date"])
            .unwrap_or("")
            .trim();
        out.push(Candidate {
            title: if title.is_empty() { url.clone() } else { title },
            clipped_at: pocket_time_to_iso(added),
            tags: split_pocket_tags(tags_raw),
            archived: status == "archive",
            body: String::new(),
            url,
            line: r.line,
        });
    }
    Ok(out)
}

/// Pocket 的 `time_added` 是**秒级 Unix 时间戳**,不是毫秒也不是日期串。
///
/// 弄错量级的后果特别隐蔽:当成毫秒解析会得到 1970 年 1 月,界面上是一堆
/// 五十年前的剪藏,但**每一条都在**,没有一条报错,用户只会觉得"这软件好怪"
pub fn pocket_time_to_iso(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }
    let Ok(secs) = raw.trim().parse::<i64>() else {
        // 不是数字就当它已经是日期串,原样带过去。`Frontmatter` 那层
        // 会兜住这个值,总比丢了他原本的时间强
        return raw.trim().to_string();
    };
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
        .unwrap_or_default()
}

/// 通用 CSV:列名别名 + 值格式猜测。这一段是"尽力而为"——
/// 用户手上的 CSV 长什么样没人知道,所以宁可多猜,也不要在**读不出标题**
/// 这种地方直接失败
pub fn from_generic_csv(text: &str) -> Result<Vec<Candidate>, String> {
    let (_cols, rows) = parse_csv(text)?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let url = pick(&r.row, &["url", "link", "href", "address", "网址", "链接"])
            .unwrap_or("")
            .trim()
            .to_string();
        let title = pick(
            &r.row,
            &["title", "name", "heading", "subject", "标题", "名称"],
        )
        .unwrap_or("")
        .trim()
        .to_string();
        let excerpt = pick(&r.row, &["excerpt", "summary", "description", "note", "摘要"])
            .unwrap_or("")
            .trim()
            .to_string();
        let tags_raw = pick(&r.row, &["tags", "tag", "labels", "categories", "标签"])
            .unwrap_or("");
        let added = pick(
            &r.row,
            &[
                "date", "added", "created", "time", "timestamp", "date_added", "saved",
                "日期", "时间",
            ],
        )
        .unwrap_or("")
        .trim();
        let status = pick(&r.row, &["status", "state", "read", "read_status", "状态"])
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        // 正文优先给"内容"列;没有正文就用摘要顶着。**空正文导不进来**
        // 是硬约束(vault 那边会拒收),所以这里必须给点什么
        let body = pick(&r.row, &["content", "body", "text", "html", "正文", "内容"])
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(Candidate {
            title: if title.is_empty() { url.clone() } else { title },
            clipped_at: normalize_date(added),
            tags: split_tags(tags_raw),
            archived: status == "archive" || status == "archived" || status == "已归档",
            body: if body.is_empty() { excerpt } else { body },
            url,
            line: r.line,
        });
    }
    Ok(out)
}

/// **只做形态判断,不做真的转换。**`from_generic_csv` 里那个"不是数字就原样带过去"
/// 就是靠这个:能转就转成 ISO,转不了说明对方本来就是日期串,别动它。
/// 转不动又不像日期的,原样返回,让 `Frontmatter` 那层去处理——**宁可原样也不丢**
pub fn normalize_date(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    // Pocket 那种秒级时间戳
    if let Ok(secs) = raw.parse::<i64>() {
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
            return dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        }
    }
    // `YYYY-MM-DD HH:MM:SS` 这种"日期 + 空格 + 时间"。**不直接换成 T**——
    // 补 T 的活儿交给 chrono,它认这个格式,而且真解析不了会告诉我们,
    // 比我们在这儿拼字符串拼出个不存在的日期强
    if raw.len() > 10 && raw.as_bytes()[10] == b' ' {
        return format!("{}T{}", &raw[..10], raw[11..].trim());
    }
    raw.to_string()
}

/// 从 Quire 自己导出的 JSON 导回去。
///
/// **认不出来就说认不出来**,不猜。Quire 的 JSON 有固定字段名,猜不出来就说明
/// 这不是我们的文件——那这时候该告诉用户"这不是 Quire 导出的文件",
/// 而不是尽力解析出一堆字段错位的东西
pub fn from_quire_json(text: &str) -> Result<Vec<Candidate>, String> {
    // **BOM 必须在进 serde 之前去掉。**`serde_json` 不认它,会报
    // "expected value at line 1 column 1"——一个把用户指向他自己文件、
    // 而文件明明是合法 JSON 的报错。Excel 和 Windows 编辑器存出来的
    // JSON 头部都常带这个字节
    let text = text.trim_start_matches('\u{feff}');
    let parsed: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("不是合法的 JSON:{e}"))?;
    let items = match &parsed {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(o) => o
            .get("clips")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => return Err("JSON 的顶层得是数组,或者含 clips 数组的对象".to_string()),
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            return Err(format!("第 {} 条不是一条记录", i + 1));
        };
        let s = |k: &str| obj.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let title = s("title");
        let url = s("url");
        let content = s("content");
        let body = if content.is_empty() { s("excerpt") } else { content };
        // **既没标题也没正文就直接不导这一条**,而不是造一篇空的出来。
        // 空篇进了库,在列表里是一条点开什么都没有的记录,比缺一条更糟
        if title.is_empty() && url.is_empty() && body.is_empty() {
            continue;
        }
        let tags = match obj.get("tags") {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            Some(serde_json::Value::String(s)) => split_tags(s),
            _ => Vec::new(),
        };
        out.push(Candidate {
            title: if title.is_empty() { url.clone() } else { title },
            url,
            clipped_at: normalize_date(&s("clipped_at")),
            tags,
            archived: obj.get("archived").and_then(|v| v.as_bool()).unwrap_or(false),
            body,
            line: i + 1,
        });
    }
    Ok(out)
}

/// 这批文本是哪种格式。**先看内容,再看扩展名**——用户从 Pocket 导出的是
/// `pocket.csv`,但也有工具给 CSV 换个 `.txt` 的后缀,也有给 JSON 换 `.csv` 的
pub fn detect_and_parse(text: &str, filename: &str) -> Result<Vec<Candidate>, String> {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    // BOM 不算空白,`trim_start` 去不掉它。**先去 BOM 再判断**,否则带 BOM 的
    // JSON 认不出来,会掉到 "不认这个格式" 那条路上——而它本来是认得的
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let looks_json = head.starts_with('[') || head.starts_with('{');
    if ext == "json" || looks_json {
        return from_quire_json(text);
    }
    if ext == "csv" || ext == "tsv" {
        // Pocket 认得出来就按 Pocket 的规矩读。**不是靠文件头判断**——
        // Pocket 的列名足够独特,靠内容认比靠格式认准
        let head = text.lines().next().unwrap_or("").to_ascii_lowercase();
        if head.contains("time_added") || head.contains("status") {
            return from_pocket_csv(text);
        }
        return from_generic_csv(text);
    }
    Err(format!("不认这个格式:{ext}"))
}

/// 一行变成 `Frontmatter` + 正文。**放到导入器里而不是 vault 里**,
/// 因为"从别的工具的一行变成 frontmatter"是导入独有的活,普通的
/// Markdown 导入没有这一步
pub fn to_frontmatter(c: Candidate) -> (Frontmatter, String) {
    let Candidate {
        url,
        title,
        tags,
        clipped_at,
        body,
        archived,
        line: _,
    } = c;
    // 一次填进结构体字面量,而不是 `default()` 之后逐个赋值。
    // 逐个赋值看着顺手,但**加字段时会静默留在 default 值上**——而
    // `Default::default()` 给的是空串和 false,那不是"没设置",是"设成了空"
    let fm = Frontmatter {
        url: url.clone(),
        title: title.clone(),
        tags,
        clipped_at,
        archived,
        ..Default::default()
    };
    // **没有正文就合成一段最小的,而不是空着。** Pocket 导出的数据里只有
    // 标题和地址,压根没有文章正文;而 vault 拒收空正文——那一篇在列表里点开
    // 是空的,用户会以为导坏了。合成的东西只由他**自己数据里的字段**拼成,
    // 不编造任何内容:标题当一级标题,地址指回原文
    let body = if body.trim().is_empty() {
        let mut b = String::new();
        if !title.trim().is_empty() {
            b.push_str(&format!("# {title}\n\n"));
        }
        if !url.trim().is_empty() {
            b.push_str(&format!("原文:{url}\n"));
        }
        b
    } else {
        body
    };
    (fm, body)
}

#[cfg(test)]
// 测试名用中文描述行为本身,比 snake_case 的机翻名字好读
#[allow(non_snake_case)]
mod tests {
    use super::*;

    /// **字段里的逗号**。Pocket 的标题 "Rust 所有权,五分钟入门" 就是这样,
    /// 按逗号硬切会把一篇切成两篇,而且两篇都不报错
    #[test]
    fn csv字段里的逗号不切行() {
        let text = "title,url\n\"Rust 所有权,五分钟入门\",https://a.com\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].row["title"], "Rust 所有权,五分钟入门");
    }

    /// **字段里的换行**。摘要经常是两段,带换行
    #[test]
    fn csv字段里的换行不算记录边界() {
        let text = "title,excerpt\n甲,\"第一段\n第二段\"\n乙,单段\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows.len(), 2, "引号里的换行不该多出一条记录");
        assert_eq!(rows[0].row["excerpt"], "第一段\n第二段");
        assert_eq!(rows[1].row["title"], "乙");
    }

    /// `""` 是**一个字面引号**,不是两个
    #[test]
    fn csv双引号是一个引号() {
        let text = "title\n\"他说\"\"好\"\"\"\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows[0].row["title"], "他说\"好\"");
    }

    /// **不带引号包裹的字段里的引号是字面量**。这条最隐蔽:
    /// 按"见引号就切模式"写,这一行的引号会一路错到行尾,
    /// 后面整行全解析错,而且不报错
    #[test]
    fn csv非引号字段里的引号不当转义() {
        let text = "title,url\n他说\"好\",https://a.com\n乙,https://b.com\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows.len(), 2, "引号不该把第二条记录吃掉");
        assert_eq!(rows[0].row["title"], "他说\"好\"");
        assert_eq!(rows[0].row["url"], "https://a.com");
    }

    /// 表头大小写和空格不敏感。Excel 存一次就把表头改成 ` Title ` 的人多得是
    #[test]
    fn csv表头忽略大小写和空格() {
        let text = " Title , URL \n甲,https://a.com\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows[0].row["title"], "甲");
        assert_eq!(rows[0].row["url"], "https://a.com");
    }

    /// **Excel 的 UTF-8 BOM**。不去掉的话第一个列名带 BOM,
    /// 而列名匹配是按名字取的,于是**每一行**都读不出标题
    #[test]
    fn csv开头的BOM不算进列名() {
        let text = "\u{feff}title,url\n甲,https://a.com\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows[0].row["title"], "甲", "BOM 吃进了列名就全完了");
    }

    /// CRLF。Windows 上每个文件都是这个行尾,而残留的 `\r` 会让
    /// 前缀判重永远对不上
    #[test]
    fn csvCRLF行尾不留在字段里() {
        let text = "title,url\r\n甲,https://a.com\r\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows[0].row["url"], "https://a.com");
    }

    /// 末尾空行不算失败。用户表格末尾常有若干空行
    #[test]
    fn csv末尾空行不算数据() {
        let text = "title,url\n甲,https://a.com\n\n\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows.len(), 1);
    }

    /// 行号是**用户在自己表格软件里看到的那个数**:含表头、1 起。
    /// 报个 off-by-one,用户跳过去查的是隔壁一行
    #[test]
    fn csv行号含表头且从一开始() {
        let text = "title,url\n甲,https://a.com\n乙,https://b.com\n丙,https://c.com\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows[0].line, 2);
        assert_eq!(rows[2].line, 4);
    }

    /// **列数不齐不该炸**。用户手工加过列的表格很常见,
    /// 少一列就整行读不出来是不能接受的
    #[test]
    fn csv列数不齐照样读() {
        let text = "title,url,tags\n甲,https://a.com\n";
        let (_c, rows) = parse_csv(text).unwrap();
        assert_eq!(rows[0].row["title"], "甲");
    }

    /// 空文件。**不是错误**,是"0 篇"
    #[test]
    fn 空csv当零篇() {
        let (_c, rows) = parse_csv("").unwrap();
        assert_eq!(rows.len(), 0);
    }

    /// **`time_added` 是秒,不是毫秒**。当毫秒解析会掉进公元五万多年份——
    /// 每一条都在、没有一条报错,用户只觉得"这软件好怪"
    #[test]
    fn pocket时间戳按秒解析() {
        // 2021-01-01T00:00:00Z = 1609459200
        assert_eq!(
            pocket_time_to_iso("1609459200"),
            "2021-01-01T00:00:00+00:00"
        );
        // 量级差出三个数量级。**钉的是"差得离谱"这个事实**,不是某个具体年份:
        // 写死 1970 是我在猜 chrono 会不会 saturate,而它其实不,
        // 会老老实实算出五万多年——写死具体年份的断言会因为猜错而误导后来的人
        let millis_wrong = chrono::DateTime::from_timestamp(1_609_459_200_000, 0).unwrap();
        assert_ne!(
            millis_wrong.format("%Y").to_string(),
            "2021",
            "秒和毫秒必须差出三个数量级,不然这个测试证明不了什么"
        );
    }

    #[test]
    fn pocket时间戳为空就返回空() {
        assert_eq!(pocket_time_to_iso(""), "");
        assert_eq!(pocket_time_to_iso("   "), "");
    }

    /// 不是数字就**原样带过去**,不丢。用户已经填好的日期比没有强
    #[test]
    fn pocket时间戳不是数字就原样留() {
        assert_eq!(pocket_time_to_iso("2023-05-01"), "2023-05-01");
    }

    /// Pocket 的标签用 `|` 分,不是逗号
    #[test]
    fn pocket标签按竖线拆() {
        let got = split_pocket_tags("阅读|coding| rust ");
        assert_eq!(got, vec!["阅读", "coding", "rust"]);
    }

    /// 通用 CSV 的标签用逗号分
    #[test]
    fn 通用标签按逗号拆() {
        assert_eq!(split_tags("阅读, coding"), vec!["阅读", "coding"]);
    }

    /// 完整跑一遍 Pocket 的真实导出
    #[test]
    fn pocket_csv整批读得出来() {
        let text = concat!(
            "title,url,time_added,tags,status\n",
            "\"C++ 入门,第二版\",https://a.com,1609459200,阅读|编程,unread\n",
            "Rust 所有权,https://b.com,1609545600,编程,archive\n"
        );
        let got = from_pocket_csv(text).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].title, "C++ 入门,第二版");
        assert_eq!(got[0].tags, vec!["阅读", "编程"]);
        assert!(!got[0].archived, "unread 不是归档");
        assert!(got[1].archived, "archive 该标成归档");
        assert_eq!(got[1].line, 3);
    }

    /// **没标题就用地址顶上**。空标题进库就是列表里一条点开什么都没有的记录
    #[test]
    fn 没标题就用地址顶上() {
        let got = from_pocket_csv("title,url\n,https://a.com\n").unwrap();
        assert_eq!(got[0].title, "https://a.com");
    }

    /// 通用 CSV 的中文列名
    #[test]
    fn 通用csv认中文列名() {
        let text = "标题,链接,标签,日期\n甲文,https://a.com,阅读,2023-05-01\n";
        let got = from_generic_csv(text).unwrap();
        assert_eq!(got[0].title, "甲文");
        assert_eq!(got[0].url, "https://a.com");
        assert_eq!(got[0].tags, vec!["阅读"]);
    }

    /// **既没标题也没正文就跳过**。造一篇空的出来比缺一条更糟
    #[test]
    fn json里空记录跳过() {
        let text = r#"[{"title":"甲","url":"https://a.com"},{"title":"","url":""}]"#;
        let got = from_quire_json(text).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// JSON 认得 Quire 自己导出的形状
    #[test]
    fn quire_json读得回来() {
        let text = r#"[{"title":"甲","url":"https://a.com","tags":["阅读","编程"],"archived":true}]"#;
        let got = from_quire_json(text).unwrap();
        assert_eq!(got[0].title, "甲");
        assert_eq!(got[0].tags, vec!["阅读", "编程"]);
        assert!(got[0].archived);
    }

    /// 合法的 JSON 但形状不对。**说清楚不认**,不猜——猜出来的是一堆错位数据
    #[test]
    fn json形状不对就说清楚() {
        let err = from_quire_json("[1,2,3]").unwrap_err();
        assert!(err.contains("第 1 条"), "得指出是哪一条:{}", err);
    }

    /// 不是 JSON 就别报"不是 JSON",报形状不对——前者是用户拼错了括号
    #[test]
    fn 坏json报的是语法错() {
        let err = from_quire_json("{ 坏掉的").unwrap_err();
        assert!(err.contains("JSON"), "{}", err);
    }

    /// **看内容,不只看扩展名**。用户给 CSV 套个 `.txt` 的壳很常见
    #[test]
    fn 靠内容认格式不看扩展名() {
        let text = "title,url\n甲,https://a.com\n";
        let got = detect_and_parse(text, "pocket.csv").unwrap();
        assert_eq!(got[0].title, "甲");
        let got2 = detect_and_parse(text, "随便.txt").unwrap_err();
        assert!(got2.contains("不认"), "{}", got2);
    }

    /// 带 BOM 的 JSON 也认
    #[test]
    fn 带bom的json也认() {
        let text = "\u{feff}[{\"title\":\"甲\",\"url\":\"https://a.com\"}]";
        let got = detect_and_parse(text, "x.json").unwrap();
        assert_eq!(got[0].title, "甲");
    }

    /// `YYYY-MM-DD HH:MM:SS` 补成 ISO。**不自己拼日期**,
    /// 只把空格换成 T,剩下的交给 chrono
    #[test]
    fn 日期时间补上T() {
        assert_eq!(normalize_date("2023-05-01 10:30:00"), "2023-05-01T10:30:00");
    }

    /// 已经是日期串就**原样留**。不要自作聪明去重排它
    #[test]
    fn 日期原样留() {
        assert_eq!(normalize_date("2023-05-01"), "2023-05-01");
        assert_eq!(normalize_date("  "), "");
    }
}

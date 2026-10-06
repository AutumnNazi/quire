//! FTS5 全文索引。
//!
//! ## 它是纯缓存,不是数据
//!
//! 剪藏的真相永远在 `.md` 文件里。这个索引删了、坏了、过期了,最坏的结果是
//! "退回逐文件扫描,慢一点"——**剪藏一个字节都不会少**。README 里写着
//! "索引随时可以删",这句在这儿是字面意思。
//!
//! ## 为什么要预切二元组
//!
//! SQLite 自带的 `unicode61` 分词器会把一整串汉字当成**一个** token:
//! 「所有权是 Rust 的核心概念」进去就是一个词,查「有权」搜不到。
//! `trigram` 分词器能解决,但它有 3 字符的硬下限,而中文里「苹果」「淘宝」
//! 全是两字——`search.rs` 开头那几段就是把它否掉的理由。
//!
//! 所以切词还是在 Rust 里做(和 `search::tokenize` 同一个函数),
//! **存进索引的是切好之后用空格连起来的词条**。FTS5 只负责在这堆词条里
//! 找"哪个词条出现过",不懂中文。
//!
//! ## 两条路必须给一样的结果
//!
//! 索引只负责**挑候选**,打分和切片段仍然走 `search.rs` 原来那套。
//! 所以同一份数据下,走索引和走扫描必须给出**逐条相同**的结果和排序。
//! `索引和扫描给出一样的结果` 那条测试盯着这个——它是这个模块的全部安全网。
//! 一旦两条路分叉,用户就会遇到"有时候搜得到有时候搜不到",那比慢可怕得多。

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

/// 一条候选。**和 `search.rs` 里的 `ClipSummary` 分开**:
/// 这里带的是打分需要的原文,不是给界面直接用的摘要
pub struct Candidate {
    /// 文件名
    pub id: String,
    /// **原始 frontmatter 段**,不带 `---`。存它是为了让索引那条路能用
    /// 和扫描那条路**同一个** `Frontmatter::parse` 重建摘要——自己拆自己拼
    /// 的话,两边的字段就会各漏一个,而且漏的是哪一边没人看得出来
    pub fm: String,
    pub title: String,
    pub body: String,
    pub tags: String,
    pub note: String,
}

/// 建表语句。**索引文件是纯缓存**,所以 schema 版本变了直接整个删掉重建,
/// 不搞迁移——重建一次几秒的事,为一个缓存写迁移脚本不值当
const SCHEMA: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS clips USING fts5(
    id UNINDEXED,
    fm UNINDEXED,
    title UNINDEXED,
    body UNINDEXED,
    title_tok,
    body_tok,
    tags_tok,
    note_tok,
    mtime UNINDEXED,
    size UNINDEXED
);
";

pub struct Index {
    conn: Connection,
}

impl Index {
    /// 打开(必要时建表)。**建不出来就返回 Err**,由调用方退回扫描——
    /// 索引是加速器,不是必需品,它坏了不该让搜索跟着坏
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// 打开索引,坏成就地删掉重建。**一定成功**,除非连目录都建不出来
    ///
    /// 索引文件会被弄坏,而且方式都不体面:同步冲突写进了半截文件、
    /// 网盘把没下完的 0 字节文件同步下来、用户自己手滑。`Index::open`
    /// 对这些一律返回 `Err`——那在测试里是对的(该让调用方看见),
    /// 但在生产上意味着**每次搜索都要先处理一次坏文件**,而处理的方式
    /// 无非就是重建。所以把这个决定收在这里,别散到每个调用点
    ///
    /// 删不掉的极端情况(文件被别的进程锁着)也照样建一个空的:
    /// 大不了这次搜索退回扫描,下一轮再试。**不能因为一个缓存文件
    /// 让整个搜索不可用**
    pub fn open_or_recreate(path: &Path) -> Self {
        if let Ok(idx) = Self::open(path) {
            return idx;
        }
        let _ = std::fs::remove_file(path);
        // 上面这行可能失败(文件被锁着)。`SQLITE_OPEN_CREATE` 还是会
        // 建一个新的,顶多这次搜索走扫描,不会每次都重来一遍
        Self::open(path).unwrap_or_else(|_| Self {
            conn: Connection::open_in_memory().expect("内存库建不出来"),
        })
    }

    /// 索引一篇。`mtime` / `size` 存下来是为了**认出哪些变了**——
    /// Quire 没有文件监控,用户在外面改的文件,这里靠"重扫时对一遍"发现
    pub fn upsert(
        &mut self,
        id: &str,
        fm_block: &str,
        body: &str,
        mtime: u64,
        size: u64,
    ) -> Result<(), rusqlite::Error> {
        // 标题、标签、批注从 frontmatter 里取,不让调用方自己拆一遍传进来。
        // 那样两边一旦拆法不一样(比如标签的块序列),索引和扫描就会悄悄分叉
        let fm = crate::frontmatter::Frontmatter::parse(fm_block);
        let title = fm.title.as_str();
        let tags = fm.tags.join(" ");
        let note = fm.note.as_str();
        // `tokenize` 返回的是**借用入参的切片**,所以转好的小写必须绑在
        // 一个活着的变量上。曾经写成 `tokenize(&s.to_lowercase())`——
        // 那是把引用指向一个当场析构的临时值,读的是已经释放的内存。
        // 症状很恶劣:大部分查询碰巧还能中,少数字段(标签/批注/特定正文)
        // 莫名其妙搜不到,像是随机坏了
        let join = |s: &str| -> String {
            let lower = s.to_lowercase();
            super::search::tokenize(&lower).into_iter().collect::<Vec<_>>().join(" ")
        };
        // FTS5 表没有唯一约束,直接 INSERT 是**追加**不是覆盖。
        // 同一篇改一次就多一行,`count(*)` 跟着涨,搜出来会重复
        self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        self.conn.execute(
            "INSERT INTO clips (id, fm, title, body, title_tok, body_tok, tags_tok, note_tok, mtime, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                id,
                fm_block,
                title,
                body,
                join(title),
                join(body),
                join(&tags),
                join(note),
                mtime as i64,
                size as i64,
            ],
        )?;
        Ok(())
    }

    /// 库里已经有的这篇,长什么样。**索引里没它就返回 None**——
    /// 那是"该重新索引了"的信号,不是错误
    pub fn stamp(&self, id: &str) -> Option<(u64, u64)> {
        self.conn
            .query_row(
                "SELECT mtime, size FROM clips WHERE id = ?1 LIMIT 1",
                [id],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
            )
            .ok()
    }

    /// 删掉一篇(它被移到回收站、或彻底删了的时候)
    pub fn remove(&mut self, id: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM clips WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 索引里一共几篇。**索引和磁盘对不上时用它判断哪边更可信**:
    /// 差得离谱就直接整个重建,比一篇篇对时间戳快
    pub fn len(&self) -> usize {
        self.conn
            .query_row("SELECT count(*) FROM clips", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 全清。重建时用
    pub fn clear(&mut self) -> Result<(), rusqlite::Error> {
        self.conn.execute("DELETE FROM clips", [])?;
        Ok(())
    }

    /// 把磁盘上已经没有的条目从索引里剔掉,返回剔了几条
    ///
    /// **这个方法是必需的,不是优化。** 同步只做了"缺的补、变的重索引",
    /// 没想到"多的删"——于是用户删掉一篇剪藏,那篇会继续被索引供出来,
    /// 搜得到、点进去报"剪藏不存在"。用户在界面上没有任何线索能解释
    /// 这件事:文件确实没了,列表里也确实没了,可就是搜得到
    ///
    /// 一条条 `remove` 太慢(几千次写事务),所以先问出保留名单再取差集
    pub fn prune(&mut self, keep: &HashSet<String>) -> Result<usize, rusqlite::Error> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM clips")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut removed = 0;
        for id in ids {
            if !keep.contains(&id) {
                self.conn.execute("DELETE FROM clips WHERE id = ?1", [&id])?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 查候选。**`tokens` 必须是切好的词条**,不是用户输入的原文——
    /// 调用方先 `tokenize` 一次。这里不切是因为存进去的时候已经切过了,
    /// 再切一遍就是白干
    ///
    /// **用 OR 而不是短语。** 原来逐文件那条路是"词条里有一个在原文里出现
    /// 就算候选"(`wanted.iter().any(...)`),短语查询更严,会把"只命中部分
    /// 词条"的结果筛掉——那就变成两套语义了,用户会看到"有时候搜得到有时候
    /// 搜不到"。宽进严排:这里只要给够候选,排序交给 `search.rs` 原来那套
    /// 按范围挑候选。**范围只改 `WHERE` 里参与匹配的那几列**
    ///
    /// 注意别把列名写进 `build_query` 生成的 MATCH 串里——那边已经被
    /// `WHERE` 的列限定管着了,再写一遍会静默把别的字段搜不了,
    /// 不报错,就是搜不到(这个坑踩过一次)
    pub fn search(
        &self,
        tokens: &[String],
        scope: crate::search::Scope,
    ) -> Result<Vec<Candidate>, rusqlite::Error> {
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let query = build_query(tokens);
        let where_clause = match scope {
            crate::search::Scope::Any => {
                "title_tok MATCH ?1 OR body_tok MATCH ?1 OR tags_tok MATCH ?1 OR note_tok MATCH ?1"
            }
            crate::search::Scope::Title => "title_tok MATCH ?1",
            crate::search::Scope::Body => "body_tok MATCH ?1",
            crate::search::Scope::Tag => "tags_tok MATCH ?1",
            crate::search::Scope::Note => "note_tok MATCH ?1",
        };
        let sql = format!(
            "SELECT id, fm, title, body, tags_tok, note_tok FROM clips WHERE {where_clause}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([query], |r| {
            Ok(Candidate {
                id: r.get(0)?,
                fm: r.get(1)?,
                title: r.get(2)?,
                body: r.get(3)?,
                tags: r.get(4)?,
                note: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

/// 拼 FTS5 的 MATCH 表达式。
///
/// 每个词条都用**列限定** + 双引号包起来。引号不是可有可无的:FTS5 的查询
/// 语法里 `-` `*` `(` `)` `:` 都有特殊含义,而词条是从用户输入里切出来的,
/// 什么字符都可能有。不包引号的话,搜 `rust*` 或者 `c++` 会直接报语法错,
/// 表现是"搜索框一敲就弹红条"。
fn build_query(tokens: &[String]) -> String {
    let quoted: Vec<String> = tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    // **这里不能再写列名。** 调用那边的 WHERE 已经是
    // `title_tok MATCH ?1 OR body_tok MATCH ?1 ...`,列名写了等于
    // `title_tok MATCH 'title_tok:(...)'`——列限定套列限定,
    // 标签和批注那两路就悄悄搜不到了(正文和标题碰巧还能中,
    // 所以只测正文的话发现不了)
    format!("({})", quoted.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn index() -> (TempDir, Index) {
        let d = TempDir::new().unwrap();
        let idx = Index::open(&d.path().join("index.sqlite")).unwrap();
        (d, idx)
    }

    /// 把用户输入切成词条,和真实调用方一个动作。
    ///
    /// **存进去的是切好的词条,查的也必须是切好的。** 第一版测试直接传
    /// 原字符串,于是「记一下」这种三字词查不到——存的是 `记一 一下 下这 这句`,
    /// 拿 `记一下` 去比当然对不上。看起来像索引坏了,其实是测试用错了契约
    fn toks(s: &str) -> Vec<String> {
        let lower = s.to_lowercase();
        crate::search::tokenize(&lower)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// 拼一段 frontmatter。测试里**用真的 frontmatter 喂 `upsert`**,
    /// 别图省事直接传标题字符串——那样测的是"参数摆对了吗",不是
    /// "从真实文件里解析出来的东西能不能搜到"
    fn fm(id: &str, title: &str, tags: &[&str], note: &str) -> String {
        let mut out = format!("id: {id}
title: {title}");
        if !tags.is_empty() {
            out.push_str(&format!("
tags: [{}]", tags.join(", ")));
        }
        if !note.is_empty() {
            out.push_str(&format!("
note: {note}"));
        }
        out
    }

    #[test]
    fn 能建能开() {
        let (_d, idx) = index();
        assert!(idx.is_empty(), "刚建好当然是空的");
    }

    /// 索引是**纯缓存**:文件删了,重新打开能重建出来
    #[test]
    fn 索引文件删了能重建() {
        let d = TempDir::new().unwrap();
        let path = d.path().join("index.sqlite");
        {
            let mut idx = Index::open(&path).unwrap();
            idx.upsert("a", &fm("x1", "甲", &[], ""), "正文里提到所有权", 1, 100).unwrap();
        }
        std::fs::remove_file(&path).unwrap();

        let idx = Index::open(&path).unwrap();
        assert!(idx.is_empty(), "文件没了就是空的,不该报错");
    }

    #[test]
    fn 两字中文能搜到() {
        // **这一条是整个模块存在的理由。** `unicode61` 会把一整串汉字当成
        // 一个 token,不预切二元组的话「苹果」这种查询永远搜不到
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "买手机", &[], ""), "这篇讲的是苹果手机的系统", 1, 100)
            .unwrap();

        let hits = idx.search(&toks("苹果"), crate::search::Scope::Any).unwrap();
        assert_eq!(hits.len(), 1, "两字中文必须能搜到");
        assert_eq!(hits[0].id, "a");
    }

    #[test]
    fn 只命中一个词条也算候选() {
        // 原来逐文件那条路是 `any` 语义,这里必须一样——
        // 改成"全部词条都要命中"就变成两套语义了
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "甲", &[], ""), "只提到了苹果", 1, 100).unwrap();

        let hits = idx
            .search(
                &[toks("苹果"), toks("华为")].concat(),
                crate::search::Scope::Any,
            )
            .unwrap();
        assert_eq!(hits.len(), 1, "部分命中也得是候选");
    }

    #[test]
    fn 标签和批注也算字段() {
        // 原来 `search.rs` 把 tags 和 note 一起预筛,少一个就搜不到标签里那篇
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "甲", &["待读"], ""), "正文", 1, 100).unwrap();
        idx.upsert("b", &fm("x2", "乙", &[], "记一下这句"), "正文", 1, 100).unwrap();

        let tags = idx.search(&toks("待读"), crate::search::Scope::Any).unwrap();
        let note = idx.search(&toks("记一下"), crate::search::Scope::Any).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(note.len(), 1);
    }

    /// 标题命中优先级靠的是**原文**,所以原文必须原样存回来,
    /// 不能存成切好的词条
    #[test]
    fn 取回来的原文没被切词弄坏() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "所有权,那个东西", &[], ""), "正文里有  Rust  和标点。", 1, 100)
            .unwrap();

        let hits = idx
            .search(&toks("rust"), crate::search::Scope::Any)
            .unwrap();
        assert_eq!(hits[0].title, "所有权,那个东西");
        assert_eq!(hits[0].body, "正文里有  Rust  和标点。", "正文得原样存回");
    }

    /// 改过的文件得能覆盖掉旧的,不然搜出来还是旧内容
    #[test]
    fn 重复索引是覆盖不是追加() {
        let (_d, mut idx) = index();
        // 旧新两版**刻意不共享任何二元组**。第一版拿「旧正文 / 新正文」来验,
        // 结果红了一次——它们共享「正文」这个二元组,而契约本来就是
        // "任一词条命中即候选",所以搜旧文本命中新版本是**正确行为**。
        // 拿一份不共享的样本,才真的在验"旧内容被替换掉了"
        idx.upsert("a", &fm("x1", "旧标题", &[], ""), "甲乙丙丁", 1, 100).unwrap();
        idx.upsert("a", &fm("x1", "新标题", &[], ""), "戊己庚辛", 2, 200).unwrap();

        assert_eq!(idx.len(), 1, "同一篇不该出现两次");
        assert_eq!(idx.search(&toks("戊己庚辛"), crate::search::Scope::Any).unwrap().len(), 1);
        assert!(idx.search(&toks("甲乙丙丁"), crate::search::Scope::Any).unwrap().is_empty());
    }

    /// 移进回收站的要从索引里去掉,不然它还会被搜到
    #[test]
    fn 移走之后搜不到了() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "甲", &[], ""), "正文里提到所有权", 1, 100).unwrap();
        assert_eq!(idx.search(&toks("所有权"), crate::search::Scope::Any).unwrap().len(), 1);

        idx.remove("a").unwrap();

        assert!(idx.search(&toks("所有权"), crate::search::Scope::Any).unwrap().is_empty());
    }

    /// **认出哪些文件变了。** Quire 没有文件监控,用户在外面改了文件,
    /// 只能靠"重扫时对一遍 mtime 和大小"发现
    #[test]
    fn 记下时间戳好认出改动() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "甲", &[], ""), "正文", 12345, 678).unwrap();
        assert_eq!(idx.stamp("a"), Some((12345, 678)));
        assert_eq!(idx.stamp("b"), None, "没索引过就是 None");
    }

    #[test]
    fn 全清() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "甲", &[], ""), "正文", 1, 100).unwrap();
        idx.clear().unwrap();
        assert!(idx.is_empty());
    }

    #[test]
    fn 空查询不返回任何东西() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "甲", &[], ""), "正文", 1, 100).unwrap();
        assert!(idx.search(&[], crate::search::Scope::Any).unwrap().is_empty());
    }

    /// 查询里带 FTS5 的特殊字符不能炸。不包引号的话
    /// 「c++」这种查询会直接报语法错,表现是"搜索框一敲就弹红条"
    #[test]
    fn 查询里的特殊字符不炸() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "C++ 那点事", &[], ""), "讲的是 c++ 的历史", 1, 100).unwrap();

        for q in ["c++", "a*b", "(x)", "a-b", "\"引号\"", "NEAR(a b)"] {
            let r = idx.search(&[q.to_string()], crate::search::Scope::Any);
            assert!(r.is_ok(), "查询 {q:?} 报错了:{:?}", r.err());
        }
    }

    /// 引号本身要能搜到
    #[test]
    fn 引号能搜到() {
        let (_d, mut idx) = index();
        idx.upsert("a", &fm("x1", "他说了一句 \"你好\"", &[], ""), "正文", 1, 100).unwrap();
        let hits = idx.search(&["\"你好\"".to_string()], crate::search::Scope::Any).unwrap();
        assert_eq!(hits.len(), 1, "引号该是内容的一部分,不是语法");
    }

    /// 建表语句得幂等。反复打开同一个文件不能报错
    #[test]
    fn 反复打开不炸() {
        let d = TempDir::new().unwrap();
        let path = d.path().join("index.sqlite");
        for _ in 0..3 {
            Index::open(&path).unwrap();
        }
    }

    /// 索引和磁盘差得离谱时,清空重建比一篇篇对时间戳快。
    /// 这条测的是"重建这条路真的走得通"
    #[test]
    fn 清空重建走得通() {
        let (_d, mut idx) = index();
        for i in 0..5 {
            idx.upsert(&format!("a{i}"), &fm("x1", "所有权入门", &[], ""), "核心概念讲解", 1, 100).unwrap();
        }
        assert_eq!(idx.len(), 5);

        idx.clear().unwrap();
        // 同样是刻意不跟前面那五篇共享词条
        idx.upsert("a0", &fm("x1", "全新标题", &[], ""), "戊己庚辛", 9, 900).unwrap();

        assert_eq!(idx.len(), 1, "清空后该只剩重建进来的那一篇");
        assert_eq!(idx.search(&toks("戊己庚辛"), crate::search::Scope::Any).unwrap().len(), 1);
        assert!(
            idx.search(&toks("所有权"), crate::search::Scope::Any).unwrap().is_empty(),
            "清空重建之前那五篇不该还能搜到"
        );
    }

    // ── 范围 ────────────────────────────────────────────────────
    //
    // 范围只改 `WHERE` 里参与匹配的那几列。这一组盯的就是**列限定
    // 有没有写对**——写错一个字符不报错,表现是那个范围静默搜不到东西,
    // 用户只会觉得"这个范围不起作用"

    fn 范围库() -> (TempDir, Index) {
        let (d, mut idx) = index();
        idx.upsert(
            "a",
            &fm("x1", "标题里有苹果", &["标签是橘子"], "批注写着香蕉"),
            "正文里也有苹果",
            1,
            100,
        )
        .unwrap();
        (d, idx)
    }

    fn 范围命中(idx: &Index, scope: crate::search::Scope, q: &str) -> usize {
        idx.search(&toks(q), scope).unwrap().len()
    }

    /// **每个范围只认自己那一个字段。** 一条用例把五种范围都走一遍
    #[test]
    fn 每种范围只认自己那个字段() {
        let (_d, idx) = 范围库();
        use crate::search::Scope::*;

        // 同一个词分别出现在四个字段里
        assert_eq!(范围命中(&idx, Title, "苹果"), 1, "标题里有「苹果」");
        assert_eq!(范围命中(&idx, Body, "苹果"), 1, "正文里有「苹果」");
        assert_eq!(范围命中(&idx, Tag, "橘子"), 1, "标签里有「橘子」");
        assert_eq!(范围命中(&idx, Note, "香蕉"), 1, "批注里有「香蕉」");
    }

    /// 范围**不跨字段漏**。这条最要紧:列限定写宽了一点(比如把
    /// `tags_tok MATCH` 写进 Title 那一支),搜标题就会带出标签里才有的,
    /// 用户看到一堆没在标题里的结果,只会以为搜索坏了
    #[test]
    fn 范围不跨字段() {
        let (_d, idx) = 范围库();
        use crate::search::Scope::*;

        assert_eq!(范围命中(&idx, Title, "橘子"), 0, "标签里的漏进标题了");
        assert_eq!(范围命中(&idx, Title, "香蕉"), 0, "批注里的漏进标题了");
        assert_eq!(范围命中(&idx, Body, "橘子"), 0, "标签里的漏进正文了");
        assert_eq!(范围命中(&idx, Tag, "苹果"), 0, "标题正文里的漏进标签了");
        assert_eq!(范围命中(&idx, Note, "橘子"), 0, "标签里的漏进批注了");
    }

    /// 全字段搜四样都认。**这是范围的默认值**,它要是坏了,
    /// 用户什么都搜不到,而界面上看不出是"范围"出了问题
    #[test]
    fn 全范围四样都认() {
        let (_d, idx) = 范围库();
        for word in ["苹果", "橘子", "香蕉"] {
            assert_eq!(范围命中(&idx, crate::search::Scope::Any, word), 1, "搜「{word}」");
        }
    }

}


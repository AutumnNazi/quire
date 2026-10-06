//! 大批量剪藏库的性能实测。**只打印数字,不做断言**——
//! 这不是回归测试,是拿真实数据回答"这算不算问题"。
//!
//! 为什么单独跑而不是塞进 `cargo test`:
//!  - release 和 debug 的耗时差好几倍,debug 下的数字没有参考价值
//!  - 造 3000 篇要写 3000 个文件,在测试套件里当噪声不合适
//!
//! 跑法:`cargo test --release 实测 -- --ignored --nocapture --test-threads=1`

use std::time::Instant;

use crate::vault::{ClipInput, Vault};

/// 造 n 篇。**正文长度照真实的文章给**(几百字),用"甲"这种一两个字当正文
/// 会把文件读的开销压到几乎不见,量出来的数字会比真实场景好一大截——
/// 那正是这类实测最容易骗自己的地方
fn make(n: usize) -> (tempfile::TempDir, Vault) {
    let dir = tempfile::TempDir::new().expect("建临时目录");
    let v = Vault::new(dir.path());
    v.ensure_dirs().unwrap();
    let body = "所有权".repeat(200) + "\\n\\n" + &"这是一段模拟文章正文,".repeat(100);
    for i in 0..n {
        v.save(&ClipInput {
            schema_version: 1,
            url: format!("https://example{i}.com/article"),
            title: format!("第 {i} 篇:讲所有权和借用检查的那点事"),
            site_name: format!("example{i}.com"),
            author: None,
            excerpt: None,
            markdown: body.clone(),
            published_at: None,
            image: None,
            favicon: None,
        })
        .unwrap();
    }
    (dir, v)
}

#[test]
#[ignore = "耗时,手动跑"]
fn 实测列表扫描耗时() {
    for n in [200usize, 1000, 3000] {
        let (_d, v) = make(n);
        // 先跑一次把文件系统缓存热起来,量的才是"日常刷新"而不是"首次打开"
        let _ = v.scan().unwrap();
        let start = Instant::now();
        let result = v.scan().unwrap();
        let elapsed = start.elapsed();
        println!("{n:>5} 篇 · scan() {elapsed:>12.3?} · 每篇 {:?}", elapsed / n as u32);
        assert_eq!(result.clips.len(), n);
    }
}

#[test]
#[ignore = "耗时,手动跑"]
fn 实测导出耗时() {
    for n in [200usize, 1000, 3000] {
        let (_d, v) = make(n);
        let _ = v.export_markdown().unwrap();
        let start = Instant::now();
        let md = v.export_markdown().unwrap();
        println!("{n:>5} 篇 · 导出 {:>12} 字节 · {elapsed:?}", md.len(),
            elapsed = start.elapsed());
    }
}

#[test]
#[ignore = "耗时,手动跑"]
fn 实测全文搜索耗时() {
    use crate::search::{search, Scope};
    for n in [200usize, 1000, 3000] {
        let (_d, v) = make(n);
        let start = Instant::now();
        let out = search(&v, "所有权", 200, None).unwrap();
        let all = start.elapsed();
        let indexed_start = Instant::now();
        let out2 = search(&v, "所有权", 200, Some(Scope::Any)).unwrap();
        let scoped = indexed_start.elapsed();
        println!(
            "{n:>5} 篇 · 扫描路 {all:>12.3?} (命中 {}) · 索引路 {scoped:>12.3?} (命中 {})",
            out.hits.len(),
            out2.hits.len()
        );
    }
}

/// **一次列表刷新真正做的事**:scan 之外还有标签栏和筛选。
/// 分开量是因为它们走的是不同的文件访问模式——scan 读每个文件一次,
/// 而标签统计在早期版本里是把所有文件读了三遍
#[test]
#[ignore = "耗时,手动跑"]
fn 实测标签栏耗时() {
    for n in [200usize, 1000, 3000] {
        let (_d, v) = make(n);
        let _ = v.tag_index().unwrap();
        let start = Instant::now();
        let tags = v.tag_index().unwrap();
        println!("{n:>5} 篇 · tag_index() {:>12.3?} · {} 个标签", start.elapsed(), tags.len());
    }
}
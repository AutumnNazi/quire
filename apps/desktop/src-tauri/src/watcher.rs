//! 盯着剪藏库,磁盘上有变化就通知前端重扫。
//!
//! ## 为什么要有这个
//!
//! Quire 的东西就是磁盘上的 `.md`。用户在 Obsidian 里改了批注、
//! 用记事本删了一篇、或者把手机上的东西同步了下来——Quire 之前
//! **全都不知道**,只能靠 `F5`。这是"local-first"承诺的一个真实缺口:
//! 数据在磁盘上、随时可能被别的工具改,而软件看不见。
//!
//! ## 最难的那一处:别被自己写的盘触发
//!
//! Quire 自己每剪藏一次、标一次已读、改一次标签,都会写文件。
//! 如果照单全收,就是**每一次操作都紧跟着一次全量重扫**——
//! 用户会看到列表刚更新完又跳一下,CPU 一直在转,而且越用越卡。
//!
//! 解法是 [`WriteGuard`]:Quire 自己动笔之前先记下"我正在写",
//! 写完一小段时间之内到达的事件一律丢掉。
//!
//! **为什么是"一小段时间"而不是精确排除。** 事件回调和写操作之间
//! 没有可用的因果关系——操作系统的通知是异步到达的,可能在写盘之后
//! 几百毫秒才来。所以只能靠时间窗。窗开得太短会漏掉真改动,
//! 开得太长会吞掉用户在旁边的编辑。**3 秒是权衡出来的**:
//! 人手动改文件再保存的间隔远大于它,而一次剪藏引发的通知都在几十毫秒内
//!
//! **时间窗不等于万能。** 用户在 Quire 剪藏的同一瞬间手动改了同一个文件,
//! 那一次改动会被吞掉,下次重扫时才出现。这是已知代价,记在 README 里。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 写盘之后这段时间内到达的事件不算数
const QUIET_AFTER_WRITE: Duration = Duration::from_secs(3);

/// 批量攒一下再发。**编辑器保存一次可能触发好几条事件**
/// (写临时文件、改名、改时间戳),一条条转发等于转发的全是噪声
const DEBOUNCE: Duration = Duration::from_millis(400);

/// Quire 自己写过东西之后的一小段时间。
///
/// **用 `Instant` 而不是计数器。** 计数要靠"开始写"和"写完"两次调用配对,
/// 而中间那条路上一旦 panic 或者提前返回,计数就永远停在非零,
/// 之后所有外部改动都被静默吞掉——那种 bug 表现为"Quire 偶尔不更新了",
/// 极难查。`Instant` 每次都在往前推,不会卡住
#[derive(Clone)]
pub struct WriteGuard {
    last: Arc<AtomicU64>,
}

impl Default for WriteGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteGuard {
    pub fn new() -> Self {
        Self {
            last: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 记一笔"Quire 刚写过"。**所有会写盘的地方都要调**,
    /// 漏一个就是"剪藏之后列表莫名跳一下"
    pub fn note_write(&self) {
        // 从进程启动算起的毫秒数。u64 溢出要几亿年,不用管
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last.store(now, Ordering::Relaxed);
    }

    /// 这个事件该不该理会。
    ///
    /// **只按时间窗判,不看是哪个文件。** 精确排除路径需要 Quire 记得
    /// 自己写过哪些文件,而那个集合和实际写盘是两条独立的路径——
    /// 中间隔了一层就一定会对不上,漏一条的表现就是"莫名其妙不更新"
    pub fn should_ignore(&self) -> bool {
        let last = self.last.load(Ordering::Relaxed);
        if last == 0 {
            return false; // 还没写过任何东西,所有事件都算
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now.saturating_sub(last) < QUIET_AFTER_WRITE.as_millis() as u64
    }
}

/// 一个文件变了之后,该不该重扫。
///
/// **只看 `.md`。** `assets/` 里下图片、`.trash/` 里搬文件,都跟
/// 列表内容无关,为它们触发一次全量扫描是白花的时间
pub fn is_watchable(path: &Path) -> bool {
    // `clips/.trash/x.md` 和 `clips/assets/x/0.png` 都在 clips/ 底下,
    // 但都不该触发。**先按相对路径掐掉**,再谈扩展名
    let parts: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    for part in &parts {
        if part.starts_with('.') || part == "assets" {
            return false;
        }
    }
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return false;
    }
    // **锁文件和临时文件不算数。** Word/Excel 开着文档时会造一个 `~$名字.md`
    // 的锁文件,Quire 自己的 `write_atomic` 也会先写 `.md.tmp` 再改名。
    // 它们存在的时间只有几毫秒,可事件是在**改完之后**才到的——
    // 照单全收就是对着一个已经不存在的文件扫一遍整个库
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| !n.starts_with("~$") && !n.starts_with('~') && !n.starts_with('.'))
}

/// 一个文件事件该不该攒起来等一次重扫。
///
/// **抽出来是为了能测。** 这两件事各测各的都能过,合起来判断反了
/// ——或者更常见的:有人图省事改成"反正自己写的,顺手把攒着的清了",
/// 那一次改动的用户就得再手动刷一次才看得到
pub fn worth_noticing(guard: &WriteGuard, paths: &[PathBuf]) -> bool {
    !guard.should_ignore() && paths.iter().any(|p| is_watchable(p))
}

/// 攒够一个静默期就报一次。**空着的时候睡这么久。**
///
/// **很长是故意的,而且是重点。** 没挂着事件的时候,监听线程手上
/// **没有任何东西需要判断**——事件来了是通道把它叫醒的,不是靠它轮询。
/// 而监控是要**整天开着**的(它本来就是这个产品零安装门槛的唯一实现路径),
/// 定频一秒一醒就是一天八万多次空转。
///
/// 事件真没来的时候也不需要它:这一条只决定"万一通知丢了"的兜底频率,
/// 而 `Debouncer::waiting()` 那边已经保证挂着的时候按差值醒,不会误报
pub const IDLE_WAIT: Duration = Duration::from_secs(3600);

/// 攒够一个静默期就报一次。
///
/// **攒的是"有没有新事件",不攒事件本身。** 用户改了五个文件,
/// Quire 只需要知道"该重扫了"——具体哪些文件变了,重扫时自然会看到。
/// 攒文件列表反而要在锁里攒,还得处理一半是临时文件的垃圾
#[derive(Default)]
pub struct Debouncer {
    pending: bool,
    since: Option<Instant>,
}

impl Debouncer {
    /// 记一个事件进来。返回 true 表示**现在就可以报了**
    pub fn push(&mut self) -> bool {
        if self.since.is_none() {
            self.since = Some(Instant::now());
        }
        self.pending = true;
        self.ready()
    }

    /// 该报了就报,并清空。**调用方在定时器里问它**
    pub fn ready(&mut self) -> bool {
        match self.since {
            Some(t) if t.elapsed() >= DEBOUNCE => {
                self.reset();
                true
            }
            _ => false,
        }
    }

    /// 还挂着但没攒够静默期。**用来决定要不要发心跳**——
    /// 线程得定期醒来看一眼,否则事件到了也没人问
    pub fn waiting(&self) -> Option<Duration> {
        self.since.map(|t| DEBOUNCE.saturating_sub(t.elapsed()))
    }

    pub fn reset(&mut self) {
        self.pending = false;
        self.since = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }
}

/// 该监听哪些目录。
///
/// **只监听 `clips/` 那一层**,不递归。`assets/` 在它底下但不需要看,
/// `.trash` 也是。递归监听的代价是每次事件都要走完整棵子树,
/// 而剪藏库越大越慢
pub fn watch_targets(root: &Path) -> Vec<PathBuf> {
    vec![root.to_path_buf()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 刚写过的静默() {
        let g = WriteGuard::new();
        assert!(!g.should_ignore(), "还没写过东西,不该静默");
        g.note_write();
        assert!(g.should_ignore(), "写完立刻来的事件要静默");
    }

    /// **那个计数卡死的坑。** 模拟"很久以前写的",不该再静默——
    /// 正是它保证用户手动改的文件最终一定会被看到
    #[test]
    fn 很久以前写的就不再静默() {
        let g = WriteGuard::new();
        g.note_write();
        // 把时间戳改到一小时前
        let hour_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            - 3_600_000;
        g.last.store(hour_ago, Ordering::Relaxed);
        assert!(!g.should_ignore(), "过了一小时还不静默,用户的手工改动就永远看不见了");
    }

    /// **两处写盘之间的时间窗要能自动滑走。** 连续剪藏两次之后,
    /// 时间戳被推到了第二次,窗口跟着往后挪
    #[test]
    fn 连续写也只静默最后那一小段() {
        let g = WriteGuard::new();
        g.note_write();
        std::thread::sleep(Duration::from_millis(30));
        g.note_write();
        assert!(g.should_ignore(), "刚写完还在窗内");
        // 窗口是从最后一次写算起的
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        g.last.store(now - (QUIET_AFTER_WRITE.as_millis() as u64 + 1000), Ordering::Relaxed);
        assert!(!g.should_ignore());
    }

    #[test]
    fn 只认剪藏那一层的md() {
        assert!(is_watchable(Path::new("C:/v/clips/2026-01-01-a-b-c.md")));
    }

    /// **回收站和图片不触发重扫。** 它们跟列表内容无关,为它们
    /// 扫一遍整个库是白花的时间——而删一篇恰恰是最频繁的操作之一
    #[test]
    fn 回收站和图片不触发() {
        assert!(!is_watchable(Path::new("C:/v/clips/.trash/a.md")));
        assert!(!is_watchable(Path::new("C:/v/clips/.deleted/a.md")));
        assert!(!is_watchable(Path::new("C:/v/clips/assets/m8x2/0.png")));
        assert!(!is_watchable(Path::new("C:/v/clips/a.txt")));
    }

    /// **临时文件不算数。** 编辑器和 Quire 自己保存时都会先写
    /// `.tmp` 再改名,那些中间态触发重扫只会扫到一个不存在的文件
    #[test]
    fn 临时文件不算() {
        assert!(!is_watchable(Path::new("C:/v/clips/a.md.tmp")));
        assert!(!is_watchable(Path::new("C:/v/clips/~$a.md")));
    }

    #[test]
    fn 攒够静默期才报() {
        let mut d = Debouncer::default();
        assert!(!d.is_pending(), "还没事件");
        d.push();
        assert!(d.is_pending());
        assert!(!d.ready(), "刚来就该报,那是一次文件改出三条通知的由来");
        std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
        assert!(d.ready(), "静默够了就该报");
        assert!(!d.is_pending(), "报过就清了");
    }

    /// **一串事件只报一次。** 用户改了五个文件,Quire 重扫一次就够,
    /// 不是五次
    #[test]
    fn 一串事件只报一次() {
        let mut d = Debouncer::default();
        for _ in 0..5 {
            d.push();
        }
        std::thread::sleep(DEBOUNCE + Duration::from_millis(50));
        assert!(d.ready());
        assert!(!d.ready(), "报过之后不该还报");
    }

    /// **有事件挂着时要说出来还差多久**,线程才知道要不要先睡一觉。
    /// 报 0 的话线程会空转
    #[test]
    fn 挂着的时候说得出差多久() {
        let mut d = Debouncer::default();
        d.push();
        let left = d.waiting();
        assert!(left.is_some(), "挂着就该有等待时间");
        assert!(left.unwrap() <= DEBOUNCE, "不能等得比静默期还久");
        assert!(d.waiting().unwrap() > Duration::ZERO, "刚来的时候不该是 0");
    }

    /// **空等的间隔必须长。** 这个数就是"万一事件丢了"的兜底频率,
/// 写成几百毫秒就等于把监控变成永不停歇的轮询——而监控是整天开着的
#[test]
    fn 空等间隔不能是轮询级别() {
        assert!(
            IDLE_WAIT >= std::time::Duration::from_secs(30),
            "空等间隔只有 {:?},这跟每秒轮询一次没区别,一天白醒八万多次",
            IDLE_WAIT
        );
    }

    #[test]
    fn 只监听剪藏那一层() {
        let t = watch_targets(Path::new("C:/v/clips"));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0], Path::new("C:/v/clips"));
    }

    /// **用户改的收、自己写的不收。** 这一条写错了监控就是废的:
    /// 收自己写的 → 每次剪藏都全量重扫;连自己的都收 → 用户在旁边
    /// 改文件永远看不见
    #[test]
    fn 外部改动收自己写的丢() {
        let g = WriteGuard::new();
        let external = vec![PathBuf::from("C:/v/clips/a.md")];
        assert!(worth_noticing(&g, &external), "还没写过东西,用户改的都算");

        g.note_write();
        assert!(
            !worth_noticing(&g, &external),
            "自己写盘引发的那条通知不能再攒一次重扫"
        );
    }

    /// **静默期过了之后,用户的改动又算数了。** 前面那条测试单独存在的话,
    /// 只要实现里把 `should_ignore` 写死成 true,它照样是绿的
    #[test]
    fn 静默期过了外部改动又算数() {
        let g = WriteGuard::new();
        g.note_write();
        g.last.store(0, Ordering::Relaxed);
        assert!(
            worth_noticing(&g, &[PathBuf::from("C:/v/clips/a.md")]),
            "时间窗过了还收不到,用户手改的文件就永远不出现了"
        );
    }

    /// 回收站、图片、临时文件那条路也不该触发重扫
    #[test]
    fn 无关文件不进重扫() {
        let g = WriteGuard::new();
        assert!(!worth_noticing(&g, &[PathBuf::from("C:/v/clips/.trash/a.md")]));
        assert!(!worth_noticing(&g, &[PathBuf::from("C:/v/clips/assets/m/0.png")]));
        assert!(!worth_noticing(&g, &[]));
    }
}
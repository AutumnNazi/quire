//! Windows 上的剪贴板变更监听。
//!
//! ## 为什么不是每 700ms 问一次
//!
//! 轮询有两个问题,第二个比第一个严重:
//!  1. **延迟。** 最坏情况半秒多,用户复制完切窗口,提示还在路上
//!  2. **一直在烧 CPU。** 监控是要**长期开着**的——它本来就是这个产品
//!     零安装门槛的唯一实现路径,用户装好就开着,一整天那么开着。
//!     每秒 1.4 次剪贴板读取,一天的量是十几万次
//!
//! ## 怎么做的
//!
//! `AddClipboardFormatListener` 让系统在一个窗口上发 `WM_CLIPBOARDUPDATE`。
//! 关键在于**那个窗口得有自己的消息循环**——消息只在循环里被派发,
//! 而 Tauri 的主循环在另一个线程,拿不到我们这里的窗口消息。
//!
//! ## 退路是必须的
//!
//! 注册可能失败:会话 0、远程桌面断开、极老的 Windows。
//! **失败就退回轮询**——宁可慢也不能不响应。监控失效意味着用户复制的东西
//! 根本不弹提示,而那是这个软件唯一的使用方式
//!
//! 即便监听成了,循环里也留着一条 30 秒的兜底心跳。万一某条通知没派发到
//! (系统忙、消息被合并),下一次轮询还能捞回来。用户看到的差别只是
//! 「慢了几秒」和「彻底不弹」,中间没有过渡

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LRESULT};
// 这几个**不在一起**:剪贴板监听在 `System::DataExchange`(名字起得莫名其妙,
// 但它就是在那儿),窗口那套在 `UI::WindowsAndMessaging`。
// 按"都在 UI 底下"的直觉去 import 会编不过,而报错信息是一长串
// unresolved import,看不出该去哪儿找
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    RegisterClassW, TranslateMessage, PostQuitMessage, WM_APP, WM_CLIPBOARDUPDATE, WM_DESTROY,
    WNDCLASSW,
};

/// 我们自己的消息。**自己发给自己**,用来让消息循环停下来
const WM_STOP: u32 = WM_APP + 1;

const CLASS_NAME: &[u16] = &[
    b'Q' as u16, b'u' as u16, b'i' as u16, b'r' as u16, b'e' as u16, b'C' as u16, b'l' as u16, 0,
];

/// 兜底心跳。**只在这条路上有**——监听成功的话这条根本不该常驻,
/// 但它也不该彻底没有:某条通知没派发到的场合,它是唯一的救命稻草
pub(crate) const FALLBACK_POLL: std::time::Duration = std::time::Duration::from_secs(30);

/// 窗口句柄。**必须是全局的,不能放 `thread_local`。**
///
/// 它是消息循环那条线程建起来的,而 `stop()` 是在 Tauri 命令线程上调的
/// ——放线程局部的话,`stop()` 读到的永远是本线程的初始值(空),
/// 函数看着干完了,实际上一条消息也没发出去。用户每开关一次监控,
/// 就多留一个线程、一个窗口句柄,和一条注册在系统上的剪贴板监听。
/// 症状是"关了监控之后 CPU 一直有占用,而且越用越慢",
/// 而界面上那个开关明明是关着的
static HANDLE: AtomicIsize = AtomicIsize::new(0);

/// 消息循环是不是真的在跑。
///
/// **不能拿 `HANDLE` 非空当"活着"。** 循环退出之后句柄还留在那儿,
/// 而那个"兜底心跳"问的就是这句话——问错了会让一条已经死掉的路径
/// 一直以为自己还活着,30 秒醒一次,永远醒着
static ALIVE: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            // **这里只敲个门,什么都不做。** 窗口过程跑在消息循环线程上,
            // 而读剪贴板要开 OLE、可能卡几十毫秒。在这儿读就是让消息队列
            // 停摆几十毫秒,更糟的是连 `WM_STOP` 都送不进来——用户点关闭
            // 监控,那个窗口永远等不到停下的消息,线程也就永远退不了
            signal();
            0
        }
        WM_STOP => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        // 其余一律交给系统。**吞掉不认识的窗口消息**是这类程序最常见的毛病:
        // 吞掉 WM_SIZE 会让窗口表现得很怪,而那个现象和剪贴板毫无关系,
        // 查起来绕一大圈
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 往干活的线程那儿递一个信号。
///
/// **无界队列的 `send` 不会等。** 队列是通的就没有等不到的时候——
/// 要真断了,那说明干活的那个线程已经退了,这次通知也确实没地方送。
/// 这条路必须快:它跑在窗口过程里,慢了就等于堵住整条消息队列
fn signal() {
    SIGNAL.with(|s| {
        if let Some(tx) = s.borrow().as_ref() {
            let _ = tx.send(());
        }
    });
}

thread_local! {
    /// 窗口过程拿不到闭包里的东西,而它要往干活的那个线程递信号。
    /// 窗口和这条通道 1:1,线程局部存储刚好合适
    static SIGNAL: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> =
        const { std::cell::RefCell::new(None) };
}

/// 起监听。**失败返回 None**,调用方退回轮询
pub(crate) fn start(state: Arc<crate::WatchState>) -> Option<()> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
    let (sig_tx, sig_rx) = std::sync::mpsc::channel::<()>();

    // **干活的线程,和消息循环分开。** 监听线程只负责"有事发生"这一个信号,
    // 读剪贴板、算指纹、发事件全在这儿做
    std::thread::spawn(move || {
        while sig_rx.recv().is_ok() {
            // 拿不到 AppHandle 就什么都做不了。**这不该发生**——
            // 监听是启动时起的,那之前 AppHandle 已经存好了
            if let Some(app) = crate::clip_app() {
                crate::on_clipboard_changed(&app, &state);
            }
        }
    });

    std::thread::spawn(move || {
        SIGNAL.with(|s| *s.borrow_mut() = Some(sig_tx));
        let Some(hwnd) = (unsafe { build_window(&ready_tx) }) else {
            // 通道的发送端跟着这条线程走。它一退,干活那边的 `recv` 就返回错,
            // 那个线程跟着一起退——**不留一个光醒不干活的后台线程**
            return;
        };
        ALIVE.store(true, Ordering::SeqCst);
        unsafe { message_loop(hwnd) };
        ALIVE.store(false, Ordering::SeqCst);
    });

    match ready_rx.recv() {
        Ok(true) => Some(()),
        // **注册失败就返回 None**,由调用方退回轮询。宁可慢也不能不响应——
        // 监控失效意味着用户复制的东西根本不弹提示,而那是这个软件唯一的使用方式
        Ok(false) | Err(_) => None,
    }
}

/// 建窗口并挂上监听。**成不成用返回值说**,不走那条信号通道——
/// 信号通道给的是"有事发生了",不是"准备好了"
unsafe fn build_window(ready: &std::sync::mpsc::Sender<bool>) -> Option<HWND> {
    let instance: HINSTANCE = GetModuleHandleW(std::ptr::null());
    let class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: CLASS_NAME.as_ptr(),
        ..std::mem::zeroed()
    };
    // 重复注册返回 0 而**不算错**——类可能已经在了。用 GetLastError 分不清
    // 「已注册」和「真失败」,所以这里靠后面的建窗口再判断一次
    if RegisterClassW(&class) == 0 {
        // 继续往下走:万一只是重复注册,窗口照样建得起来
    }
    let hwnd = CreateWindowExW(
        0,
        CLASS_NAME.as_ptr(),
        std::ptr::null(),
        0, // 不显示
        0,
        0,
        0,
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        let _ = ready.send(false);
        return None;
    }
    if AddClipboardFormatListener(hwnd) == 0 {
        let _ = ready.send(false);
        DestroyWindow(hwnd);
        return None;
    }
    HANDLE.store(hwnd as isize, Ordering::SeqCst);
    let _ = ready.send(true);
    Some(hwnd)
}

unsafe fn message_loop(hwnd: HWND) {
    let mut msg = std::mem::zeroed();
    loop {
        // 返回 0 表示 quit,-1 表示出错。**两种都得出循环**——
        // 出错不跳就是死循环,症状是线程卡住、监听静默失效
        match GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) {
            0 | -1 => break,
            _ => {}
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    // **退出前注销。** 不注销的话系统那边还挂着一个指向已销毁窗口的
    // 指针,用户下一次复制就会让系统去写一块已经还给系统的内存
    let _ = RemoveClipboardFormatListener(hwnd);
    DestroyWindow(hwnd);
}

/// 让监听停下来。**重复调用没事**,`HANDLE` 一swap 就空,第二次压根不发消息
pub(crate) fn stop() {
    // **先把句柄拿走再发消息。** 反过来的话,消息循环里那句"取句柄去注销"
    // 会读到已经被换掉的空值,监听就永远留在系统列表里了
    let hwnd = HANDLE.swap(0, Ordering::SeqCst) as HWND;
    if !hwnd.is_null() {
        unsafe {
            // WParam / LParam 是类型别名(就是 isize),不是新类型,
            // 所以不能 `WParam(0)`——那是把它当函数调了。字面量 0
            // 自己会推成正确的类型
            PostMessageW(hwnd, WM_STOP, 0, 0);
        }
    }
}

/// 消息循环是不是还在跑。**兜底心跳每 30 秒问一次**,
/// 答不上来就说明这条路已经断了,那条兜底路也就不必再醒
pub(crate) fn alive() -> bool {
    ALIVE.load(Ordering::SeqCst)
}


#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    /// 这两条都动全局句柄,得串着来,否则一条把另一条的句柄换掉了
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **`stop()` 得能从别的线程把句柄要回来。**
    ///
    /// 句柄是消息循环那条线程建起来的,而 `stop()` 是在 Tauri 命令线程上
    /// 调的——放 `thread_local` 的话这里读到的永远是空的,函数干完活什么
    /// 也没停。用户每开关一次监控就多留一个线程和一个窗口,
    /// 而界面上那个开关显示的是关着的
    #[test]
    fn 停掉之后句柄就交回去了() {
        let _g = lock();
        // 塞一个不存在的句柄。PostMessageW 对着它失败返回 0,
        // 而 `stop()` 该做的另一半——把句柄交回去——照样得做
        HANDLE.store(0x1234_5678, Ordering::SeqCst);
        // **在别的线程上停**,正是原先漏掉的那一处
        std::thread::spawn(stop).join().unwrap();
        assert_eq!(
            HANDLE.load(Ordering::SeqCst),
            0,
            "句柄没交回去:下一次开关监控会再建一个窗口,旧的那个连着线程一起留在后台"
        );
    }

    /// 真起一次监听,再真停一次。
    ///
    /// **默认连编译都不进,得显式叫。** 这条碰 `AppHandle`,而 `AppHandle`
    /// 一进测试二进制,本机 link 出来的 exe 就在加载阶段
    /// STATUS_ENTRYPOINT_NOT_FOUND,连 "running 1 test" 都来不及打印,
    /// 整轮 400 多个测试跟着一起归零。
    ///
    /// `#[ignore]` 躲不掉这个——它只是不执行,代码照样被链接进去。
    /// 只能靠 feature 让它**根本不编译**。想跑就:
    ///
    /// ```text
    /// cargo test --features live-clipwatch -- --ignored clipwatch
    /// ```
    #[cfg(feature = "live-clipwatch")]
    #[test]
    #[ignore = "碰 AppHandle,会让本机的测试二进制在加载阶段就起不来"]
    fn 真起真停之后不算活着了() {
        let _g = lock();
        HANDLE.store(0, Ordering::SeqCst);
        ALIVE.store(false, Ordering::SeqCst);

        let state = Arc::new(crate::WatchState::default());
        if start(state).is_none() {
            // 会话 0、远程桌面断开、跑在没桌面的 CI 上都会走到这。
            // **大声说出来,别静悄悄当成通过**——这条没验过就是没验过
            eprintln!("⚠️ 这个环境注册不了剪贴板监听(多半是没有桌面会话),跳过 真起真停之后不算活着了");
            return;
        }
        assert!(
            alive(),
            "注册成了就该算活着,不然那条 30 秒的兜底路不会去补漏"
        );

        stop();
        // 消息循环收到 WM_STOP 要走一趟派发,给点时间
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(), "停不掉:线程和窗口会一直留在后台,越开关越慢");
    }
}

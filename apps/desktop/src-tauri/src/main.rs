// 发布的 exe 不该带一个控制台窗口,只在 debug 构建时保留
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    quire_lib::run();
}

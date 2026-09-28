/**
 * Service worker:只负责把点击转成消息,以及把结果显示在图标上。
 *
 * 刻意保持成一个不依赖任何东西的普通脚本。真正需要打包的只有 content.js,
 * 单独构建成 IIFE 就不会触发 Rollup 的代码分割——内容脚本在 MV3 里
 * 不支持 ES module,一旦分出共享 chunk 整条链路就废了。
 */

const OK_COLOR = "#2f6f4f";
const ERR_COLOR = "#b3261e";
const DEFAULT_TITLE = "剪藏整页到剪贴板";

chrome.action.onClicked.addListener(async (tab) => {
  if (tab.id === undefined) return;
  try {
    const reply = await chrome.tabs.sendMessage(tab.id, { type: "quire-clip" });
    if (reply && reply.ok) {
      flash(OK_COLOR, "✓", `已放进剪贴板:${reply.title}`);
    } else {
      flash(ERR_COLOR, "!", (reply && reply.error) || "这一页抽不出正文");
    }
  } catch {
    // content script 没注入:chrome:// 、扩展商店、PDF 查看器这类页面
    // 扩展本来就跑不了。这不是故障,说清楚就行了,别弹一堆吓人的报错。
    flash(ERR_COLOR, "!", "这个页面不让扩展运行(试试普通网页,地址别是 chrome:// 开头)");
  }
});

function flash(color, text, title) {
  chrome.action.setBadgeText({ text });
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setTitle({ title });
  setTimeout(() => {
    chrome.action.setBadgeText({ text: "" });
    chrome.action.setTitle({ title: DEFAULT_TITLE });
  }, 2500);
}

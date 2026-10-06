/**
 * Service worker:只负责把点击转成消息,以及把结果显示在图标上。
 *
 * 刻意保持成一个不依赖任何东西的普通脚本。真正需要打包的只有 content.js,
 * 单独构建成 IIFE 就不会触发 Rollup 的代码分割——内容脚本在 MV3 里
 * 不支持 ES module,一旦分出共享 chunk 整条链路就废了。
 *
 * 所以下面这个 `t()` 是 `src/i18n.ts` 的手抄版,不是 import。**两份必须
 * 一起改**:`i18n.test.ts` 会核对 `background.js` 里用到的键在两份词典里
 * 都在,少一个就红。
 *
 * (注释里别写带星号的通配路径,`_locales` 后面接一个星号那种。它里面
 * 藏着"星号紧跟斜杠",会把剥注释的扫描器提前甩出去,后面整段当代码解析。)
 */

const OK_COLOR = "#2f6f4f";
const ERR_COLOR = "#b3261e";

/** 取一句文案。查不到返回空串——键名漏进界面比一句英文还难懂。 */
function t(key, substitutions) {
  return chrome.i18n.getMessage(key, substitutions || []);
}

chrome.action.onClicked.addListener(async (tab) => {
  if (tab.id === undefined) return;
  try {
    const reply = await chrome.tabs.sendMessage(tab.id, { type: "quire-clip" });
    if (reply && reply.ok) {
      flash(OK_COLOR, "✓", t("toast_saved", [reply.title || ""]));
    } else {
      // 内容脚本给的是**已经翻好的句子**,直接显示。查不到内容脚本的回复
      // (比如它自己崩了)时兜底,别留个空提示让人以为剪藏成功了
      flash(ERR_COLOR, "!", (reply && reply.error) || t("error_no_content"));
    }
  } catch {
    // content script 没注入:chrome:// 、扩展商店、PDF 查看器这类页面
    // 扩展本来就跑不了。这不是故障,说清楚就行了,别弹一堆吓人的报错。
    flash(ERR_COLOR, "!", t("error_page_blocked"));
  }
});

function flash(color, text, title) {
  chrome.action.setBadgeText({ text });
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setTitle({ title });
  setTimeout(() => {
    chrome.action.setBadgeText({ text: "" });
    chrome.action.setTitle({ title: t("action_title") });
  }, 2500);
}

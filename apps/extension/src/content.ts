// 必须从 /full 入口导入。主入口("defuddle")没有接 Markdown 转换,
// separateMarkdown 会静默失效、contentMarkdown 恒为 undefined,
// 于是每一次剪藏都报"抽不出正文"。
import Defuddle from "defuddle/full";
import { buildPayload, prepareDocumentForExtraction, type Extracted } from "./extract";
import { t } from "./i18n";

interface ClipRequest {
  type: "quire-clip";
}

interface ClipReply {
  ok: boolean;
  title?: string;
  error?: string;
}

chrome.runtime.onMessage.addListener((message: unknown, _sender, sendResponse) => {
  const request = message as ClipRequest | undefined;
  if (request?.type !== "quire-clip") return undefined;

  try {
    sendResponse(clipCurrentPage());
  } catch (err) {
    sendResponse({ ok: false, error: err instanceof Error ? err.message : String(err) });
  }
  // 同步回话,不用 return true 占住通道
  return undefined;
});

function clipCurrentPage(): ClipReply {
  // 先在页面的克隆上做预处理(懒加载图片、媒体转链接),再交给 Defuddle。
  // **不能把预处理做在页面本身上**:用户还开着这页,当着他的面把视频换成
  // 一条链接是惊吓。Defuddle 拿到的是克隆,页面原样不动。
  const prepared = prepareDocumentForExtraction(document, location.href, {
    video: t("media_video"),
    audio: t("media_audio"),
    embed: t("media_embed"),
    videoRemote: t("media_video_remote"),
    audioRemote: t("media_audio_remote"),
    embedRemote: t("media_embed_remote"),
  });
  // separateMarkdown 一次给出两份:content 是干净 HTML,contentMarkdown 是
  // Markdown。**不能换成 markdown: true**——那样 content 变 Markdown,
  // 就拿不到给别的应用用的 HTML 了,两个一起开更会互相打架。
  const result = new Defuddle(prepared, {
    url: location.href,
    separateMarkdown: true,
  }).parse();

  const markdown = (result.contentMarkdown ?? "").trim();
  if (!markdown) {
    // 登录页、搜索结果页、纯图片站都走这条路
    return { ok: false, error: t("error_no_content") };
  }

  const payload = buildPayload(result as Extracted, {
    content: result.content,
    markdown,
    url: location.href,
    fallbackTitle: document.title || location.hostname,
  });

  if (!writeToClipboard(payload.html, payload.markdown)) {
    return { ok: false, error: t("error_clipboard") };
  }
  return { ok: true, title: payload.metas.find(([k]) => k === "quire-title")?.[1] };
}

/**
 * 写剪贴板。
 *
 * 走 `execCommand("copy")` 而不是 `navigator.clipboard.write`:后者要求
 * 文档处在用户手势上下文里,从 service worker 发起的调用会被拒。前者
 * 只要页面本身有焦点就能成,而点扩展图标并不会把焦点从网页上拿走。
 */
function writeToClipboard(html: string, text: string): boolean {
  const onCopy = (event: ClipboardEvent) => {
    // 不 preventDefault 的话浏览器会拿页面选中内容盖掉我们写的数据
    event.preventDefault();
    event.clipboardData?.setData("text/html", html);
    event.clipboardData?.setData("text/plain", text);
  };

  document.addEventListener("copy", onCopy);
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  } finally {
    document.removeEventListener("copy", onCopy);
  }
  return ok;
}

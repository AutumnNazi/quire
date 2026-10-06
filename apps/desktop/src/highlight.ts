/** 命中的字符区间。`[起, 止)` 都是**字符**下标,和 Rust 侧一致。
 *
 *  用字符不用字节:中文一个字符三个字节,按字节切会切在半个字上,
 *  界面上就是一个乱码方块 */
export type MarkRange = [number, number];

/** 一段带命中位置的文本。搜索结果才有 `marks`,普通列表的摘要没有 */
export interface SubText {
  text: string;
  marks: MarkRange[];
}

/** 没有命中位置的普通文本。**统一走这个函数**,免得在每个调用点
 * 写一遍 `{ text, marks: [] }`,然后某天有人传了个错的对齐 */
export function plain(text: string | null): SubText | null {
  return text === null ? null : { text, marks: [] };
}

/** 洗一遍区间:丢掉不合法的,再把重叠和挨着的并成一段。
 *
 *  **合并必须在渲染之前做完。** 渲染的时候再想"补长上一段",就得回头去切
 *  原文、还得记住上一段已经吞掉多少——那是个必然写错的算法。合并成一段
 *  就简单了:后一段接不上就新开一段,接得上就延长上一段 */
function normalize(text: string, marks: MarkRange[]): MarkRange[] {
  const chars = Array.from(text);
  const valid = marks
    .filter(
      ([from, to]) =>
        Number.isInteger(from) &&
        Number.isInteger(to) &&
        from >= 0 &&
        to > from &&
        to <= chars.length,
    )
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: MarkRange[] = [];
  for (const [from, to] of valid) {
    const last = merged[merged.length - 1];
    // `from <= last[1]` 是"挨着或重叠"。二元组切词天然重叠,用户搜中文
    // 几乎每一条都会撞上,不合并的话界面上是两段套着的底色、中间一道接缝
    if (last && from <= last[1]) last[1] = Math.max(last[1], to);
    else merged.push([from, to]);
  }
  return merged;
}

/** 把一段文字按命中区间拆成可拼的节点。
 *
 *  **这里不碰 `innerHTML`。** 标题和摘要都来自用户浏览过的网页,那里面
 *  可能有 `<script>`;高亮是最容易被人顺手写成 `innerHTML` 的地方,
 *  所以这个函数只认 `createTextNode`,从根上不给那条路
 *
 *  越界的区间直接丢掉而不是尽量凑合:**标错的比不标的更糟**——
 *  用户点进去发现高亮的那段跟说的不是一回事,比没有高亮糟糕得多
 */
export function highlight(text: string, marks: MarkRange[]): Node[] {
  const ranges = normalize(text, marks);
  if (ranges.length === 0) return [document.createTextNode(text)];
  const chars = Array.from(text);
  const out: Node[] = [];
  let at = 0;
  for (const [from, to] of ranges) {
    if (from > at) out.push(document.createTextNode(chars.slice(at, from).join("")));
    const mark = document.createElement("mark");
    mark.className = "hit";
    mark.textContent = chars.slice(from, to).join("");
    out.push(mark);
    at = to;
  }
  if (at < chars.length) out.push(document.createTextNode(chars.slice(at).join("")));
  return out;
}
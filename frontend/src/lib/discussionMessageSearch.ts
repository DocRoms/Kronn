/** Build DOM ranges from rendered Markdown, including matches that span two
 * adjacent text nodes (for example `foo **bar**`). */
export function findRenderedTextRanges(root: HTMLElement, query: string): Range[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [];

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Array<{ node: Text; start: number; end: number }> = [];
  let fullText = '';
  let current = walker.nextNode();
  while (current) {
    const node = current as Text;
    const value = node.data;
    if (value) {
      const start = fullText.length;
      fullText += value;
      nodes.push({ node, start, end: fullText.length });
    }
    current = walker.nextNode();
  }

  const haystack = fullText.toLocaleLowerCase();
  const ranges: Range[] = [];
  let offset = 0;
  while (offset <= haystack.length - needle.length) {
    const found = haystack.indexOf(needle, offset);
    if (found < 0) break;
    const end = found + needle.length;
    const startNode = nodes.find(part => found >= part.start && found < part.end);
    const endNode = nodes.find(part => end > part.start && end <= part.end);
    if (startNode && endNode) {
      const range = document.createRange();
      range.setStart(startNode.node, found - startNode.start);
      range.setEnd(endNode.node, end - endNode.start);
      ranges.push(range);
    }
    offset = found + Math.max(needle.length, 1);
  }
  return ranges;
}

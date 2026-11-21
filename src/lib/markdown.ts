// Dependency-free Markdown → safe HTML renderer for the editor preview.
// Everything is HTML-escaped first, then structural markdown becomes a small
// allow-list of tags, so user content can never inject markup or scripts.

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function safeHref(url: string): string | null {
  const value = url.trim();
  if (/^(https?:\/\/|mailto:)/i.test(value)) return value;
  // Relative links stay relative; everything else (javascript:, data:, …) is dropped.
  if (/^[\w./#-]+$/.test(value)) return value;
  return null;
}

function inline(text: string): string {
  let out = escapeHtml(text);
  // Inline code first so its content is not re-processed.
  const codeSlots: string[] = [];
  out = out.replace(/`([^`\n]+)`/g, (_m, code: string) => {
    codeSlots.push(`<code>${code}</code>`);
    return `\u0000${codeSlots.length - 1}\u0000`;
  });
  out = out.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>');
  out = out.replace(/(^|[\s(])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  out = out.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (_match, label: string, href: string) => {
    const safe = safeHref(href.replace(/&amp;/g, '&'));
    return safe ? `<a href="${escapeHtml(safe)}">${label}</a>` : label;
  });
  return out.replace(/\u0000(\d+)\u0000/g, (_m, index: string) => codeSlots[Number(index)] ?? '');
}

/// Renders a limited, safe subset: headings, fenced code, blockquotes, lists,
/// hr, paragraphs. Tables/footnotes pass through as plain text.
export function markdownToHtml(markdown: string): string {
  const lines = markdown.replace(/\r\n?/g, '\n').split('\n');
  const blocks: string[] = [];
  let paragraph: string[] = [];
  let list: { ordered: boolean; items: string[] } | null = null;
  let code: { lang: string; body: string[] } | null = null;
  let quote: string[] = [];

  const flushParagraph = () => { if (paragraph.length) { blocks.push(`<p>${inline(paragraph.join(' '))}</p>`); paragraph = []; } };
  const flushList = () => { if (list) { blocks.push(`<${list.ordered ? 'ol' : 'ul'}>${list.items.map(item => `<li>${inline(item)}</li>`).join('')}</${list.ordered ? 'ol' : 'ul'}>`); list = null; } };
  const flushQuote = () => { if (quote.length) { blocks.push(`<blockquote><p>${inline(quote.join(' '))}</p></blockquote>`); quote = []; } };
  const flushAll = () => { flushParagraph(); flushList(); flushQuote(); };

  for (const raw of lines) {
    if (code) {
      if (/^```/.test(raw.trim())) { blocks.push(`<pre><code>${escapeHtml(code.body.join('\n'))}</code></pre>`); code = null; }
      else code.body.push(raw);
      continue;
    }
    const fence = raw.trim().match(/^```(\w*)$/);
    if (fence) { flushAll(); code = { lang: fence[1] ?? '', body: [] }; continue; }
    if (!raw.trim()) { flushAll(); continue; }
    const heading = raw.match(/^(#{1,6})\s+(.*)$/);
    if (heading) { flushAll(); const level = heading[1].length; blocks.push(`<h${level}>${inline(heading[2])}</h${level}>`); continue; }
    if (/^(---+|\*\*\*+|___+)\s*$/.test(raw.trim())) { flushAll(); blocks.push('<hr>'); continue; }
    const quoteLine = raw.match(/^>\s?(.*)$/);
    if (quoteLine) { flushParagraph(); flushList(); quote.push(quoteLine[1]); continue; }
    const unordered = raw.match(/^\s*[-*+]\s+(.*)$/);
    const ordered = raw.match(/^\s*\d+[.)]\s+(.*)$/);
    if (unordered || ordered) {
      flushParagraph(); flushQuote();
      const isOrdered = Boolean(ordered);
      const item = (unordered?.[1] ?? ordered?.[1] ?? '').trim();
      if (!list || list.ordered !== isOrdered) { flushList(); list = { ordered: isOrdered, items: [] }; }
      list.items.push(item);
      continue;
    }
    flushList(); flushQuote();
    paragraph.push(raw.trim());
  }
  if (code) blocks.push(`<pre><code>${escapeHtml(code.body.join('\n'))}</code></pre>`);
  flushAll();
  return blocks.join('\n');
}

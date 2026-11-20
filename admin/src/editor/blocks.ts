/**
 * Gutenberg block parser/serializer for the Wordless admin editor.
 *
 * Supported blocks (lossless round-trip): paragraph, heading (h2/h3),
 * list (ordered/unordered), quote, image (by attachment id), code,
 * separator. Any other `wp:*` block — and any HTML that is not inside a
 * block comment — is kept as a `raw` block: it is preserved verbatim on
 * serialize and is editable as HTML source, never dropped.
 *
 * Inline formatting inside paragraph/heading/list/quote text is kept as
 * HTML (the editor edits it with contentEditable and stores innerHTML),
 * so `<strong>`, links etc. survive a round-trip.
 */

export type BlockType =
  | "paragraph"
  | "heading"
  | "list"
  | "quote"
  | "image"
  | "code"
  | "separator"
  | "raw";

export interface Block {
  id: string;
  type: BlockType;
  /** Inner HTML for paragraph / heading / quote. */
  html?: string;
  /** Heading level (2 or 3). */
  level?: 2 | 3;
  /** List: ordered? and item inner HTMLs. */
  ordered?: boolean;
  items?: string[];
  /** Image: attachment id (0 when unknown), url, alt, caption. */
  attachmentId?: number;
  url?: string;
  alt?: string;
  caption?: string;
  /** Code block source (unescaped). */
  code?: string;
  /** Quote citation. */
  cite?: string;
  /** Raw block: verbatim HTML, including its block comments. */
  raw?: string;
}

let idCounter = 0;
export function newBlockId(): string {
  idCounter += 1;
  return `block-${idCounter}-${Math.random().toString(36).slice(2, 8)}`;
}

function makeBlock(partial: Omit<Block, "id">): Block {
  return { id: newBlockId(), ...partial };
}

export function emptyParagraph(): Block {
  return makeBlock({ type: "paragraph", html: "" });
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function unescapeHtml(text: string): string {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#0?39;/g, "'")
    .replace(/&amp;/g, "&");
}

interface BlockComment {
  name: string;
  attrs: Record<string, unknown>;
  selfClosing: boolean;
  /** Index of the end of the comment. */
  end: number;
}

const OPEN_RE = /<!--\s*wp:([a-z0-9][a-z0-9-]*)(?:\s+(\{.*?\}))?\s*(\/)?-->/g;
const CLOSE_RE = /<!--\s*\/wp:([a-z0-9][a-z0-9-]*)\s*-->/g;

function parseAttrs(raw: string | undefined): Record<string, unknown> {
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : {};
  } catch {
    return {};
  }
}

/** Find the matching close comment for a block starting at `start`. */
function findMatchingClose(content: string, name: string, start: number): { start: number; end: number } | null {
  let depth = 1;
  let pos = start;
  while (pos < content.length) {
    OPEN_RE.lastIndex = pos;
    CLOSE_RE.lastIndex = pos;
    const open = OPEN_RE.exec(content);
    const close = CLOSE_RE.exec(content);
    const nextOpen = open ? open.index : Infinity;
    const nextClose = close ? close.index : Infinity;
    if (nextClose === Infinity && nextOpen === Infinity) return null;
    if (nextClose < nextOpen) {
      depth -= 1;
      if (depth === 0) {
        if (close && close[1] === name) {
          return { start: close.index, end: close.index + close[0].length };
        }
        return null; // Mismatched close: malformed; caller keeps it raw.
      }
      pos = nextClose + (close ? close[0].length : 1);
    } else {
      if (open && !open[3]) depth += 1;
      pos = nextOpen + (open ? open[0].length : 1);
    }
  }
  return null;
}

function parseBlockComment(content: string, at: number): BlockComment | null {
  OPEN_RE.lastIndex = at;
  const match = OPEN_RE.exec(content);
  if (!match || match.index !== at) return null;
  return {
    name: match[1] ?? "",
    attrs: parseAttrs(match[2]),
    selfClosing: match[3] === "/",
    end: at + match[0].length,
  };
}

/** Extract the inner HTML of the first <tag ...>...</tag> in `html`. */
function innerOfTag(html: string, tag: string): string | null {
  const re = new RegExp(`<${tag}(?:\\s[^>]*)?>([\\s\\S]*?)<\\/${tag}>`, "i");
  const match = re.exec(html);
  return match ? (match[1] ?? "") : null;
}

function attrValue(html: string, attr: string): string | null {
  const re = new RegExp(`${attr}\\s*=\\s*"([^"]*)"`, "i");
  const match = re.exec(html);
  return match ? (match[1] ?? null) : null;
}

function toBlock(name: string, attrs: Record<string, unknown>, inner: string, verbatim: string): Block {
  switch (name) {
    case "paragraph": {
      const html = innerOfTag(inner, "p");
      return makeBlock({ type: "paragraph", html: html ?? inner });
    }
    case "heading": {
      const level = attrs.level === 3 ? 3 : 2;
      const tag = `h${level}`;
      const html = innerOfTag(inner, tag) ?? innerOfTag(inner, "h2") ?? innerOfTag(inner, "h3") ?? inner;
      return makeBlock({ type: "heading", level, html });
    }
    case "list": {
      const ordered = attrs.ordered === true || /<ol[\s>]/i.test(inner);
      const items: string[] = [];
      const liRe = /<li(?:\s[^>]*)?>([\s\S]*?)<\/li>/gi;
      let m: RegExpExecArray | null;
      while ((m = liRe.exec(inner)) !== null) items.push(m[1] ?? "");
      return makeBlock({ type: "list", ordered, items });
    }
    case "quote": {
      const cite = innerOfTag(inner, "cite") ?? undefined;
      const withoutCite = inner.replace(/<cite(?:\s[^>]*)?>[\s\S]*?<\/cite>/i, "");
      // A quote normally wraps inner wp:paragraph blocks; keep their <p> HTML.
      const paragraphs = [...withoutCite.matchAll(/<p(?:\s[^>]*)?>([\s\S]*?)<\/p>/gi)].map(
        (p) => `<p>${p[1] ?? ""}</p>`,
      );
      const html = paragraphs.length > 0 ? paragraphs.join("") : withoutCite;
      return makeBlock({ type: "quote", html, cite });
    }
    case "image": {
      const src = attrValue(inner, "src") ?? "";
      const alt = attrValue(inner, "alt") ?? "";
      const caption = innerOfTag(inner, "figcaption") ?? undefined;
      const id = typeof attrs.id === "number" ? attrs.id : 0;
      return makeBlock({ type: "image", attachmentId: id, url: src, alt, caption });
    }
    case "code": {
      const code = innerOfTag(inner, "code") ?? inner;
      return makeBlock({ type: "code", code: unescapeHtml(code) });
    }
    case "separator":
      return makeBlock({ type: "separator" });
    default:
      return makeBlock({ type: "raw", raw: verbatim });
  }
}

/** Plain text of all blocks, for word counts and reading time. Tags are
 *  stripped; block boundaries become spaces so words never join. */
export function plainTextOfBlocks(blocks: Block[]): string {
  const parts: string[] = [];
  for (const b of blocks) {
    if (b.html !== undefined) parts.push(b.html.replace(/<[^>]*>/g, " "));
    if (b.items !== undefined) {
      for (const item of b.items) parts.push(item.replace(/<[^>]*>/g, " "));
    }
    if (b.code !== undefined) parts.push(b.code);
    if (b.caption !== undefined) parts.push(b.caption);
  }
  return parts
    .join(" ")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#0?39;/g, "'")
    .replace(/\s+/g, " ")
    .trim();
}

/** Word count of the block list (title excluded — callers add it). */
export function wordCountOfBlocks(blocks: Block[]): number {
  const text = plainTextOfBlocks(blocks);
  return text === "" ? 0 : text.split(" ").length;
}

/** Reading time in whole minutes at ~200 words per minute, minimum 1
 *  once there is any text, 0 when empty. */
export function readingTimeMinutes(blocks: Block[]): number {
  const words = wordCountOfBlocks(blocks);
  return words === 0 ? 0 : Math.max(1, Math.round(words / 200));
}
export function parseBlocks(content: string): Block[] {
  const blocks: Block[] = [];
  let pos = 0;
  const pushRaw = (html: string): void => {
    if (html.trim().length > 0) blocks.push(makeBlock({ type: "raw", raw: html }));
  };
  while (pos < content.length) {
    const comment = parseBlockComment(content, pos);
    if (!comment) {
      // Plain HTML up to the next block comment (or the end). Non-wp
      // HTML comments are kept inside the raw chunk, never dropped.
      const next = content.indexOf("<!-- wp:", pos);
      const end = next === -1 ? content.length : next;
      pushRaw(content.slice(pos, end));
      pos = end;
      continue;
    }
    if (comment.selfClosing) {
      const verbatim = content.slice(pos, comment.end);
      blocks.push(toBlock(comment.name, comment.attrs, "", verbatim));
      pos = comment.end;
      continue;
    }
    const close = findMatchingClose(content, comment.name, comment.end);
    if (close === null) {
      // Unclosed block: keep the rest verbatim so nothing is lost.
      pushRaw(content.slice(pos));
      break;
    }
    const verbatim = content.slice(pos, close.end);
    const inner = content.slice(comment.end, close.start);
    blocks.push(toBlock(comment.name, comment.attrs, inner, verbatim));
    pos = close.end;
  }
  return blocks;
}

/** Serialize a block list back to Gutenberg block-comment HTML. */
export function serializeBlocks(blocks: Block[]): string {
  return blocks.map(serializeBlock).join("\n");
}

function serializeBlock(block: Block): string {
  switch (block.type) {
    case "paragraph":
      return `<!-- wp:paragraph -->\n<p>${block.html ?? ""}</p>\n<!-- /wp:paragraph -->`;
    case "heading": {
      const level = block.level ?? 2;
      const attrs = level === 3 ? ` {"level":3}` : "";
      return `<!-- wp:heading${attrs} -->\n<h${level}>${block.html ?? ""}</h${level}>\n<!-- /wp:heading -->`;
    }
    case "list": {
      const tag = block.ordered ? "ol" : "ul";
      const attrs = block.ordered ? ` {"ordered":true}` : "";
      const items = (block.items ?? [])
        .map((item) => `<!-- wp:list-item -->\n<li>${item}</li>\n<!-- /wp:list-item -->`)
        .join("\n");
      return `<!-- wp:list${attrs} -->\n<${tag} class="wp-block-list">${items ? `\n${items}\n` : ""}</${tag}>\n<!-- /wp:list -->`;
    }
    case "quote": {
      const cite = block.cite ? `\n<cite>${escapeHtml(block.cite)}</cite>` : "";
      return `<!-- wp:quote -->\n<blockquote class="wp-block-quote"><!-- wp:paragraph -->\n${block.html ?? ""}\n<!-- /wp:paragraph -->${cite}</blockquote>\n<!-- /wp:quote -->`;
    }
    case "image": {
      const idAttr =
        block.attachmentId && block.attachmentId > 0 ? `"id":${block.attachmentId},` : "";
      const attrs = idAttr ? ` {${idAttr}"sizeSlug":"large"}` : "";
      const caption = block.caption
        ? `\n<figcaption class="wp-element-caption">${escapeHtml(block.caption)}</figcaption>`
        : "";
      return `<!-- wp:image${attrs} -->\n<figure class="wp-block-image"><img src="${escapeHtml(block.url ?? "")}" alt="${escapeHtml(block.alt ?? "")}"/>${caption}</figure>\n<!-- /wp:image -->`;
    }
    case "code":
      return `<!-- wp:code -->\n<pre class="wp-block-code"><code>${escapeHtml(block.code ?? "")}</code></pre>\n<!-- /wp:code -->`;
    case "separator":
      return `<!-- wp:separator -->\n<hr class="wp-block-separator has-alpha-channel-opacity"/>\n<!-- /wp:separator -->`;
    case "raw":
      return block.raw ?? "";
  }
}

import { describe, expect, it } from "vitest";
import { parseBlocks, serializeBlocks } from "./blocks";

const SAMPLE = `<!-- wp:paragraph -->
<p>Hello <strong>world</strong> — <a href="https://example.com">a link</a>.</p>
<!-- /wp:paragraph -->
<!-- wp:heading {"level":3} -->
<h3>Section title</h3>
<!-- /wp:heading -->
<!-- wp:list -->
<ul class="wp-block-list"><!-- wp:list-item -->
<li>First item</li>
<!-- /wp:list-item --><!-- wp:list-item -->
<li>Second <em>item</em></li>
<!-- /wp:list-item --></ul>
<!-- /wp:list -->
<!-- wp:quote -->
<blockquote class="wp-block-quote"><!-- wp:paragraph -->
<p>To be, or not to be.</p>
<!-- /wp:paragraph --><cite>Shakespeare</cite></blockquote>
<!-- /wp:quote -->
<!-- wp:image {"id":42,"sizeSlug":"large"} -->
<figure class="wp-block-image"><img src="/media/abc-photo.png" alt="A photo"/></figure>
<!-- /wp:image -->
<!-- wp:code -->
<pre class="wp-block-code"><code>const x = 1 &lt; 2;</code></pre>
<!-- /wp:code -->
<!-- wp:separator -->
<hr class="wp-block-separator has-alpha-channel-opacity"/>
<!-- /wp:separator -->`;

describe("block parse/serialize round-trip", () => {
  it("parses all supported block types", () => {
    const blocks = parseBlocks(SAMPLE);
    expect(blocks.map((b) => b.type)).toEqual([
      "paragraph",
      "heading",
      "list",
      "quote",
      "image",
      "code",
      "separator",
    ]);
    expect(blocks[0]?.html).toContain("<strong>world</strong>");
    expect(blocks[1]?.level).toBe(3);
    expect(blocks[1]?.html).toBe("Section title");
    expect(blocks[2]?.items).toEqual(["First item", "Second <em>item</em>"]);
    expect(blocks[2]?.ordered).toBe(false);
    expect(blocks[3]?.cite).toBe("Shakespeare");
    expect(blocks[3]?.html).toContain("To be, or not to be.");
    expect(blocks[4]?.attachmentId).toBe(42);
    expect(blocks[4]?.url).toBe("/media/abc-photo.png");
    expect(blocks[4]?.alt).toBe("A photo");
    expect(blocks[5]?.code).toBe("const x = 1 < 2;");
  });

  it("round-trips losslessly (parse → serialize → parse → serialize)", () => {
    const once = serializeBlocks(parseBlocks(SAMPLE));
    const twice = serializeBlocks(parseBlocks(once));
    expect(twice).toBe(once);
    // Key content survives the round-trip.
    for (const needle of [
      "<strong>world</strong>",
      "Section title",
      "Second <em>item</em>",
      "Shakespeare",
      "/media/abc-photo.png",
      "const x = 1 &lt; 2;",
    ]) {
      expect(once).toContain(needle);
    }
  });

  it("preserves unknown blocks verbatim and never drops them", () => {
    const content = `<!-- wp:paragraph -->
<p>Before</p>
<!-- /wp:paragraph -->
<!-- wp:gallery {"ids":[1,2]} -->
<figure class="wp-block-gallery"><img src="/media/a.png" alt=""/><img src="/media/b.png" alt=""/></figure>
<!-- /wp:gallery -->
<!-- wp:embed {"url":"https://example.com/video"} -->
<figure class="wp-block-embed"><div class="wp-block-embed__wrapper">https://example.com/video</div></figure>
<!-- /wp:embed -->`;
    const blocks = parseBlocks(content);
    expect(blocks.map((b) => b.type)).toEqual(["paragraph", "raw", "raw"]);
    const out = serializeBlocks(blocks);
    expect(out).toContain('<!-- wp:gallery {"ids":[1,2]} -->');
    expect(out).toContain("/media/a.png");
    expect(out).toContain("/media/b.png");
    expect(out).toContain('<!-- wp:embed {"url":"https://example.com/video"} -->');
    expect(out).toContain("https://example.com/video");
  });

  it("keeps uncommented HTML as a raw block instead of dropping it", () => {
    const content = `<div class="custom"><p>Legacy content</p></div>`;
    const blocks = parseBlocks(content);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]?.type).toBe("raw");
    expect(serializeBlocks(blocks)).toBe(content);
  });

  it("handles empty content and unclosed blocks without losing text", () => {
    expect(parseBlocks("")).toEqual([]);
    const unclosed = `<!-- wp:paragraph -->\n<p>Half written`;
    const blocks = parseBlocks(unclosed);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]?.type).toBe("raw");
    expect(serializeBlocks(blocks)).toContain("Half written");
  });

  it("serializes an ordered list with the ordered attribute", () => {
    const blocks = parseBlocks(
      `<!-- wp:list {"ordered":true} -->\n<ol class="wp-block-list"><!-- wp:list-item -->\n<li>One</li>\n<!-- /wp:list-item --></ol>\n<!-- /wp:list -->`,
    );
    expect(blocks[0]?.ordered).toBe(true);
    expect(blocks[0]?.items).toEqual(["One"]);
    expect(serializeBlocks(blocks)).toContain('{"ordered":true}');
  });
});

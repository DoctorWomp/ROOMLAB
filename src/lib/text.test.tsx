// T07 rich-text safety + T06 Unicode rendering (frontend half).
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { RichText, tokenize, mentionQuery } from "./text";

const render = (s: string) => renderToStaticMarkup(<RichText text={s} />);

describe("RichText safety (T07)", () => {
  it("escapes HTML and script tags", () => {
    const html = render('<script>alert(1)</script><img src=x onerror="alert(2)">');
    expect(html).not.toContain("<script");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;script&gt;");
  });
  it("never produces links, even for javascript:, file: or data: URLs", () => {
    for (const u of ["javascript:alert(1)", "file:///C:/Users/secret.txt", "data:text/html,<b>x</b>", "https://evil.example/x.png"]) {
      const html = render(`see ${u}`);
      expect(html).not.toMatch(/<a\b/);
      expect(html).not.toMatch(/href=/);
      expect(html).not.toMatch(/src=/);
      expect(html).toContain("inert-url");
    }
  });
  it("markdown-ish markup cannot inject attributes", () => {
    const html = render('**"><svg onload=alert(1)>** `<iframe>` *x*');
    expect(html).not.toContain("<svg");
    expect(html).not.toContain("<iframe");
    expect(html).toContain("<strong>");
    expect(html).toContain("<code>");
  });
});

describe("Unicode rendering (T06)", () => {
  it("keeps ZWJ, VS16, RTL and long strings intact", () => {
    const s = "👩‍👩‍👧‍👦 ❤️ مرحبا " + "ж".repeat(3000);
    const html = render(s);
    expect(html).toContain("👩‍👩‍👧‍👦");
    expect(html).toContain("❤️");
    expect(html).toContain("مرحبا");
    expect(html).toContain('dir="auto"');
  });
  it("multiline becomes <br>", () => {
    expect(render("a\nb")).toContain("<br/>");
  });
});

describe("tokenize / mentions", () => {
  it("finds mentions", () => {
    expect(tokenize("hi @Moth and @Sable").filter((t) => t.t === "mention").map((t) => t.v)).toEqual(["@Moth", "@Sable"]);
  });
  it("mention query at caret", () => {
    expect(mentionQuery("hello @Lu", 9)).toBe("Lu");
    expect(mentionQuery("hello Lu", 8)).toBeNull();
  });
});

import assert from "node:assert/strict";
import test from "node:test";

const context = {
  font: "",
  measureText(value: string) {
    return { width: [...value].length * 7 };
  },
};

Object.defineProperty(globalThis, "OffscreenCanvas", {
  configurable: true,
  value: class FakeOffscreenCanvas {
    getContext() {
      return context;
    }
  },
});
Object.defineProperty(globalThis, "document", {
  configurable: true,
  value: {},
});

const { layoutTranscriptText, measureTranscriptTextHeight } = await import("../src/lib/pretext-layout.ts");

test("uses Pretext layout to measure transcript text", () => {
  const metrics = {
    contentWidth: 100,
    font: "400 14px Manrope",
    linkFont: "600 14px Manrope",
    lineHeight: 20,
    letterSpacing: 0,
  };

  assert.equal(measureTranscriptTextHeight("hello", metrics), 20);
  assert.equal(measureTranscriptTextHeight("one\ntwo\nthree", metrics), 60);
  assert.equal(measureTranscriptTextHeight("https://example.com/path", metrics), 40);

  const lines = layoutTranscriptText("hello world", { ...metrics, contentWidth: 50 });
  assert.deepEqual(
    lines.map((line) => line.fragments.map((fragment) => fragment.text).join("")),
    ["hello ", "world"],
  );

  const linkedLines = layoutTranscriptText("go https://example.com/path", { ...metrics, contentWidth: 200 });
  const linkedFragment = linkedLines.flatMap((line) => line.fragments).find((fragment) => fragment.href);
  assert.equal(linkedFragment?.text, "example.com · path");
  assert.equal(linkedFragment?.href, "https://example.com/path");
});

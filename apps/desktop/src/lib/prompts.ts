import { Extension } from "@tiptap/core";
import { Plugin } from "@tiptap/pm/state";
import { Decoration as ProseMirrorDecoration, DecorationSet } from "@tiptap/pm/view";
import type { Node as ProseMirrorNode } from "@tiptap/pm/model";

import { assignPromptXmlTagColors, promptXmlMatchesInText, promptXmlTagClass, type XmlTagMatch } from "./prompt-xml.ts";

export {
  normalizePrompt,
  normalizePromptTags,
  promptPreview,
  promptTagsLabel,
} from "./prompt-model.ts";
export type { PromptRecord } from "./prompt-model.ts";

export {
  PROMPT_XML_TAG_COLOR_COUNT,
  promptXmlTagClass,
  promptXmlTagPattern,
} from "./prompt-xml.ts";

export function buildPromptXmlDecorations(doc: ProseMirrorNode) {
  const matches: XmlTagMatch[] = [];
  const decorations: ReturnType<typeof ProseMirrorDecoration.inline>[] = [];
  doc.descendants((node, position) => {
    if (!node.isText || !node.text) return;
    matches.push(...promptXmlMatchesInText(node.text, position));
  });

  assignPromptXmlTagColors(matches);
  for (const match of matches) {
    decorations.push(ProseMirrorDecoration.inline(match.from, match.to, {
      class: promptXmlTagClass(match.colorIndex!),
    }));
  }
  return DecorationSet.create(doc, decorations);
}

export const PromptXmlDecorations = Extension.create({
  name: "promptXmlDecorations",

  addProseMirrorPlugins() {
    return [
      new Plugin({
        props: {
          decorations(state) {
            return buildPromptXmlDecorations(state.doc);
          },
        },
      }),
    ];
  },
});

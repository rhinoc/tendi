export const promptXmlTagPattern = /<\/?([A-Za-z_][\w:.-]*)(?:\s+[^<>]*)?\s*\/?>/g;

export const PROMPT_XML_TAG_COLOR_COUNT = 6;

export function promptXmlTagClass(colorIndex: number): string {
  return `promptXmlTag promptXmlTagColor${colorIndex % PROMPT_XML_TAG_COLOR_COUNT}`;
}

export type XmlTagMatch = {
  from: number;
  to: number;
  name: string;
  isClosing: boolean;
  isSelfClosing: boolean;
  colorIndex: number | null;
};

export function assignPromptXmlTagColors(matches: XmlTagMatch[]) {
  const stack: XmlTagMatch[] = [];
  let nextColorIndex = 0;
  for (const match of matches) {
    if (match.isSelfClosing) {
      match.colorIndex = nextColorIndex;
      nextColorIndex = (nextColorIndex + 1) % PROMPT_XML_TAG_COLOR_COUNT;
    } else if (!match.isClosing) {
      match.colorIndex = nextColorIndex;
      nextColorIndex = (nextColorIndex + 1) % PROMPT_XML_TAG_COLOR_COUNT;
      stack.push(match);
    } else {
      let openingIndex = -1;
      for (let index = stack.length - 1; index >= 0; index -= 1) {
        if (stack[index].name === match.name) {
          openingIndex = index;
          break;
        }
      }
      if (openingIndex >= 0) {
        match.colorIndex = stack[openingIndex].colorIndex;
        stack.splice(openingIndex, 1);
      } else {
        match.colorIndex = nextColorIndex;
        nextColorIndex = (nextColorIndex + 1) % PROMPT_XML_TAG_COLOR_COUNT;
      }
    }
  }
}

export function promptXmlMatchesInText(text: string, offset = 0): XmlTagMatch[] {
  return Array.from(text.matchAll(promptXmlTagPattern), (match) => {
    const from = offset + match.index!;
    const tag = match[0];
    return {
      from,
      to: from + tag.length,
      name: match[1],
      isClosing: tag.startsWith("</"),
      isSelfClosing: tag.endsWith("/>"),
      colorIndex: null,
    };
  });
}

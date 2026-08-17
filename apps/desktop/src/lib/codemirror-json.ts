import { LanguageSupport, LRLanguage } from "@codemirror/language";
import { jsonLanguage } from "@codemirror/lang-json";
import { styleTags, tags } from "@lezer/highlight";

const codeMirrorJsonHighlighting = styleTags({
  String: tags.string,
  Number: tags.number,
  "True False": tags.bool,
  PropertyName: tags.propertyName,
  Null: tags.null,
  ", :": tags.separator,
  "[ ]": tags.squareBracket,
  "{ }": tags.brace,
});

const codeMirrorJsonLanguage = LRLanguage.define({
  name: "json",
  parser: jsonLanguage.parser.configure({
    props: [codeMirrorJsonHighlighting],
  }),
  languageData: {
    closeBrackets: { brackets: ["[", "{", '"'] },
    indentOnInput: /^\s*[\}\]]$/,
  },
});

export function codeMirrorJson() {
  return new LanguageSupport(codeMirrorJsonLanguage);
}

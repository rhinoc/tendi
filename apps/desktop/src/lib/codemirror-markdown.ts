import { markdown } from "@codemirror/lang-markdown";
import {
  LanguageDescription,
  LanguageSupport,
  StreamLanguage,
  type StreamParser,
} from "@codemirror/language";

type StreamParserLoader = () => Promise<StreamParser<unknown>>;

function streamLanguage(
  name: string,
  alias: readonly string[],
  extensions: readonly string[],
  loadParser: StreamParserLoader,
): LanguageDescription {
  return LanguageDescription.of({
    name,
    alias,
    extensions,
    load: async () => new LanguageSupport(StreamLanguage.define(await loadParser())),
  });
}

const codeMirrorMarkdownLanguages = [
  LanguageDescription.of({
    name: "JSON",
    alias: ["jsonc"],
    extensions: ["json", "jsonc"],
    load: async () => (await import("./codemirror-json.ts")).codeMirrorJson(),
  }),
  streamLanguage("JavaScript", ["js", "jsx", "node"], ["js", "jsx", "mjs", "cjs"], async () => (
    await import("@codemirror/legacy-modes/mode/javascript")
  ).javascript),
  streamLanguage("TypeScript", ["ts", "tsx"], ["ts", "tsx", "mts", "cts"], async () => (
    await import("@codemirror/legacy-modes/mode/javascript")
  ).typescript),
  streamLanguage("Shell", ["bash", "sh", "zsh", "console"], ["sh", "bash", "zsh"], async () => (
    await import("@codemirror/legacy-modes/mode/shell")
  ).shell),
  streamLanguage("Python", ["py"], ["py", "pyw"], async () => (
    await import("@codemirror/legacy-modes/mode/python")
  ).python),
  streamLanguage("Rust", ["rs"], ["rs"], async () => (
    await import("@codemirror/legacy-modes/mode/rust")
  ).rust),
  streamLanguage("Go", ["golang"], ["go"], async () => (
    await import("@codemirror/legacy-modes/mode/go")
  ).go),
  streamLanguage("YAML", ["yml"], ["yaml", "yml"], async () => (
    await import("@codemirror/legacy-modes/mode/yaml")
  ).yaml),
  streamLanguage("TOML", [], ["toml"], async () => (
    await import("@codemirror/legacy-modes/mode/toml")
  ).toml),
  streamLanguage("CSS", [], ["css"], async () => (
    await import("@codemirror/legacy-modes/mode/css")
  ).css),
  streamLanguage("HTML", ["html", "xml", "svg"], ["html", "htm", "xml", "svg"], async () => (
    await import("@codemirror/legacy-modes/mode/xml")
  ).html),
  streamLanguage("SQL", [], ["sql"], async () => (
    await import("@codemirror/legacy-modes/mode/sql")
  ).standardSQL),
  streamLanguage("Dockerfile", ["docker"], [], async () => (
    await import("@codemirror/legacy-modes/mode/dockerfile")
  ).dockerFile),
] as const;

export function codeMirrorMarkdown() {
  return markdown({ codeLanguages: codeMirrorMarkdownLanguage });
}

export function codeMirrorMarkdownLanguage(info: string): LanguageDescription | null {
  return LanguageDescription.matchLanguageName(codeMirrorMarkdownLanguages, info, true);
}

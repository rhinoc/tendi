export const TOKENIZER_LABEL = "OpenAI o200k_base";
export const TOKENIZER_PACKAGE = "gpt-tokenizer";
export const TOKENIZER_URL = "https://github.com/niieani/gpt-tokenizer";

export type MarkdownTokenStats = {
  file: number;
  selection: number;
  description?: number;
  content?: number;
  isSkillMarkdown: boolean;
};

export type TranscriptTokenStats = {
  input: number;
  output: number;
  total: number;
  inputDetails?: TokenBreakdownDetail[];
  outputDetails?: TokenBreakdownDetail[];
};

export type TranscriptTokenItem = {
  type?: string;
  body?: string;
  tag?: string;
  command?: string;
  result?: string;
  tools?: TranscriptTokenItem[];
};

export type TranscriptSkillLink = {
  skill_name: string;
};

export type TokenBreakdownDetail = {
  label: string;
  value: number;
};

export type TokenBreakdownSegment = {
  label: string;
  value: number;
  details?: TokenBreakdownDetail[];
  notes?: string[];
};

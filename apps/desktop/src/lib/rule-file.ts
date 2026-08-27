export type RuleFileResult = {
  content: string;
  sha256: string;
};

export async function readRuleFile(read: () => Promise<unknown>): Promise<RuleFileResult> {
  const result = await read();
  if (!result || typeof result !== "object") throw new Error("Rule file response was incomplete");
  const candidate = result as { content?: unknown; sha256?: unknown };
  if (typeof candidate.content !== "string" || typeof candidate.sha256 !== "string") {
    throw new Error("Rule file response was incomplete");
  }
  return { content: candidate.content, sha256: candidate.sha256 };
}

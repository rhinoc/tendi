export const DOMAIN_KEYS = ["skills", "prompts", "sessions", "rules", "hooks", "mcp"] as const;

export type DomainKey = (typeof DOMAIN_KEYS)[number];

export const RUNTIME_DOMAIN_KEYS = ["agents", ...DOMAIN_KEYS] as const;

export type RuntimeDomainKey = (typeof RUNTIME_DOMAIN_KEYS)[number];

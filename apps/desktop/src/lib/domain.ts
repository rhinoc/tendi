export enum RuntimeDomainKey {
  Agents = "agents",
  Skills = "skills",
  Prompts = "prompts",
  Sessions = "sessions",
  Rules = "rules",
  Hooks = "hooks",
  Mcp = "mcp",
}

export const DOMAIN_KEYS = [
  RuntimeDomainKey.Skills,
  RuntimeDomainKey.Prompts,
  RuntimeDomainKey.Sessions,
  RuntimeDomainKey.Rules,
  RuntimeDomainKey.Hooks,
  RuntimeDomainKey.Mcp,
] as const;

export const RUNTIME_DOMAIN_KEYS = [
  RuntimeDomainKey.Agents,
  RuntimeDomainKey.Skills,
  RuntimeDomainKey.Prompts,
  RuntimeDomainKey.Sessions,
  RuntimeDomainKey.Rules,
  RuntimeDomainKey.Hooks,
  RuntimeDomainKey.Mcp,
] as const;

export type DomainKey = Exclude<RuntimeDomainKey, RuntimeDomainKey.Agents>;

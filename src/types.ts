export type ClientKind = "codex" | "claude" | "openai";
export type ProviderProtocol = "openai" | "anthropic" | "responses";

export interface Provider {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string;
  apiKey: string;
  enabled: boolean;
  priority: number;
  client: ClientKind | "all";
  health: "healthy" | "warning" | "circuit_open";
  failures: number;
  models: string[];
  customUserAgent?: string | null;
}

export interface ModelAlias {
  id: string;
  alias: string;
  targets: AliasTarget[];
  client: ClientKind | "all";
  enabled: boolean;
  failoverEnabled: boolean;
}

export interface AliasTarget { id: string; providerId: string; upstreamModel: string; enabled: boolean }

export interface GatewayConfig {
  enabled: boolean;
  host: string;
  port: number;
  recordUsage: boolean;
  outboundProxy: string;
  clients: Record<ClientKind, boolean>;
  autoFailover: Record<ClientKind, boolean>;
  failureThreshold: number;
  recoverySeconds: number;
  maxRetries: number;
  rectifier: RectifierConfig;
}

export interface RectifierConfig {
  enabled: boolean;
  thinkingSignature: boolean;
  thinkingBudget: boolean;
  mediaFallback: boolean;
  textOnlyPreflight: boolean;
}

export interface RequestLog {
  id: number;
  timestamp: string;
  client: ClientKind;
  providerName: string;
  model: string;
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  cost: number;
  latencyMs: number;
  status: number;
  error?: string;
}

export interface UsagePoint { label: string; input: number; output: number; cached: number; cost: number; requests: number }
export interface UsageSummary { requests: number; tokens: number; cacheHitRate: number; cost: number; successRate: number; avgLatencyMs: number }
export interface GroupStat { name: string; requests: number; tokens: number; successRate: number; cost: number; latencyMs: number }
export interface DashboardData {
  config: GatewayConfig;
  providers: Provider[];
  aliases: ModelAlias[];
  summary: UsageSummary;
  trend: UsagePoint[];
  providerStats: GroupStat[];
  modelStats: GroupStat[];
  logs: RequestLog[];
  uptimeSeconds: number;
  activeConnections: number;
}

export interface UsageFilter { range: "today" | "7d" | "30d"; client: "all" | ClientKind; provider: string; model: string }

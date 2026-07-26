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
  /** 控制台余额查询配置；kind 为空表示不查询。 */
  balance?: BalanceQuery;
}

/** 参考 cc-switch 的 NewAPI 用量预设：访问令牌 + 用户 ID 查询控制台余额。 */
export interface BalanceQuery {
  /** "" 表示不查询，目前支持 "newapi"。 */
  kind: string;
  /** 控制台地址，留空则由供应商 Base URL 推导根路径。 */
  baseUrl: string;
  /** 在控制台「个人设置 → 安全设置」生成的访问令牌。 */
  token: string;
  /** 控制台用户 ID，作为 New-Api-User 请求头发送。 */
  userId: string;
}

export interface BalanceInfo { total: number; used: number; remaining: number; currency: string }

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
  /** 自动给 Anthropic 请求注入 cache_control 缓存断点。 */
  cacheInjection: boolean;
  minimizeToTray: boolean;
  launchAtLogin: boolean;
  webdav: WebdavConfig;
}

export interface WebdavConfig { url: string; username: string; password: string; remoteDir: string }
export interface StorageInfo { dataDir: string; defaultDir: string; isCustom: boolean; dbSizeBytes: number }
export interface BackupEntry { fileName: string; sizeBytes: number; createdMs: number }
export interface SyncReport { remoteFile: string; sizeBytes: number }
export interface UpdateInfo { version: string; notes: string }

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

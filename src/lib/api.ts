import { invoke } from "@tauri-apps/api/core";
import type { DashboardData, GatewayConfig, ModelAlias, Provider, UsageFilter } from "../types";

const isTauri = () => "__TAURI_INTERNALS__" in window;

const demo: DashboardData = {
  config: { enabled: true, host: "127.0.0.1", port: 18080, recordUsage: true, outboundProxy: "", clients: { codex: true, claude: true, openai: true }, autoFailover: { codex: true, claude: true, openai: false }, failureThreshold: 4, recoverySeconds: 60, maxRetries: 3, rectifier: { enabled: true, thinkingSignature: true, thinkingBudget: true, mediaFallback: true, textOnlyPreflight: true } },
  providers: [
    { id: "openai-main", name: "OpenAI Primary", protocol: "responses", baseUrl: "https://api.openai.com/v1", apiKey: "sk-••••••••••••", enabled: true, priority: 1, client: "codex", health: "healthy", failures: 0, models: ["gpt-5.2-codex", "gpt-5.2", "gpt-5.1"] },
    { id: "anthropic-main", name: "Anthropic Direct", protocol: "anthropic", baseUrl: "https://api.anthropic.com", apiKey: "sk-ant-••••••••", enabled: true, priority: 1, client: "claude", health: "healthy", failures: 0, models: ["claude-opus-4-5", "claude-sonnet-4-5", "claude-haiku-4-5"] },
    { id: "openrouter", name: "OpenRouter Backup", protocol: "openai", baseUrl: "https://openrouter.ai/api/v1", apiKey: "sk-or-••••••••", enabled: true, priority: 2, client: "all", health: "warning", failures: 1, models: ["deepseek/deepseek-chat", "openai/gpt-5.2", "anthropic/claude-sonnet-4.5"] },
  ],
  aliases: [
    { id: "a1", alias: "gpt-5-codex", targets: [{ id: "t1", providerId: "openai-main", upstreamModel: "gpt-5.2-codex", enabled: true }, { id: "t2", providerId: "openrouter", upstreamModel: "openai/gpt-5.2", enabled: true }], client: "codex", enabled: true, failoverEnabled: true },
    { id: "a2", alias: "claude-sonnet", targets: [{ id: "t3", providerId: "anthropic-main", upstreamModel: "claude-sonnet-4-5", enabled: true }, { id: "t4", providerId: "openrouter", upstreamModel: "anthropic/claude-sonnet-4.5", enabled: true }], client: "claude", enabled: true, failoverEnabled: true },
    { id: "a3", alias: "fast", targets: [{ id: "t5", providerId: "openrouter", upstreamModel: "deepseek/deepseek-chat", enabled: true }], client: "all", enabled: true, failoverEnabled: false },
  ],
  summary: { requests: 1284, tokens: 18742000, cacheHitRate: 42.8, cost: 38.64, successRate: 98.7, avgLatencyMs: 1842 },
  trend: [
    { label: "07/20", input: 980, output: 360, cached: 220, cost: 3.2, requests: 142 }, { label: "07/21", input: 1260, output: 470, cached: 410, cost: 4.9, requests: 188 },
    { label: "07/22", input: 1080, output: 420, cached: 530, cost: 4.1, requests: 164 }, { label: "07/23", input: 1720, output: 610, cached: 680, cost: 6.5, requests: 221 },
    { label: "07/24", input: 1450, output: 520, cached: 720, cost: 5.7, requests: 194 }, { label: "07/25", input: 1890, output: 740, cached: 840, cost: 7.8, requests: 246 },
    { label: "07/26", input: 1540, output: 580, cached: 660, cost: 6.4, requests: 129 },
  ],
  providerStats: [
    { name: "OpenAI Primary", requests: 724, tokens: 10820000, successRate: 99.4, cost: 24.2, latencyMs: 1620 },
    { name: "Anthropic Direct", requests: 438, tokens: 6420000, successRate: 98.6, cost: 12.8, latencyMs: 2130 },
    { name: "OpenRouter Backup", requests: 122, tokens: 1502000, successRate: 94.3, cost: 1.64, latencyMs: 2480 },
  ],
  modelStats: [
    { name: "gpt-5.2-codex", requests: 724, tokens: 10820000, successRate: 99.4, cost: 24.2, latencyMs: 1620 },
    { name: "claude-sonnet-4-5", requests: 438, tokens: 6420000, successRate: 98.6, cost: 12.8, latencyMs: 2130 },
    { name: "deepseek-chat", requests: 122, tokens: 1502000, successRate: 94.3, cost: 1.64, latencyMs: 2480 },
  ],
  logs: Array.from({ length: 8 }, (_, i) => ({ id: i + 1, timestamp: new Date(Date.now() - i * 480000).toISOString(), client: i % 3 === 0 ? "claude" : i % 3 === 1 ? "codex" : "openai", providerName: i === 5 ? "OpenRouter Backup" : i % 2 ? "OpenAI Primary" : "Anthropic Direct", model: i % 2 ? "gpt-5.2-codex" : "claude-sonnet-4-5", inputTokens: 2410 + i * 83, outputTokens: 620 + i * 21, cachedTokens: 800 + i * 40, cost: 0.038 + i * 0.004, latencyMs: 1240 + i * 210, status: i === 5 ? 429 : 200, error: i === 5 ? "Rate limit exceeded; failed over to next provider" : undefined })),
  uptimeSeconds: 43154,
  activeConnections: 3,
};

export async function getDashboard(filter: UsageFilter): Promise<DashboardData> {
  if (!isTauri()) return Promise.resolve(demo);
  return invoke("get_dashboard", { filter });
}
export async function saveConfig(config: GatewayConfig) { if (!isTauri()) { demo.config = config; return; } await invoke("save_config", { config }); }
export async function saveProvider(provider: Provider) { if (!isTauri()) { const i = demo.providers.findIndex(p => p.id === provider.id); i < 0 ? demo.providers.push(provider) : demo.providers.splice(i, 1, provider); return; } await invoke("save_provider", { provider }); }
export async function fetchProviderModels(provider: Provider): Promise<string[]> { if (!isTauri()) return Promise.resolve(provider.models.length ? provider.models : ["model-fast", "model-balanced", "model-reasoning"]); return invoke("fetch_provider_models", { provider }); }
export async function deleteProvider(id: string) { if (!isTauri()) { demo.providers = demo.providers.filter(p => p.id !== id); return; } await invoke("delete_provider", { id }); }
export async function saveAlias(alias: ModelAlias) { if (!isTauri()) { const i = demo.aliases.findIndex(a => a.id === alias.id); i < 0 ? demo.aliases.push(alias) : demo.aliases.splice(i, 1, alias); return; } await invoke("save_alias", { alias }); }
export async function deleteAlias(id: string) { if (!isTauri()) { demo.aliases = demo.aliases.filter(a => a.id !== id); return; } await invoke("delete_alias", { id }); }
export async function resetCircuit(providerId: string) { if (!isTauri()) return; await invoke("reset_circuit", { providerId }); }
export async function seedDemo() { if (!isTauri()) return; await invoke("seed_demo_data"); }

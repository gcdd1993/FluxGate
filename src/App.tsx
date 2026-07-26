import { useEffect, useMemo, useState } from "react";
import {
  Activity, ArrowDown, ArrowUp, BarChart3, Check, ChevronDown, CircleGauge,
  Clock3, Copy, Database, Gauge, KeyRound, Layers3, ListFilter, Menu, Network,
  Pencil, Plus, Power, RefreshCw, RotateCcw, Route, Search, Server, Settings,
  ShieldCheck, Trash2, TriangleAlert, X, Zap,
} from "lucide-react";
import { Area, AreaChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { deleteAlias, deleteProvider, fetchProviderModels, getDashboard, resetCircuit, saveAlias, saveConfig, saveProvider, seedDemo } from "./lib/api";
import type { AliasTarget, ClientKind, DashboardData, GatewayConfig, ModelAlias, Provider, UsageFilter } from "./types";

type Page = "overview" | "routing" | "providers" | "models" | "usage" | "logs" | "settings";
const nav: { id: Page; label: string; icon: typeof Activity }[] = [
  { id: "overview", label: "概览", icon: CircleGauge }, { id: "routing", label: "路由", icon: Route },
  { id: "providers", label: "供应商", icon: Server }, { id: "models", label: "模型别名", icon: Layers3 },
  { id: "usage", label: "使用统计", icon: BarChart3 }, { id: "logs", label: "请求日志", icon: Database },
];
const clientLabel: Record<string, string> = { all: "全部客户端", codex: "Codex", claude: "Claude", openai: "OpenAI 兼容" };
const emptyFilter: UsageFilter = { range: "7d", client: "all", provider: "", model: "" };

const uid = () => crypto.randomUUID();
const fmt = (n: number) => n >= 1_000_000 ? `${(n / 1_000_000).toFixed(2)}M` : n >= 1_000 ? `${(n / 1_000).toFixed(1)}K` : n.toLocaleString();
const money = (n: number) => `$${n.toFixed(n < 1 ? 4 : 2)}`;
const duration = (seconds: number) => `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;

export default function App() {
  const [page, setPage] = useState<Page>("overview");
  const [data, setData] = useState<DashboardData | null>(null);
  const [filter, setFilter] = useState<UsageFilter>(emptyFilter);
  const [loading, setLoading] = useState(true);
  const [sidebar, setSidebar] = useState(false);
  const [toast, setToast] = useState("");

  const refresh = async (next = filter) => {
    setLoading(true);
    try { setData(await getDashboard(next)); }
    catch (e) { setToast(`加载失败：${String(e)}`); }
    finally { setLoading(false); }
  };
  useEffect(() => { void refresh(); }, []);
  useEffect(() => { if (!toast) return; const timer = setTimeout(() => setToast(""), 3500); return () => clearTimeout(timer); }, [toast]);
  const updateFilter = (next: UsageFilter) => { setFilter(next); void refresh(next); };

  if (!data) return <div className="boot"><span className="spinner" />正在启动本地网关…</div>;
  const current = nav.find(item => item.id === page)?.label ?? "设置";
  return (
    <div className="app-shell">
      <a className="skip-link" href="#main">跳到主要内容</a>
      <aside className={`sidebar ${sidebar ? "open" : ""}`} aria-label="主导航">
        <div className="brand"><span className="brand-mark"><Zap size={19} /></span><span><b>FluxGate</b><small>LOCAL AI GATEWAY</small></span></div>
        <nav>{nav.map(item => <button key={item.id} className={page === item.id ? "active" : ""} onClick={() => { setPage(item.id); setSidebar(false); }}><item.icon size={18} /><span>{item.label}</span>{item.id === "routing" && <i className={`nav-dot ${data.config.enabled ? "live" : ""}`} />}</button>)}</nav>
        <div className="sidebar-foot">
          <button className={page === "settings" ? "active" : ""} onClick={() => setPage("settings")}><Settings size={18} />设置</button>
          <div className="gateway-mini"><span className={`pulse ${data.config.enabled ? "live" : ""}`} /><span><b>{data.config.enabled ? "网关运行中" : "网关已暂停"}</b><small>{data.config.host}:{data.config.port}</small></span></div>
        </div>
      </aside>
      <header className="topbar"><button className="icon-btn menu-btn" aria-label="打开导航" onClick={() => setSidebar(v => !v)}><Menu /></button><div><span className="eyebrow">CONTROL PLANE</span><h1>{current}</h1></div><div className="top-actions"><span className={`status-chip ${data.config.enabled ? "success" : "neutral"}`}><span className="pulse live" />{data.config.enabled ? "运行中" : "已暂停"}</span><button className="icon-btn" aria-label="刷新数据" onClick={() => void refresh()}><RefreshCw size={18} className={loading ? "spin" : ""} /></button></div></header>
      <main id="main">
        {loading && <div className="loading-line" />}
        {page === "overview" && <Overview data={data} go={setPage} />}
        {page === "routing" && <Routing data={data} onChange={async config => { await saveConfig(config); setData({ ...data, config }); setToast("路由配置已保存"); }} onReset={async id => { await resetCircuit(id); await refresh(); setToast("熔断状态已重置"); }} />}
        {page === "providers" && <Providers data={data} refresh={refresh} notify={setToast} />}
        {page === "models" && <Models data={data} refresh={refresh} notify={setToast} />}
        {page === "usage" && <Usage data={data} filter={filter} updateFilter={updateFilter} />}
        {page === "logs" && <Logs data={data} filter={filter} updateFilter={updateFilter} />}
        {page === "settings" && <SettingsPage data={data} onChange={async config => { await saveConfig(config); setData({ ...data, config }); setToast("设置已保存"); }} onSeed={async () => { await seedDemo(); await refresh(); setToast("示例统计数据已写入"); }} />}
      </main>
      {sidebar && <button className="scrim" aria-label="关闭导航" onClick={() => setSidebar(false)} />}
      {toast && <div className="toast" role="status"><Check size={17} />{toast}</div>}
    </div>
  );
}

function SectionHead({ eyebrow, title, text, action }: { eyebrow: string; title: string; text: string; action?: React.ReactNode }) {
  return <div className="section-head"><div><span className="eyebrow">{eyebrow}</span><h2>{title}</h2><p>{text}</p></div>{action}</div>;
}

function StatCard({ icon: Icon, label, value, hint, tone = "blue" }: { icon: typeof Activity; label: string; value: string; hint: string; tone?: string }) {
  return <article className={`stat-card ${tone}`}><div className="stat-icon"><Icon size={19} /></div><span>{label}</span><strong>{value}</strong><small>{hint}</small></article>;
}

function Overview({ data, go }: { data: DashboardData; go: (page: Page) => void }) {
  const activeProviders = data.providers.filter(p => p.enabled);
  return <div className="page-stack">
    <section className="hero-panel">
      <div><span className="eyebrow">LOCAL-FIRST · ZERO CLOUD CONTROL PLANE</span><h2>一个端点，连接你的所有 AI 工具。</h2><p>在本机统一 Codex、Claude 与 OpenAI 兼容请求。即时切换供应商，自动故障转移，并掌握每一个 Token。</p><div className="hero-actions"><button className="primary" onClick={() => go("routing")}><Route size={17} />管理路由</button><button className="secondary" onClick={() => go("providers")}><Plus size={17} />添加供应商</button></div></div>
      <div className="endpoint-card"><div className="endpoint-top"><span><span className="pulse live" />LOCAL ENDPOINT</span><ShieldCheck size={18} /></div><code>http://{data.config.host}:{data.config.port}/v1</code><button aria-label="复制本地端点" onClick={() => navigator.clipboard.writeText(`http://${data.config.host}:${data.config.port}/v1`)}><Copy size={16} />复制</button><small>密钥仅保存在本机 SQLite 数据库中</small></div>
    </section>
    <section className="stats-grid">
      <StatCard icon={Activity} label="今日请求" value={fmt(data.summary.requests)} hint={`${data.activeConnections} 个活跃连接`} />
      <StatCard icon={Zap} label="真实消耗 Tokens" value={fmt(data.summary.tokens)} hint={`缓存命中 ${data.summary.cacheHitRate.toFixed(1)}%`} tone="violet" />
      <StatCard icon={ShieldCheck} label="成功率" value={`${data.summary.successRate.toFixed(1)}%`} hint={`平均延迟 ${data.summary.avgLatencyMs}ms`} tone="green" />
      <StatCard icon={Clock3} label="连续运行" value={duration(data.uptimeSeconds)} hint={`预估费用 ${money(data.summary.cost)}`} tone="amber" />
    </section>
    <div className="overview-grid">
      <section className="panel chart-panel"><SectionHead eyebrow="TRAFFIC" title="请求与 Token 趋势" text="最近 7 天的网关流量" action={<button className="text-btn" onClick={() => go("usage")}>查看统计 <ArrowUp size={14} /></button>} /><UsageChart data={data.trend} /></section>
      <section className="panel"><SectionHead eyebrow="UPSTREAMS" title="供应商健康度" text={`${activeProviders.length} 个上游已加入路由`} />
        <div className="health-list">{data.providers.slice(0, 5).map(p => <div className="health-row" key={p.id}><div className="provider-avatar">{p.name.slice(0, 2).toUpperCase()}</div><div><b>{p.name}</b><small>{clientLabel[p.client]} · P{p.priority}</small></div><Health status={p.health} /></div>)}</div>
      </section>
    </div>
    <section className="panel"><SectionHead eyebrow="RECENT" title="最近请求" text="跨客户端的最新网关活动" action={<button className="text-btn" onClick={() => go("logs")}>全部日志</button>} /><LogTable logs={data.logs.slice(0, 5)} compact /></section>
  </div>;
}

function Routing({ data, onChange, onReset }: { data: DashboardData; onChange: (c: GatewayConfig) => void; onReset: (id: string) => void }) {
  const [config, setConfig] = useState(data.config);
  const update = (patch: Partial<GatewayConfig>) => { const next = { ...config, ...patch }; setConfig(next); void onChange(next); };
  const flipClient = (client: ClientKind) => update({ clients: { ...config.clients, [client]: !config.clients[client] } });
  const flipFailover = (client: ClientKind) => update({ autoFailover: { ...config.autoFailover, [client]: !config.autoFailover[client] } });
  return <div className="page-stack">
    <SectionHead eyebrow="ROUTING ENGINE" title="本地路由" text="控制流量接管、供应商优先级和故障恢复策略" />
    <section className={`gateway-banner ${config.enabled ? "online" : ""}`}><div className="gateway-orb"><Power /></div><div><span className="eyebrow">GATEWAY SERVICE</span><h3>{config.enabled ? "网关正在监听" : "网关已暂停转发"}</h3><p>http://{config.host}:{config.port} · 运行 {duration(data.uptimeSeconds)}</p></div><label className="switch"><input type="checkbox" checked={config.enabled} onChange={() => update({ enabled: !config.enabled })} /><span /></label></section>
    <section className="panel"><SectionHead eyebrow="CLIENT TAKEOVER" title="客户端接管" text="为不同 API 协议独立启用本地路由" /><div className="client-grid">{(["codex", "claude", "openai"] as ClientKind[]).map(client => <article className={config.clients[client] ? "client-card selected" : "client-card"} key={client}><div className="client-logo">{client === "codex" ? "CX" : client === "claude" ? "CL" : "AI"}</div><div><b>{clientLabel[client]}</b><small>{client === "claude" ? "/v1/messages" : client === "codex" ? "/v1/responses" : "/v1/chat/completions"}</small></div><label className="switch small"><input type="checkbox" checked={config.clients[client]} onChange={() => flipClient(client)} /><span /></label></article>)}</div></section>
    <section className="panel"><div className="split-head"><SectionHead eyebrow="FAILOVER" title="自动故障转移队列" text="按优先级尝试健康供应商；失败时自动切换" /><div className="field inline"><label>客户端</label><select aria-label="故障转移客户端"><option>全部队列</option></select></div></div>
      <div className="routing-columns">{(["codex", "claude", "openai"] as ClientKind[]).map(client => <div className="route-queue" key={client}><div className="queue-head"><div><b>{clientLabel[client]}</b><small>{data.providers.filter(p => p.client === client || p.client === "all").length} 个供应商</small></div><label className="switch small"><input type="checkbox" checked={config.autoFailover[client]} onChange={() => flipFailover(client)} /><span /></label></div>{data.providers.filter(p => p.client === client || p.client === "all").sort((a,b) => a.priority-b.priority).map((p, i) => <div className="queue-item" key={p.id}><span className="priority">P{i + 1}</span><div><b>{p.name}</b><small>{p.protocol} · {p.baseUrl.replace(/^https?:\/\//, "")}</small></div><Health status={p.health} />{p.health === "circuit_open" && <button className="icon-btn tiny" aria-label={`重置 ${p.name} 熔断`} onClick={() => onReset(p.id)}><RotateCcw size={14} /></button>}</div>)}</div>)}</div>
    </section>
    <section className="panel"><SectionHead eyebrow="CIRCUIT BREAKER" title="熔断与重试" text="避免持续请求异常上游；参数修改后即时生效" /><div className="form-grid four"><Field label="连续失败阈值"><input type="number" min="1" max="20" value={config.failureThreshold} onChange={e => update({ failureThreshold: +e.target.value })} /></Field><Field label="恢复等待（秒）"><input type="number" min="1" value={config.recoverySeconds} onChange={e => update({ recoverySeconds: +e.target.value })} /></Field><Field label="最大重试次数"><input type="number" min="0" max="10" value={config.maxRetries} onChange={e => update({ maxRetries: +e.target.value })} /></Field><Field label="请求统计"><label className="switch-field"><span>记录 Token 与延迟</span><label className="switch small"><input type="checkbox" checked={config.recordUsage} onChange={() => update({ recordUsage: !config.recordUsage })} /><span /></label></label></Field></div></section>
  </div>;
}

function Providers({ data, refresh, notify }: { data: DashboardData; refresh: () => Promise<void>; notify: (s: string) => void }) {
  const [edit, setEdit] = useState<Provider | null>(null);
  const blank = (): Provider => ({ id: uid(), name: "", protocol: "openai", baseUrl: "https://", apiKey: "", enabled: true, priority: data.providers.length + 1, client: "all", health: "healthy", failures: 0, models: [], customUserAgent: "" });
  const submit = async (p: Provider) => { await saveProvider(p); setEdit(null); await refresh(); notify("供应商已保存"); };
  return <div className="page-stack"><SectionHead eyebrow="UPSTREAMS" title="供应商" text="管理认证、协议、端点与路由优先级" action={<button className="primary" onClick={() => setEdit(blank())}><Plus size={17} />添加供应商</button>} />
    <div className="provider-grid">{data.providers.map(p => <article className="provider-card" key={p.id}><div className="provider-card-top"><div className="provider-avatar large">{p.name.slice(0,2).toUpperCase()}</div><Health status={p.health} /></div><h3>{p.name}</h3><p>{p.baseUrl}</p><div className="tag-row"><span className="tag">{p.protocol}</span><span className="tag">{clientLabel[p.client]}</span><span className="tag">P{p.priority}</span></div><dl><div><dt>认证</dt><dd><KeyRound size={14} />{p.apiKey ? `${p.apiKey.slice(0, 5)}••••••••` : "未配置"}</dd></div><div><dt>可用模型</dt><dd>{p.models.length ? `${p.models.length} 个` : "尚未获取"}</dd></div><div><dt>连续失败</dt><dd>{p.failures} 次</dd></div></dl><div className="card-actions"><button className="secondary" onClick={() => setEdit(p)}><Pencil size={15} />编辑</button><button className="danger-ghost" aria-label={`删除 ${p.name}`} onClick={async () => { if (confirm(`确定删除供应商「${p.name}」？`)) { await deleteProvider(p.id); await refresh(); notify("供应商已删除"); } }}><Trash2 size={16} /></button></div></article>)}</div>
    {!data.providers.length && <Empty title="还没有供应商" text="添加第一个上游端点后即可开始路由。" action={<button className="primary" onClick={() => setEdit(blank())}>添加供应商</button>} />}
    {edit && <ProviderModal value={edit} onClose={() => setEdit(null)} onSave={submit} />}
  </div>;
}

function ProviderModal({ value, onClose, onSave }: { value: Provider; onClose: () => void; onSave: (p: Provider) => void }) {
  const [p, setP] = useState(value); const patch = (x: Partial<Provider>) => setP({ ...p, ...x });
  const [fetching, setFetching] = useState(false); const [fetchError, setFetchError] = useState(""); const [modelQuery, setModelQuery] = useState("");
  const visibleModels = p.models.filter(model => model.toLowerCase().includes(modelQuery.toLowerCase())).slice(0, 80);
  const fetchModels = async () => { setFetching(true); setFetchError(""); try { patch({ models: await fetchProviderModels(p) }); } catch (error) { setFetchError(String(error)); } finally { setFetching(false); } };
  return <Modal title={value.name ? "编辑供应商" : "添加供应商"} subtitle="密钥仅保存在本机，不会同步到任何云端。" onClose={onClose}><form onSubmit={e => { e.preventDefault(); void onSave(p); }}><div className="form-grid two"><Field label="供应商名称" required><input required value={p.name} onChange={e => patch({ name: e.target.value })} placeholder="例如 OpenRouter" /></Field><Field label="目标客户端"><select value={p.client} onChange={e => patch({ client: e.target.value as Provider["client"] })}><option value="all">全部客户端</option><option value="codex">Codex</option><option value="claude">Claude</option><option value="openai">OpenAI 兼容</option></select></Field><Field label="上游协议"><select value={p.protocol} onChange={e => patch({ protocol: e.target.value as Provider["protocol"] })}><option value="openai">OpenAI Chat Completions</option><option value="responses">OpenAI Responses</option><option value="anthropic">Anthropic Messages</option></select></Field><Field label="优先级"><input type="number" min="1" value={p.priority} onChange={e => patch({ priority: +e.target.value })} /></Field></div><Field label="Base URL" required hint="填写服务根地址，模型发现会自动请求 /models 或 /v1/models。"><input required type="url" value={p.baseUrl} onChange={e => patch({ baseUrl: e.target.value.replace(/\/$/, "") })} placeholder="https://api.example.com/v1" /></Field><Field label="API Key" hint="用于路由和模型发现请求。"><input type="password" autoComplete="off" value={p.apiKey} onChange={e => patch({ apiKey: e.target.value })} placeholder="sk-..." /></Field><Field label="自定义 User-Agent" hint="可选。仅当供应商明确要求客户端指纹时填写；留空会保留原客户端 UA，不会伪装浏览器。"><input value={p.customUserAgent ?? ""} onChange={e => patch({ customUserAgent: e.target.value })} placeholder="例如 claude-cli/1.0.119 (external, cli)" /></Field><section className="model-discovery" aria-labelledby="model-discovery-title"><div className="model-discovery-head"><div><b id="model-discovery-title">可用模型</b><small>{p.models.length ? `已获取 ${p.models.length} 个模型` : "从供应商读取模型列表"}</small></div><button type="button" className="secondary" disabled={fetching || !p.baseUrl || p.baseUrl === "https://"} onClick={() => void fetchModels()}><RefreshCw size={15} className={fetching ? "spin" : ""} />{fetching ? "正在获取" : p.models.length ? "重新获取" : "获取模型"}</button></div>{fetchError && <div className="inline-error" role="alert"><TriangleAlert size={15} /><span>{fetchError}</span><button type="button" onClick={() => void fetchModels()}>重试</button></div>}{p.models.length > 0 && <><div className="model-search"><Search size={15} /><input aria-label="搜索已获取模型" value={modelQuery} onChange={e => setModelQuery(e.target.value)} placeholder="搜索模型 ID" /><span>{visibleModels.length}/{p.models.length}</span></div><div className="discovered-models" role="list" aria-label="供应商模型列表">{visibleModels.map(model => <button type="button" role="listitem" key={model} title="复制模型 ID" onClick={() => void navigator.clipboard.writeText(model)}><Layers3 size={13} /><code>{model}</code><Copy size={13} /></button>)}</div>{p.models.length > 80 && !modelQuery && <small className="model-limit">当前显示前 80 项，可通过搜索定位其他模型。</small>}</>}</section><label className="check-row"><input type="checkbox" checked={p.enabled} onChange={() => patch({ enabled: !p.enabled })} /><span><b>加入路由</b><small>允许网关向此供应商转发请求</small></span></label><div className="modal-actions"><button type="button" className="secondary" onClick={onClose}>取消</button><button className="primary" disabled={!p.name || !p.baseUrl}>保存供应商</button></div></form></Modal>;
}

function Models({ data, refresh, notify }: { data: DashboardData; refresh: () => Promise<void>; notify: (s: string) => void }) {
  const [edit, setEdit] = useState<ModelAlias | null>(null);
  const blankTarget = (): AliasTarget => ({ id: uid(), providerId: data.providers[0]?.id ?? "", upstreamModel: data.providers[0]?.models[0] ?? "", enabled: true });
  const blank = (): ModelAlias => ({ id: uid(), alias: "", targets: [blankTarget()], client: "all", enabled: true, failoverEnabled: true });
  return <div className="page-stack"><SectionHead eyebrow="MODEL ROUTING" title="模型别名" text="把客户端请求的稳定别名映射到任意供应商真实模型" action={<button className="primary" disabled={!data.providers.length} onClick={() => setEdit(blank())}><Plus size={17} />新建别名</button>} />
    <section className="panel table-panel"><div className="alias-legend"><span>客户端模型</span><span>路由目标队列</span></div><div className="alias-list">{data.aliases.map(a => <div className="alias-row" key={a.id}><div className="model-name"><Layers3 size={17} /><div><code>{a.alias}</code><small>{clientLabel[a.client]} · {a.targets.filter(target => target.enabled).length} 个可用目标</small></div></div><div className="alias-arrow"><span /><Route size={16} /><span /></div><div className="alias-target-stack">{a.targets.slice(0, 3).map((target, index) => { const provider = data.providers.find(item => item.id === target.providerId); return <div className={target.enabled ? "alias-target" : "alias-target disabled"} key={target.id}><span className="priority">P{index + 1}</span><div><b>{provider?.name ?? "供应商已删除"}</b><code>{target.upstreamModel}</code></div></div> })}{a.targets.length > 3 && <small className="more-targets">另有 {a.targets.length - 3} 个目标</small>}</div><span className={`status-chip ${a.enabled ? "success" : "neutral"}`}>{!a.enabled ? "已停用" : a.failoverEnabled && a.targets.length > 1 ? "故障转移" : "单目标"}</span><div className="row-actions"><button className="icon-btn" aria-label={`编辑 ${a.alias}`} onClick={() => setEdit(a)}><Pencil size={16} /></button><button className="icon-btn danger" aria-label={`删除 ${a.alias}`} onClick={async () => { await deleteAlias(a.id); await refresh(); notify("模型别名已删除"); }}><Trash2 size={16} /></button></div></div>)}</div></section>
    {!data.aliases.length && <Empty title="还没有模型别名" text="创建别名，让客户端配置不再依赖具体供应商模型名。" />}
    {edit && <AliasModal value={edit} providers={data.providers} onClose={() => setEdit(null)} onSave={async a => { await saveAlias(a); setEdit(null); await refresh(); notify("模型别名已保存"); }} />}
  </div>;
}

function AliasModal({ value, providers, onClose, onSave }: { value: ModelAlias; providers: Provider[]; onClose: () => void; onSave: (a: ModelAlias) => void }) {
  const [a, setA] = useState(value); const patch = (x: Partial<ModelAlias>) => setA({ ...a, ...x });
  const updateTarget = (index: number, change: Partial<AliasTarget>) => patch({ targets: a.targets.map((target, targetIndex) => targetIndex === index ? { ...target, ...change } : target) });
  const addTarget = () => { const provider = providers[0]; patch({ targets: [...a.targets, { id: uid(), providerId: provider?.id ?? "", upstreamModel: provider?.models[0] ?? "", enabled: true }] }); };
  const moveTarget = (index: number, direction: -1 | 1) => { const next = [...a.targets]; const destination = index + direction; if (destination < 0 || destination >= next.length) return; [next[index], next[destination]] = [next[destination], next[index]]; patch({ targets: next }); };
  const removeTarget = (index: number) => patch({ targets: a.targets.filter((_, targetIndex) => targetIndex !== index) });
  const duplicateTargets = new Set(a.targets.map(target => `${target.providerId}::${target.upstreamModel.trim()}`)).size !== a.targets.length;
  const noEnabledTarget = a.enabled && !a.targets.some(target => target.enabled);
  const invalid = !a.alias.trim() || !a.targets.length || a.targets.some(target => !target.providerId || !target.upstreamModel.trim()) || duplicateTargets || noEnabledTarget;
  return <Modal title="模型别名" subtitle="同一别名可配置多个供应商和模型；请求失败时按 P1 → Pn 顺序切换。" onClose={onClose}><form onSubmit={event => { event.preventDefault(); if (!invalid) void onSave(a); }}><div className="form-grid two"><Field label="客户端请求别名" required><input required value={a.alias} onChange={event => patch({ alias: event.target.value })} placeholder="例如 coding-fast" /></Field><Field label="适用客户端"><select value={a.client} onChange={event => patch({ client: event.target.value as ModelAlias["client"] })}><option value="all">全部客户端</option><option value="codex">Codex</option><option value="claude">Claude</option><option value="openai">OpenAI 兼容</option></select></Field></div><section className="alias-queue" aria-labelledby="alias-queue-title"><div className="alias-queue-head"><div><b id="alias-queue-title">路由目标队列</b><small>允许同一供应商配置多个不同模型，排序即故障转移优先级。</small></div><button type="button" className="secondary" onClick={addTarget}><Plus size={15} />添加目标</button></div><div className="alias-target-editor">{a.targets.map((target, index) => { const provider = providers.find(item => item.id === target.providerId); const listId = `target-models-${target.id}`; return <article className={target.enabled ? "target-editor-row" : "target-editor-row disabled"} key={target.id}><span className="priority large">P{index + 1}</span><div className="target-fields"><label><span>供应商</span><select aria-label={`P${index + 1} 供应商`} value={target.providerId} onChange={event => { const selected = providers.find(item => item.id === event.target.value); updateTarget(index, { providerId: event.target.value, upstreamModel: selected?.models[0] ?? "" }); }}>{providers.map(item => <option value={item.id} key={item.id}>{item.name}</option>)}</select></label><label><span>上游模型</span><input required aria-label={`P${index + 1} 上游模型`} list={listId} value={target.upstreamModel} onChange={event => updateTarget(index, { upstreamModel: event.target.value })} placeholder="选择或输入模型 ID" /><datalist id={listId}>{provider?.models.map(model => <option value={model} key={model} />)}</datalist></label></div><label className="switch small" title={target.enabled ? "停用目标" : "启用目标"}><input aria-label={`${target.enabled ? "停用" : "启用"} P${index + 1}`} type="checkbox" checked={target.enabled} onChange={() => updateTarget(index, { enabled: !target.enabled })} /><span /></label><div className="target-actions"><button type="button" className="icon-btn tiny" disabled={index === 0} aria-label={`上移 P${index + 1}`} onClick={() => moveTarget(index, -1)}><ArrowUp size={14} /></button><button type="button" className="icon-btn tiny" disabled={index === a.targets.length - 1} aria-label={`下移 P${index + 1}`} onClick={() => moveTarget(index, 1)}><ArrowDown size={14} /></button><button type="button" className="icon-btn tiny danger" aria-label={`删除 P${index + 1}`} onClick={() => removeTarget(index)}><Trash2 size={14} /></button></div></article> })}</div>{!a.targets.length && <div className="queue-empty"><Route size={20} /><span>至少添加一个路由目标</span></div>}{duplicateTargets && <div className="inline-error" role="alert"><TriangleAlert size={15} /><span>同一供应商和模型不能重复添加。</span></div>}{noEnabledTarget && <div className="inline-error" role="alert"><TriangleAlert size={15} /><span>启用别名时至少需要一个已启用目标。</span></div>}</section><div className="alias-options"><label className="check-row"><input type="checkbox" checked={a.enabled} onChange={() => patch({ enabled: !a.enabled })} /><span><b>启用此映射</b><small>匹配别名后使用上方目标队列</small></span></label><label className="check-row"><input type="checkbox" checked={a.failoverEnabled} onChange={() => patch({ failoverEnabled: !a.failoverEnabled })} /><span><b>自动故障转移</b><small>失败或熔断时继续尝试下一个已启用目标</small></span></label></div><div className="modal-actions"><button type="button" className="secondary" onClick={onClose}>取消</button><button className="primary" disabled={invalid}>保存别名</button></div></form></Modal>;
}

function Usage({ data, filter, updateFilter }: { data: DashboardData; filter: UsageFilter; updateFilter: (f: UsageFilter) => void }) {
  const [tab, setTab] = useState<"providers" | "models">("providers");
  return <div className="page-stack"><SectionHead eyebrow="OBSERVABILITY" title="使用统计" text="筛选会同步驱动全部指标、趋势与明细" /><Filters data={data} filter={filter} update={updateFilter} />
    <section className="usage-hero"><div><span className="eyebrow">TRUE CONSUMPTION</span><strong>{fmt(data.summary.tokens)}</strong><p>真实消耗 Tokens</p></div><div className="usage-metrics"><div><span>请求数</span><b>{fmt(data.summary.requests)}</b></div><div><span>缓存命中率</span><b>{data.summary.cacheHitRate.toFixed(1)}%</b></div><div><span>预估费用</span><b>{money(data.summary.cost)}</b></div><div><span>成功率</span><b>{data.summary.successRate.toFixed(1)}%</b></div></div></section>
    <section className="panel chart-panel"><SectionHead eyebrow="TOKEN TREND" title="Token 与费用趋势" text="输入、输出和缓存读取（千 Tokens）" /><UsageChart data={data.trend} /></section>
    <section className="panel table-panel"><div className="tab-head"><div className="segmented"><button className={tab === "providers" ? "active" : ""} onClick={() => setTab("providers")}>供应商统计</button><button className={tab === "models" ? "active" : ""} onClick={() => setTab("models")}>模型统计</button></div><button className="secondary"><ArrowDown size={15} />导出 CSV</button></div><StatsTable rows={tab === "providers" ? data.providerStats : data.modelStats} /></section>
  </div>;
}

function Logs({ data, filter, updateFilter }: { data: DashboardData; filter: UsageFilter; updateFilter: (f: UsageFilter) => void }) {
  const [query, setQuery] = useState("");
  const logs = useMemo(() => data.logs.filter(l => `${l.providerName} ${l.model} ${l.status}`.toLowerCase().includes(query.toLowerCase())), [data.logs, query]);
  return <div className="page-stack"><SectionHead eyebrow="REQUEST TRACE" title="请求日志" text="检查路由决策、Token、延迟和故障转移事件" action={<button className="secondary" onClick={() => setQuery("")}><RotateCcw size={15} />重置</button>} /><div className="filter-bar"><div className="search-box"><Search size={16} /><input aria-label="搜索日志" value={query} onChange={e => setQuery(e.target.value)} placeholder="搜索供应商、模型或状态码" /></div><select aria-label="客户端筛选" value={filter.client} onChange={e => updateFilter({ ...filter, client: e.target.value as UsageFilter["client"] })}><option value="all">全部客户端</option><option value="codex">Codex</option><option value="claude">Claude</option><option value="openai">OpenAI 兼容</option></select><select aria-label="日期范围" value={filter.range} onChange={e => updateFilter({ ...filter, range: e.target.value as UsageFilter["range"] })}><option value="today">今天</option><option value="7d">最近 7 天</option><option value="30d">最近 30 天</option></select></div><section className="panel table-panel"><LogTable logs={logs} /></section></div>;
}

function SettingsPage({ data, onChange, onSeed }: { data: DashboardData; onChange: (c: GatewayConfig) => void; onSeed: () => void }) {
  const [c, setC] = useState(data.config);
  const update = (x: Partial<GatewayConfig>) => setC({ ...c, ...x });
  const updateRectifier = (x: Partial<GatewayConfig["rectifier"]>) => update({ rectifier: { ...c.rectifier, ...x } });
  return <div className="page-stack"><SectionHead eyebrow="PREFERENCES" title="网关设置" text="配置监听地址、请求整流与本地数据" /><section className="panel settings-form"><SectionHead eyebrow="NETWORK" title="监听与代理" text="修改地址或端口后需要重启应用" /><div className="form-grid two"><Field label="监听地址"><input value={c.host} onChange={e => update({ host: e.target.value })} /></Field><Field label="端口"><input type="number" min="1024" max="65535" value={c.port} onChange={e => update({ port: +e.target.value })} /></Field></div><Field label="全局出站代理" hint="可选，支持 http:// 与 socks5://。"><input value={c.outboundProxy} onChange={e => update({ outboundProxy: e.target.value })} placeholder="http://127.0.0.1:7890" /></Field></section><section className="panel rectifier-panel"><div className="rectifier-head"><div className="rectifier-icon"><Zap size={18} /></div><div><span className="eyebrow">REQUEST RECTIFIER</span><h3>整流器</h3><p>自动修复不同客户端与上游 API 之间的请求兼容问题。</p></div><SettingSwitch label="启用整流器" checked={c.rectifier.enabled} onChange={enabled => updateRectifier({ enabled })} /></div><div className={c.rectifier.enabled ? "rectifier-options" : "rectifier-options disabled"} aria-disabled={!c.rectifier.enabled}><SettingRow title="Thinking / 签名整流" text="上游拒绝 thinking、reasoning 或签名字段时，清理不兼容内容并在同一供应商重试一次。" checked={c.rectifier.thinkingSignature} disabled={!c.rectifier.enabled} onChange={thinkingSignature => updateRectifier({ thinkingSignature })} /><SettingRow title="Thinking Budget 整流" text="遇到 Anthropic budget_tokens 最小值约束时，将 budget 调整为 32000，并在需要时把 max_tokens 调整为 64000。" checked={c.rectifier.thinkingBudget} disabled={!c.rectifier.enabled} onChange={thinkingBudget => updateRectifier({ thinkingBudget })} /><SettingRow title="不支持图片降级" text="上游拒绝图片输入时，把图片块替换为 [Unsupported Image]，避免会话直接中断。" checked={c.rectifier.mediaFallback} disabled={!c.rectifier.enabled} onChange={mediaFallback => updateRectifier({ mediaFallback })} /><SettingRow title="纯文本模型预检" text="对已确认的纯文本模型在发送前降级图片；关闭后仍保留上游报错后的图片兜底。" checked={c.rectifier.textOnlyPreflight} disabled={!c.rectifier.enabled || !c.rectifier.mediaFallback} onChange={textOnlyPreflight => updateRectifier({ textOnlyPreflight })} /></div></section><div className="settings-save"><span>整流配置保存后立即作用于新请求，无需重启网关。</span><button className="primary" onClick={() => void onChange(c)}>保存设置</button></div><section className="panel danger-zone"><div><span className="eyebrow">DEVELOPER</span><h3>写入示例统计</h3><p>向本地数据库添加一组脱敏请求，用于检查统计图表。</p></div><button className="secondary" onClick={onSeed}>生成示例数据</button></section><section className="about-card"><div className="brand-mark"><Zap /></div><div><h3>FluxGate 0.1.0</h3><p>Tauri + Rust 驱动的本地 AI 网关</p></div><span>数据目录由系统应用目录托管</span></section></div>;
}

function SettingSwitch({ label, checked, disabled = false, onChange }: { label: string; checked: boolean; disabled?: boolean; onChange: (checked: boolean) => void }) {
  return <label className="switch" title={label}><input type="checkbox" aria-label={label} checked={checked} disabled={disabled} onChange={event => onChange(event.target.checked)} /><span /></label>;
}

function SettingRow({ title, text, checked, disabled, onChange }: { title: string; text: string; checked: boolean; disabled: boolean; onChange: (checked: boolean) => void }) {
  return <div className="rectifier-row"><div><b>{title}</b><small>{text}</small></div><SettingSwitch label={title} checked={checked} disabled={disabled} onChange={onChange} /></div>;
}

function Filters({ data, filter, update }: { data: DashboardData; filter: UsageFilter; update: (f: UsageFilter) => void }) {
  return <div className="filter-bar"><ListFilter size={17} /><select aria-label="日期范围" value={filter.range} onChange={e => update({ ...filter, range: e.target.value as UsageFilter["range"] })}><option value="today">今天</option><option value="7d">最近 7 天</option><option value="30d">最近 30 天</option></select><select aria-label="客户端" value={filter.client} onChange={e => update({ ...filter, client: e.target.value as UsageFilter["client"] })}><option value="all">全部客户端</option><option value="codex">Codex</option><option value="claude">Claude</option><option value="openai">OpenAI 兼容</option></select><select aria-label="供应商" value={filter.provider} onChange={e => update({ ...filter, provider: e.target.value })}><option value="">全部供应商</option>{data.providers.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</select><div className="search-box"><Search size={15} /><input aria-label="模型筛选" value={filter.model} onChange={e => update({ ...filter, model: e.target.value })} placeholder="筛选模型" /></div></div>;
}

function UsageChart({ data }: { data: DashboardData["trend"] }) {
  return <div className="chart" role="img" aria-label="Token 使用趋势图"><ResponsiveContainer width="100%" height="100%"><AreaChart data={data} margin={{ top: 12, right: 12, bottom: 0, left: -18 }}><defs><linearGradient id="input" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#5b8cff" stopOpacity=".45" /><stop offset="1" stopColor="#5b8cff" stopOpacity="0" /></linearGradient><linearGradient id="cached" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#8b5cf6" stopOpacity=".35" /><stop offset="1" stopColor="#8b5cf6" stopOpacity="0" /></linearGradient></defs><CartesianGrid stroke="#242a36" vertical={false} /><XAxis dataKey="label" tick={{ fill: "#788397", fontSize: 11 }} axisLine={false} tickLine={false} /><YAxis tick={{ fill: "#788397", fontSize: 11 }} axisLine={false} tickLine={false} /><Tooltip contentStyle={{ background: "#171b24", border: "1px solid #2b3342", borderRadius: 12, color: "#fff" }} /><Area type="monotone" dataKey="input" name="输入" stroke="#5b8cff" fill="url(#input)" strokeWidth={2} /><Area type="monotone" dataKey="output" name="输出" stroke="#22c997" fill="transparent" strokeWidth={2} /><Area type="monotone" dataKey="cached" name="缓存" stroke="#8b5cf6" fill="url(#cached)" strokeWidth={2} /></AreaChart></ResponsiveContainer></div>;
}

function LogTable({ logs, compact = false }: { logs: DashboardData["logs"]; compact?: boolean }) {
  return <div className="table-scroll"><table><thead><tr><th>时间</th><th>客户端</th><th>供应商 / 模型</th><th>Tokens</th>{!compact && <th>费用</th>}<th>耗时</th><th>状态</th></tr></thead><tbody>{logs.map(l => <tr key={l.id}><td><time>{new Date(l.timestamp).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })}</time></td><td><span className="tag">{clientLabel[l.client]}</span></td><td><b>{l.providerName}</b><code>{l.model}</code>{l.error && <small className="error-text">{l.error}</small>}</td><td className="mono">{fmt(l.inputTokens + l.outputTokens + l.cachedTokens)}</td>{!compact && <td className="mono">{money(l.cost)}</td>}<td><span className={`latency ${l.latencyMs > 5000 ? "bad" : l.latencyMs > 2000 ? "warn" : ""}`}>{l.latencyMs}ms</span></td><td><span className={`status-code ${l.status < 400 ? "ok" : "bad"}`}>{l.status}</span></td></tr>)}</tbody></table>{!logs.length && <Empty title="没有匹配的请求" text="调整筛选条件，或先通过本地网关发起请求。" />}</div>;
}

function StatsTable({ rows }: { rows: DashboardData["providerStats"] }) {
  return <div className="table-scroll"><table><thead><tr><th>名称</th><th>请求数</th><th>Tokens</th><th>成功率</th><th>平均延迟</th><th>预估费用</th></tr></thead><tbody>{rows.map(r => <tr key={r.name}><td><b>{r.name}</b></td><td className="mono">{fmt(r.requests)}</td><td className="mono">{fmt(r.tokens)}</td><td><span className="success-text">{r.successRate.toFixed(1)}%</span></td><td className="mono">{r.latencyMs}ms</td><td className="mono">{money(r.cost)}</td></tr>)}</tbody></table></div>;
}

function Health({ status }: { status: Provider["health"] }) { const meta = status === "healthy" ? ["正常", "success"] : status === "warning" ? ["波动", "warning"] : ["熔断", "danger"]; return <span className={`health ${meta[1]}`}><span />{meta[0]}</span>; }
function Field({ label, hint, required, children }: { label: string; hint?: string; required?: boolean; children: React.ReactNode }) { return <label className="field"><span>{label}{required && <i>*</i>}</span>{children}{hint && <small>{hint}</small>}</label>; }
function Modal({ title, subtitle, onClose, children }: { title: string; subtitle: string; onClose: () => void; children: React.ReactNode }) { return <div className="modal-wrap" role="dialog" aria-modal="true" aria-labelledby="modal-title"><button className="modal-scrim" aria-label="关闭" onClick={onClose} /><div className="modal"><div className="modal-head"><div><h2 id="modal-title">{title}</h2><p>{subtitle}</p></div><button className="icon-btn" aria-label="关闭" onClick={onClose}><X /></button></div>{children}</div></div>; }
function Empty({ title, text, action }: { title: string; text: string; action?: React.ReactNode }) { return <div className="empty"><Network size={28} /><h3>{title}</h3><p>{text}</p>{action}</div>; }

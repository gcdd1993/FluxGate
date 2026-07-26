<div align="center">

# FluxGate

**本地优先的 AI 供应商聚合网关**

把多家 AI 供应商聚合到一个本地端点，为 Codex、Claude Code 和 OpenAI 兼容客户端提供自动故障转移、协议转换与用量统计。

[![Release](https://img.shields.io/github/v/release/gcdd1993/FluxGate?color=blue)](https://github.com/gcdd1993/FluxGate/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![Platform](https://img.shields.io/badge/platform-Windows-lightgrey.svg)](https://github.com/gcdd1993/FluxGate/releases)

</div>

## 它解决什么问题

用过多家 AI 中转商的人都遇到过：某家突然 502、限流、跑路。FluxGate 在本地运行一个始终在线的网关，客户端只需指向这一个地址——

- 某家供应商挂了，**同一次请求内**自动切换到下一家，调用方几乎无感知
- 不需要在工具里来回改配置、换密钥
- 所有流量、Token、成本、延迟集中可见

## 功能

### 路由与容错
- **优先级路由**：按 Codex / Claude / OpenAI 兼容三类客户端各自维护供应商队列，拖拽卡片即调整 P1、P2 优先级；「通用供应商」出现在所有队列
- **自动故障转移**：单次请求内多轮尝试全部候选（指数退避 250ms→2s），可重试错误换供应商继续，不可重试错误立即换下一家
- **熔断器**：连续失败达到阈值后熔断，恢复等待期后半开放行探测请求，成功自动恢复
- **SSE 早期故障转移**：流式请求在上游发出错误事件时，只要还没向客户端写入任何字节，同样切换下一家

### 协议与兼容
- **协议转换**：Anthropic Messages / OpenAI Chat Completions / OpenAI Responses 互转（含流式）
- **模型别名**：一个别名映射多个「供应商 × 模型」目标，按队列顺序故障转移；聚合 `/v1/models` 输出
- **请求整流器**：自动校正 max_tokens 超限、temperature 越界、thinking 预算等常见上游兼容性问题，上游报错后自动整形重试
- **Prompt 缓存注入**：对 Anthropic 上游自动注入 `cache_control` 断点（工具表 / 系统提示词 / 最新消息锚点），客户端不做任何缓存标记也能命中 Anthropic Prompt Caching 省钱

### 客户端接管
- 一键接管 Claude Code（`~/.claude/settings.json`）与 Codex（`~/.codex/config.toml`），自动改写指向本地网关；备份保存在原文件旁（`.fluxgate.bak` 后缀），关闭接管即原样还原

### 观测与统计
- 请求日志：供应商 / 模型 / Token / 成本 / 延迟 / 状态码 / 故障转移事件
- 用量统计：7 天趋势图、按供应商与按模型汇总
- 余额查询：NewAPI / one-api 系控制台协议（访问令牌 + 用户 ID），卡片上直接显示剩余额度

### 桌面体验
- 系统托盘驻留：关窗不退出、网关持续服务；开机自启（可选静默启动）；单实例
- 自定义数据目录（可指向云盘 / NAS）、自动备份轮换与一键恢复、WebDAV 多设备同步
- 应用内自动更新（GitHub Releases + minisign 签名校验）

## 本地端点

默认地址 `http://127.0.0.1:18080`（刻意避开 cc-switch 常用的 15721 端口）。

| 客户端 | 端点 |
| --- | --- |
| Codex / Responses | `/v1/responses` |
| Claude Messages | `/v1/messages` |
| OpenAI 兼容 | `/v1/chat/completions` |
| 模型列表 | `/v1/models` |
| 健康检查 | `/health` |

## 快速开始

1. 从 [Releases](https://github.com/gcdd1993/FluxGate/releases) 下载安装包并安装
2. 在「供应商」页添加你的上游端点（Base URL + API Key），拖拽排列优先级
3. 在「设置 → 客户端接管」开启 Claude Code / Codex 接管，或手动把客户端指向 `http://127.0.0.1:18080`
4. 完成。之后某家供应商故障时网关会自动切换，无需人工干预

## 数据

所有设置、供应商凭证、别名与请求日志保存在本机 SQLite（`fluxgate.db`）中，不与任何远端控制面同步。数据目录可在「设置 → 高级」中自定义，支持自动备份与 WebDAV 同步。

## 开发

前置要求：Node.js 20+、Rust 1.77+、Tauri 2 平台依赖。

```bash
npm install
npm run tauri dev    # 开发模式
npm run tauri build  # 构建安装包
```

技术栈：Tauri 2 · Rust（axum + rusqlite）· React 18 + TypeScript · SQLite

## 发版

```bash
# 1. 更新 package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml 中的版本号
# 2. 打 tag 推送，GitHub Actions 自动构建、签名并发布 Release
git tag v0.0.1 && git push origin v0.0.1
```

## License

MIT

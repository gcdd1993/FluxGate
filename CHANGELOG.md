# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.0.2] - 2026-07-27

### 协议与兼容
- Claude 客户端（含 Claude Code）现可完整使用仅提供 OpenAI 兼容端点的供应商：
  - 请求完整转换：system 提示、工具定义与 `tool_choice`、`tool_use` / `tool_result` 往返、图片（base64 / URL → `image_url`）、`temperature` / `top_p` / `stop_sequences` 等采样参数
  - 流式实时转换：OpenAI `chat.completion.chunk` SSE 重写为 Anthropic Messages 事件流（`message_start` → `content_block_*` → `message_delta` → `message_stop`），支持工具调用增量与跨分块缓冲
  - 非流式响应还原 `tool_use` 块并映射 `stop_reason`（`tool_calls`→`tool_use`、`length`→`max_tokens`）
- 经转换的流式请求自动注入 `stream_options.include_usage`，Token 用量与成本统计不受影响

### 修复
- 开机自启处于关闭状态时，启动不再误报 "autostart update failed (os error 2)"：仅在自启状态实际变化时才写注册表
- Windows release 构建不再附带控制台窗口（补上 `windows_subsystem = "windows"`）

## [0.0.1] - 2026-07-26

首个公开版本。

### 路由与容错
- 按客户端（Codex / Claude / OpenAI 兼容）分队列的供应商优先级路由，卡片拖拽排序
- 单请求内多轮自动故障转移：候选全量尝试、指数退避重试（250ms→2s）、可重试错误跨供应商续跑
- 熔断器：失败阈值熔断 + 恢复等待 + 半开探测自动恢复，前端可手动重置
- SSE 流式请求在未向客户端写出字节前遇上游错误事件时提前故障转移

### 协议与兼容
- Anthropic Messages / OpenAI Chat Completions / OpenAI Responses 请求与流式响应互转
- 模型别名：多目标（供应商 × 模型）队列映射与故障转移，聚合 `/v1/models`
- 请求整流器：max_tokens / temperature / thinking 预算自动校正，上游报错后整形重试
- Prompt 缓存注入：Anthropic 上游自动注入 `cache_control` 断点（预算 4 个，保留调用方已有标记）

### 客户端接管
- 一键接管 / 还原 Claude Code 与 Codex 本地配置（自动备份）

### 观测
- 请求日志（Token / 成本 / 延迟 / 状态 / 错误详情）与 7 天用量趋势统计
- NewAPI / one-api 系控制台余额查询（访问令牌 + 用户 ID）

### 桌面
- 系统托盘驻留、关闭最小化、开机自启（可选静默）、单实例
- 自定义数据目录、自动备份轮换（保留 10 份）与恢复、WebDAV 云同步
- 应用内自动更新（GitHub Releases + minisign 签名）

# Changelog

本项目的版本记录遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/) 格式。

## [0.2.0] - 2026-07-10

### Added

- 新增统一的 LLM 配置文件 `llm.toml`，集中管理 provider / model / base_url / thinking 等非敏感设置；API Key 始终只从环境变量读取（变量名可通过 `api_key_env` 自定义），永不写入配置文件。
- 新增 `src/llm_config.rs`：负责加载、合并、校验 `llm.toml`，优先级为 CLI 参数 > 配置文件 > 内置默认值。
- `llm_client.rs` 重写为多格式客户端，支持两种协议：
  - `anthropic`：Anthropic Messages API（`/v1/messages`），兼容官方地址及各类中转站
  - `openai`：OpenAI Chat Completions 格式（`/chat/completions`），兼容 DeepSeek 官方 API
- 新增模型 thinking（推理）模式开关，默认关闭：
  - `llm.toml` 中的 `thinking` 字段
  - CLI 参数 `--llm-thinking` / `--llm-no-thinking`（互斥，覆盖配置文件）
  - Anthropic 格式通过 `thinking.budget_tokens` 控制推理预算；OpenAI/DeepSeek 格式通过 `thinking.type` 开关
- 新增 CLI 参数：`--llm-config`（配置文件路径）、`--llm-provider`、`--llm-model`、`--llm-base-url`，用于临时覆盖 `llm.toml` 中的对应设置。
- 运行时打印 `llm-provider` 状态行，显示实际生效的 provider / model / thinking / base_url，便于确认配置是否按预期加载。
- 使用 `wiremock` 新增 5 个覆盖真实 HTTP 往返的重试集成测试：
  - 5xx / 503 瞬时错误后重试成功
  - 模型返回非法 JSON 后重试成功
  - 连续 3 次非法 JSON 后正确放弃并报错
  - `max_tokens` 截断错误不重试（避免同样输入长度的无意义重试）
  - OpenAI 格式下的重试路径
- 新增 `llm_config.rs` 的 6 个单元测试，覆盖默认值解析、文件读取、CLI 覆盖优先级、thinking 默认关闭、budget 校验、非法 provider 报错。

### Changed

- `README.md` / `README_en.md` 补充「LLM 配置」章节，更新项目结构、环境变量说明，移除仅支持 `ANTHROPIC_AUTH_TOKEN` 的过时描述。
- 修复 clippy 提示的多处风格问题：多余闭包、手写 `div_ceil`、可省略的生命周期标注、手写取模判断改用 `is_multiple_of`，以及把 `mod tests` 后方孤立的 `visit_dir` 函数移到测试模块之前。

[0.2.0]: https://github.com/xyt-dev/epub2html-english-tutor/releases/tag/v0.2.0

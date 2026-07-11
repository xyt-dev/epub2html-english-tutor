# Changelog

本项目的版本记录遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/) 格式。

## [Unreleased]

### Added

- LLM 配置改为具名 `[[llm]]` profile 列表；每本新书选择一次，实际生效的非敏感配置会持久化到 `*_state.json` 首部，断点续传不会因之后修改模板而静默换模型。
- 新增 `--switch`，用于显式替换已有书籍绑定的 profile；被删除的旧 profile 会要求用户明确切换，不会自动回退。
- profile 支持直接保存 `api_key`，并保留 `api_key_env` / provider 默认环境变量作为后备。Key 不写入 state、日志或错误信息，Unix 配置文件以 `0600` 原子写入。

### Changed

- `--setup` 每次追加一个具名 profile，不再覆盖整份配置；旧版单个 `[llm]` 配置在下次向导保存时自动迁移为列表。
- README 与配置模板补充多 profile 选择、凭据优先级、state 快照和显式切换说明。
- `--setup` 的 API Key 改为可见输入，并新增 `max_output_tokens` 提示；默认输出上限提升至 `32768`。无协议的 `base_url` 自动补全为 HTTPS。

### Fixed

- thinking 强度统一改用 provider 官方 effort 字段：Anthropic Messages 发送 `output_config.effort`，OpenAI 格式发送 `reasoning_effort`；彻底移除运行时、profile 与 state 中的 `thinking_budget_tokens`，旧文件中的该字段会在重写时清理。

## [0.2.0] - 2026-07-10

### Added

- 新增平台标准配置目录下的 LLM 配置文件（如 `~/.config/epub-reader/llm.toml`），集中管理 provider / model / base_url / thinking / thinking_effort / jobs 等非敏感设置；API Key 始终只从环境变量读取（变量名可通过 `api_key_env` 自定义），永不写入配置文件。项目目录里的 `llm.toml` 仅作为带注释的模板/示例，程序运行时从不读取它。
- 新增 `src/config/` 模块（`paths` / `schema` / `loader` / `wizard`），负责平台配置目录解析、加载、合并、校验 LLM 配置，优先级为 CLI 参数 > 配置文件 > 内置默认值；无配置文件时退回到 CLI 参数 + 内置默认值运行。
- 新增 `--setup` 交互式配置向导（基于 `dialoguer`），询问常用字段（provider / model / base_url / api_key_env / thinking / thinking_effort / jobs），写完直接生效，无需手动编辑 TOML。
- `llm_client.rs` 重写为多格式客户端，支持两种协议：
  - `anthropic`：Anthropic Messages API（`/v1/messages`），兼容官方地址及各类中转站
  - `openai`：OpenAI Chat Completions 格式（`/chat/completions`），兼容 DeepSeek 官方 API
- 新增模型 thinking（推理）模式开关，默认关闭：
  - 配置文件中的 `thinking` 字段
  - CLI 参数 `--llm-thinking` / `--llm-no-thinking`（互斥，覆盖配置文件）
  - Anthropic 格式通过 `thinking.budget_tokens` 控制推理预算；OpenAI/DeepSeek 格式通过 `thinking.type` 开关
- 新增 `thinking_effort` 配置和 `--llm-thinking-effort` CLI 覆盖，接受 `low` / `medium` / `high` / `xhigh` / `max`；单独使用 CLI 参数会自动开启 thinking。OpenAI 格式向 DeepSeek 发送 `reasoning_effort`，DeepSeek Anthropic 兼容端点发送 `output_config.effort`，官方 Anthropic 仍使用 thinking token budget。
- 新增 CLI 参数：`--llm-config`（配置文件路径覆盖）、`--llm-provider`、`--llm-model`、`--llm-base-url`，用于临时覆盖配置文件中的对应设置。
- 运行时打印 `llm-provider` 状态行，显示实际生效的 provider / model / thinking / effort / base_url，便于确认配置是否按预期加载。
- 使用 `wiremock` 新增 5 个覆盖真实 HTTP 往返的重试集成测试：
  - 5xx / 503 瞬时错误后重试成功
  - 模型返回非法 JSON 后重试成功
  - 连续 3 次非法 JSON 后正确放弃并报错
  - `max_tokens` 截断错误不重试（避免同样输入长度的无意义重试）
  - OpenAI 格式下的重试路径
- 新增 `src/config/` 模块的单元测试，覆盖默认值解析、文件读取、CLI 覆盖优先级、thinking 默认关闭、budget 校验、非法 provider 报错。

### Changed

- `README.md` / `README_en.md` 补充「LLM 配置」章节，更新项目结构、环境变量说明，移除仅支持 `ANTHROPIC_AUTH_TOKEN` 的过时描述。
- 修复 clippy 提示的多处风格问题：多余闭包、手写 `div_ceil`、可省略的生命周期标注、手写取模判断改用 `is_multiple_of`，以及把 `mod tests` 后方孤立的 `visit_dir` 函数移到测试模块之前。

[0.2.0]: https://github.com/xyt-dev/epub2html-english-tutor/releases/tag/v0.2.0

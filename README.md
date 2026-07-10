# epub-reader — EPUB / Markdown / TXT 转 HTML + AI 逐段翻译工具

[English](README_en.md)

> 把 `.epub`、`.md/.markdown`、`.txt` 转成可阅读 HTML，并调用大模型（支持 Anthropic 格式或 OpenAI 格式，兼容 DeepSeek 官方 API 和各类中转站）为每段生成译文、词汇讲解和短语分析。支持断点续传、离线重建、可控并发翻译、连续段落批请求和可配置文本分段。

![png](1.png)

## 功能特性

- 支持 `epub`、`md/markdown`、`txt` 三类输入
- 支持单文件处理，也支持递归扫描整个目录
- 输出阅读友好的 HTML，每段带 3 个折叠区块
- Markdown fenced code block 和 EPUB/HTML 里的 `<pre>` 代码块会原样保留
- 代码块不参与翻译，但会在 HTML 中以 Catppuccin Mocha 风格做语法高亮
- 调用大模型返回结构化 JSON：译文 / 词汇 / chunks，支持 Anthropic 格式（含中转站）和 OpenAI 格式（含 DeepSeek）
- 连续段落会按批次请求 Claude，并在请求和响应里显式携带段落 ID
- 支持 `Ctrl+C` 中断后继续跑，已完成段落不会重复请求
- 支持 `--rebuild` 离线重建 HTML，不调用 API
- 支持 `--count` 只统计会发送给第三方模型的有效文本，不调用 API、不写输出文件
- 支持 `--jobs` 控制并发请求数，支持 `--request-delay-ms` 节流
- 支持 `-v/--verbose` 在终端打印完整 LLM 请求和响应内容
- 每批翻译都会附带书名，并支持 `--context-paragraphs` 附带前 N 段原文上下文
- 默认批处理策略：目标约 `5000` 有效字符、硬上限 `7000`、每批最多 `10` 段，批失败会自动降级为逐段重试
- TXT / Markdown 分段规则可通过 CLI 参数调节
- 阅读 HTML 内置章节目录、当前位置提示和段落级恢复定位
- 折叠区的展开状态会持久化；阅读进度按段落位置计算，不受折叠展开影响
- HTML 和 state 都走原子写入，崩溃时更安全

## 安装

### 前置条件

```bash
# 安装 Rust（若未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# LLM 相关设置（provider/model/base_url/thinking）统一放在平台标准配置目录下的
# llm.toml 中（如 ~/.config/epub-reader/llm.toml），项目目录里的 llm.toml 只是
# 一份可读模板，程序运行时不会读取它。首次使用建议跑一遍交互式配置向导：
cargo run --release -- --setup
# 向导只问常用字段（provider/model/base_url/api_key_env/thinking/jobs），
# 写完后按提示设置对应的环境变量，例如：
export ANTHROPIC_AUTH_TOKEN="sk-ant-..."   # provider = "anthropic" 时
# export DEEPSEEK_API_KEY="sk-..."        # provider = "openai" 时（DeepSeek 原生 API）
```

### 编译

```bash
cd epub-reader
cargo build --release
```

## 快速开始

### 1. 翻译单个 EPUB

```bash
cargo run --release -- ./books/vol1.epub
```

### 2. 翻译整个目录

```bash
cargo run --release -- ./books ./output
```

### 3. 处理 Markdown

```bash
cargo run --release -- ./notes/chapter01.md
```

### 4. 处理 TXT

```bash
cargo run --release -- ./draft.txt
```

### 5. TXT 按每行强制分段

适合诗歌、逐行台词、OCR 后的短句文本：

```bash
cargo run --release -- --txt-hard-linebreaks ./draft.txt
```

### 6. 控制并发和节流

```bash
cargo run --release -- --jobs 3 --request-delay-ms 250 ./books
```

说明：

- `--jobs` 控制同时进行的批请求数，而不是单段请求数；也可以写进 llm.toml 的 `jobs`
- 并发只影响吞吐，不影响每批附带的原文上下文；上下文始终按原文位置切片
- 每个批次默认会尽量保持连续段落，并把有效字符数控制在 `7000` 以内
- `--context-paragraphs` 默认带前 `10` 段原文作为上下文；设为 `0` 可关闭

### 7. 统计发送前文本量

不调 API，不生成 HTML，只统计会进入翻译请求的正文：

```bash
cargo run --release -- --count ./books
```

统计结果包含：

- 可翻译段落数
- 去空白后的有效字符数
- 有效中文字数
- 英文单词数
- 请求批次数
- 按当前批处理和 prompt 包装估算的输入 token 数

代码块、目录页、被解析规则过滤掉的内容不会计入。

### 8. 离线重建 HTML

不调 API，只根据已有 `*_state.json` 重新生成 HTML：

```bash
cargo run --release -- --rebuild ./books ./output
```

> `--rebuild` 需要使用和之前相同的输入源与输出目录，才能找到对应的 state 文件。

## 命令行用法

```text
Usage: epub-reader [OPTIONS] <INPUT> [OUTPUT]

Arguments:
  <INPUT>   Input file or directory (.epub/.md/.markdown/.txt)
  [OUTPUT]  Output directory for HTML and state files [default: output]

Options:
      --rebuild
          Rebuild HTML from existing state files without API calls
      --count
          Count translatable source text and exit without API calls or output files
  -v, --verbose
          Print full LLM request and response bodies to stderr
      --jobs <JOBS>
          Maximum number of concurrent translation requests [default: 2]
      --request-delay-ms <REQUEST_DELAY_MS>
          Delay in milliseconds before launching each translation request [default: 0]
      --context-paragraphs <CONTEXT_PARAGRAPHS>
          Number of preceding source paragraphs to include as context for each translation request [default: 10]
      --min-paragraph-chars <MIN_PARAGRAPH_CHARS>
          Minimum characters required for a text block without sentence punctuation [default: 2]
      --title-max-words <TITLE_MAX_WORDS>
          Maximum words to treat a short line as a book title candidate [default: 12]
      --heading-max-words <HEADING_MAX_WORDS>
          Maximum words to treat an uppercase short line as a heading [default: 8]
      --txt-hard-linebreaks
          In .txt files, treat each non-empty line as its own paragraph
      --txt-no-sentence-split
          In .txt files, do not start a new paragraph after sentence-ending punctuation
  -h, --help
          Print help
  -V, --version
          Print version
```

## 支持的输入格式

### EPUB

- 读取 spine 顺序内容
- 优先提取 HTML 中的 `p`、`blockquote`、`li`
- 会保留 `pre` 代码块，并在 HTML 中渲染为只读高亮代码
- 如果正文结构比较怪，会尝试用 `div` 做回退提取
- 会过滤部分目录页、页码页、导航项

### Markdown

- 支持读取 YAML frontmatter 中的 `title`
- 如果没有 frontmatter 标题，首个合适的 `# H1` 会作为书名
- `H1-H3` 会优先视为章节标题
- 普通段落和列表项会被当作可翻译文本块
- fenced code block 会保留在输出 HTML 中，不进入翻译请求

### TXT

- 空行、场景分隔符会触发分段
- 会尝试识别诸如 `Chapter 1`、`第十二章`、`PROLOGUE` 之类标题
- 默认会在句末和缩进位置切段
- 可通过 `--txt-hard-linebreaks` 和 `--txt-no-sentence-split` 调整规则

## 常用场景

### 网络小说 / 轻小说 EPUB

```bash
cargo run --release -- --jobs 3 ./novels
```

### Obsidian / Typora 的 Markdown 笔记

```bash
cargo run --release -- ./notes/book-summary.md
```

### OCR 导出的纯文本

```bash
cargo run --release -- --txt-hard-linebreaks --min-paragraph-chars 1 ./ocr.txt
```

### 已经跑了一半，继续执行

```bash
cargo run --release -- ./books ./output
```

重新执行相同命令即可。程序会自动读取 `*_state.json`，只翻译未完成段落。

## 输出文件

默认输出目录是 `./output`。

```text
output/
├── book-slug.html
├── book-slug_state.json
├── another-book.html
└── another-book_state.json
```

- `*.html`
  最终阅读文件
- `*_state.json`
  断点续传状态文件，保存每个段落的 AI 响应

> 不要随意删除 `*_state.json`，除非你确定要从头重跑。

生成出来的 HTML 还包含这些阅读辅助能力：

- 右上角章节目录，可快速跳转长篇章节
- 底部当前位置标签，显示当前章节和段落序号
- 阅读位置按 `para_id + 段内偏移` 保存，不再只存粗糙的滚动百分比
- 词汇 / 译文 / chunks 折叠状态会记住
- 进度条按“当前段落 / 总段落”计算，折叠展开不会让百分比失真
- 代码块会保留原文，并使用 Catppuccin Mocha 主题做离线语法高亮

### 阅读器主题配置（稀有暗金紫金）

当前阅读器的聚焦态不再用亮蓝色，而是用一套偏暗的紫金主题：正文阅读区域保持低饱和深色，当前段落、目录当前项、顶部按钮和进度条用“暗紫 + 暗金”强调，代码块继续保持 Catppuccin Mocha。

如果你想复用这一套视觉语言，核心 CSS 变量可以直接参考：

```css
:root {
  --bg: #1a1b26;
  --surface: #1f2335;
  --border: #3b4168;
  --text: #c0caf5;
  --text-dim: #565f89;

  --accent: #d6b36a;
  --accent-bright: #f0d08c;
  --accent-border: rgba(214, 179, 106, 0.28);
  --focus-rare: #d9c0ff;
  --focus-gear: rgba(168, 117, 255, 0.16);
  --gear-gold: #f1e6cb;

  --purple: #a875ff;
  --rare: #8a52db;
  --rare-soft: rgba(138, 82, 219, 0.18);
  --rare-deep: rgba(72, 34, 104, 0.74);
}
```

通用搭配建议：

- 按钮 / badge：深紫渐变底 + 暗金描边
- 聚焦英文正文：`--focus-rare`，也就是现在这组偏淡紫的稀有词条色
- 聚焦 block glow：`--focus-gear`，也就是整块外圈的紫色暗金发光层
- 左侧聚焦条：`--gear-gold`，也就是 `#f1e6cb` 这组更像装备词条的金属色
- 当前段落 / 当前章节：紫色暗金 glow + 金色边界
- 焦点态 `:focus-visible`：禁用浏览器默认蓝框，改成金色细描边 + 紫色外圈
- 进度条：从深紫过渡到亮紫，再收在暗金
- 代码高亮：继续用 Catppuccin Mocha，不和阅读器外层 UI 共用同一套色板

当前实现位置在 [src/html_gen.rs](src/html_gen.rs)。

## 批处理策略

翻译阶段默认不是“一段发一次请求”，而是“连续段落小批量请求”：

- 每个请求会发送一个 `items` 数组，数组里每项都带 `id` 和 `text`
- 每个请求都会发送 `book.title`，用于书名、卷名和整体语气参考
- 每个请求默认还会带一个 `context` 数组，包含前 `10` 段原文，仅供上下文参考，不要求模型输出
- 模型必须返回同样带 `id` 的 `items` 数组
- 本地会按 `id` 校验、重排并写回 HTML / `state.json`

当前默认策略：

- 目标批大小：约 `5000` 个有效字符
- 单批硬上限：`7000` 个有效字符
- 单批最多：`10` 段
- 单段超过 `2800` 有效字符：单独发送
- 上下文窗口：前 `10` 段原文，可用 `--context-paragraphs N` 调整
- 运行时会显示待发送批次的有效字符数和包含 prompt / `book` / `context` / `items` 包装后的输入 token 估算
- 批请求失败：自动回退为逐段请求

这样做的目的是：

- 摊薄 system prompt 的重复 token 成本
- 保持相邻段落上下文，避免把不连续内容拼在一批
- 即使批返回顺序不同，也能靠 `para_id` 准确落回对应段落

## 如何继续 / 重跑

### 继续跑

直接重复上次命令：

```bash
cargo run --release -- ./books ./output
```

### 重新生成 HTML，但不重调 API

```bash
cargo run --release -- --rebuild ./books ./output
```

这个命令也适合在升级了前端阅读界面后，批量把已有书籍重新生成一遍 HTML。

### 完全重来

删除对应的：

- `output/<slug>.html`
- `output/<slug>_state.json`

然后再重新运行。

## 工作原理

核心思路不是“靠位置对齐”，而是“靠段落 ID 对齐”。

```text
输入文件
  └─→ parse_*()
        └─→ Book / Chapter / Paragraph(id, text)
                      │
                      ├─→ html_gen: 生成段落骨架
                      ├─→ pending: 需要请求 LLM 的段落
                      └─→ state.json: para_id -> LlmResponse
```

当前流程：

1. 解析输入文件，得到统一的 `Book` 结构
2. 为正文生成可翻译段落骨架，并把代码块作为只读高亮模块保留
3. 将连续段落按批次组成 `items[{id, text}]` 请求，并发发送给配置的模型
4. 收到 `items[{id, translation, vocabulary, chunks}]` 后按 `para_id` 校验并 patch HTML
5. 先原子写入 HTML，再写入 `*_state.json`
6. 浏览器端按段落锚点保存阅读位置和折叠状态

这样做的结果是：

- 即使并发请求完成顺序不同，也不会错位
- 即使批响应内部顺序不同，也会按 `id` 重排后再落盘
- 中途崩溃时，最坏情况通常只是某段需要重翻
- 如果批请求失败，会自动拆回单段重试
- `--rebuild` 可以完全跳过 API，只根据 state 恢复 HTML
- 长篇阅读时可以靠章节目录和段落级定位快速回到上次位置
- 代码示例、终端输出片段这类内容不会丢失，也不会被误送去翻译

## 项目结构

```text
src/
├── main.rs            # CLI 参数、主流程、并发翻译调度
├── parser.rs          # 输入格式分发
├── parse_utils.rs     # 通用分段规则、标题识别、BookBuilder
├── epub_parser.rs     # EPUB 解析
├── markdown_parser.rs # Markdown 解析
├── text_parser.rs     # TXT 解析
├── html_gen.rs        # HTML 生成与段落 patch
├── config/            # LLM 配置：路径解析、schema、加载合并、交互式向导
├── llm_client.rs      # Anthropic / OpenAI 兼容客户端
├── state.rs           # state.json 读写
├── fs_utils.rs        # 原子写文件
├── ui.rs              # 终端输出样式
└── types.rs           # Book / Paragraph / LlmResponse 等结构
```

## LLM 配置

配置文件不在项目目录里，而是放在平台标准配置目录下（可用 `--llm-config` 指定别的路径）：

- Linux: `$XDG_CONFIG_HOME/epub-reader/llm.toml`（未设置时为 `~/.config/epub-reader/llm.toml`）
- macOS: `~/Library/Application Support/epub-reader/llm.toml`
- Windows: `%APPDATA%\epub-reader\llm.toml`

项目目录里的 `llm.toml` 只是一份带注释的模板/示例，程序运行时从不读取它。

最简单的配置方式是跑一遍交互式向导，它只问常用字段，写完直接生效：

```bash
cargo run --release -- --setup
```

配置文件也可以手写，格式如下：

```toml
[llm]
provider = "anthropic"        # "anthropic" 或 "openai"
model = "deepseek-v4-flash"
base_url = "https://api.anthropic.com"
thinking = false              # 默认关闭；DeepSeek/Claude 均支持开启
# thinking_budget_tokens = 4096
# api_key_env = "ANTHROPIC_AUTH_TOKEN"
# max_output_tokens = 8192
# request_timeout_secs = 180
jobs = 2
```

- `provider = "anthropic"`：走 Anthropic Messages API（`/v1/messages`），兼容官方地址和各类中转站
- `provider = "openai"`：走 OpenAI Chat Completions 格式（`/chat/completions`），兼容 DeepSeek 官方 API
- API Key 永远只从环境变量读取，变量名由 `api_key_env` 指定（默认 `anthropic` 对应 `ANTHROPIC_AUTH_TOKEN`，`openai` 对应 `DEEPSEEK_API_KEY`），不会写进配置文件
- 优先级：CLI 参数 > 配置文件 > 内置默认值。CLI 覆盖项：`--llm-provider` `--llm-model` `--llm-base-url` `--llm-thinking` / `--llm-no-thinking` `--jobs`
- `thinking` 默认关闭；`--llm-thinking` 和 `--llm-no-thinking` 不能同时使用
- 没有配置文件时，程序会退回到 CLI 参数 + 内置默认值运行；跑 `--setup` 即可交互式创建一份

## 注意事项

- API Key 只在正常翻译模式下需要（走配置文件里 `api_key_env` 指定的环境变量）；`--rebuild` 不需要
- 如果你修改了原始输入文件，段落 ID 可能变化，旧 state 可能无法完全复用
- `--jobs` 可以设得很高，也不会改变翻译上下文；但是否值得调高取决于你的 provider 并发 / RPM / TPM 限额，以及本次任务总批次数
- 对排版特别碎的 TXT，建议试试：
  - `--txt-hard-linebreaks`
  - `--min-paragraph-chars 1`
  - `--txt-no-sentence-split`

## 开发与验证

```bash
cargo fmt
cargo check
cargo test
```

如果想看当前 CLI 帮助：

```bash
cargo run -- --help
```

# epub-reader — 轻小说 epub 转 HTML + AI 逐段翻译工具

> Overlord 系列轻小说的英文 epub 批量转换器，输出带折叠面板的精美 HTML，并调用 Claude API 对每段原文进行翻译、词汇讲解和短语分析，**支持中断续传**。

---

## 目录

- [快速开始](#快速开始)
- [功能一览](#功能一览)
- [项目结构](#项目结构)
- [协议详解](#协议详解)
  - [段落 ID 规范](#1-段落-id-规范)
  - [HTML 占位符格式](#2-html-占位符格式)
  - [LLM 请求/响应协议](#3-llm-请求响应协议)
  - [断点续传机制](#4-断点续传机制)
  - [原子写入机制](#5-原子写入机制)
- [HTML 样式说明](#html-样式说明)
- [输出文件说明](#输出文件说明)
- [词汇与短语标准](#词汇与短语标准)

---

## 快速开始

### 前置条件

```bash
# 安装 Rust（若未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 设置 Anthropic API Key
export ANTHROPIC_AUTH_TOKEN="sk-xxxxxxxx..."
```

### 编译 & 运行

```bash
cd epub-reader

# 编译 release 版本（更快）
cargo build --release

# 运行（默认读取 ./LightNovels 目录下所有 .epub）
cargo run --release -- ../LightNovels

# 或指定路径
cargo run --release -- /path/to/epub/folder
```

### 输出位置

```
epub-reader/output/
├── overlord-light-novels-01-the-undead-king.html       ← 阅读文件
├── overlord-light-novels-01-the-undead-king_state.json ← 进度存档（勿删）
├── overlord-light-novels-02-the-dark-warrior.html
├── overlord-light-novels-02-the-dark-warrior_state.json
└── ...
```

### 中断后续传

程序随时可以 `Ctrl+C` 中断。重新运行同一命令，它会自动读取 `_state.json`，**跳过已翻译的段落**，从上次停止的位置继续。

---

## 功能一览

| 功能 | 说明 |
|---|---|
| epub 批量解析 | 递归扫描目录下所有 `.epub`，按 spine 顺序提取章节和段落 |
| HTML 骨架生成 | 每段原文配三个折叠面板，初始内容为占位注释 |
| Claude API 翻译 | 逐段调用 `claude-sonnet-4-6`，返回结构化 JSON |
| 实时填空 | 每翻译完一段，立即将 JSON 内容渲染进 HTML 并写盘 |
| 断点续传 | 状态持久化到 JSON 文件，中断后自动续传 |
| 错误重试 | 每段最多重试 3 次，单段失败不影响整体进度 |
| 精美样式 | Tokyo Night 配色、折叠面板、顶部进度条、响应式布局 |

---

## 项目结构

```
epub-reader/
├── Cargo.toml
└── src/
    ├── main.rs          # 主流程：扫描 epub → 解析 → 生成 HTML → LLM 翻译
    ├── types.rs         # 核心数据结构（Book / Paragraph / LlmResponse 等）
    ├── epub_parser.rs   # epub 解析：spine 遍历 + HTML 段落提取
    ├── html_gen.rs      # HTML 生成（骨架）、段落块渲染、原地 patch
    ├── llm_client.rs    # Anthropic Messages API 封装
    └── state.rs         # 断点续传状态读写（JSON 文件）
```

---

## 协议详解

### 1. 段落 ID 规范

每个段落在整个书中拥有**全局唯一 ID**，格式：

```
{book-slug}-ch{chapter:03}-p{para:04}
```

示例：

```
overlord-light-novels-01-the-undead-king-ch002-p0017
│                                          │    │
│                                          │    └─ 当前章节第 17 段（4位补零）
│                                          └────── 第 2 章（3位补零）
└───────────────────────────────────────────────── 书名 slug（URL 安全）
```

这个 ID 同时用于：
- HTML `<div>` 的 `id` 属性（锚点跳转）
- `_state.json` 中的 key（续传查找）

---

### 2. HTML 占位符格式

每个段落在 HTML 中渲染为如下结构：

```html
<!-- 翻译前（data-status="pending"，左边框为灰色） -->
<div class="para-block" id="overlord-...-ch002-p0017" data-status="pending">
  <p class="original-text">原文英文段落...</p>

  <details class="ai-section translation-section">
    <summary>🈳 译文</summary>
    <div class="ai-content"><!-- FILL:translation --></div>
  </details>

  <details class="ai-section vocab-section">
    <summary>📚 词汇 (IELTS 6.5+)</summary>
    <div class="ai-content"><!-- FILL:vocab --></div>
  </details>

  <details class="ai-section chunk-section">
    <summary>🔗 常用短语 / Chunks</summary>
    <div class="ai-content"><!-- FILL:chunks --></div>
  </details>
</div>

<!-- 翻译后（data-status="done"，左边框变为绿色） -->
<div class="para-block" id="overlord-...-ch002-p0017" data-status="done">
  <p class="original-text">原文英文段落...</p>

  <details class="ai-section translation-section">
    <summary>🈳 译文</summary>
    <div class="ai-content"><p>中文翻译内容...</p></div>
  </details>

  <details class="ai-section vocab-section">
    <summary>📚 词汇 (IELTS 6.5+)</summary>
    <div class="ai-content">
      <table class="vocab-table">
        <thead><tr><th>单词</th><th>音标</th><th>词性</th><th>释义</th><th>例句</th></tr></thead>
        <tbody>
          <tr>
            <td class="word">ephemeral</td>
            <td class="ipa">/ɪˈfem.ər.əl/</td>
            <td class="pos">adj.</td>
            <td>短暂的，转瞬即逝的</td>
            <td class="example"><em>Fame is ephemeral, but art endures.</em></td>
          </tr>
          <!-- 更多词汇... -->
        </tbody>
      </table>
    </div>
  </details>

  <details class="ai-section chunk-section">
    <summary>🔗 常用短语 / Chunks</summary>
    <div class="ai-content">
      <ul class="chunk-list">
        <li>
          <span class="chunk-phrase">fade into obscurity</span>
          <span class="chunk-cn">（逐渐被遗忘，淡出视野）</span><br>
          <em class="chunk-example">Many great artists fade into obscurity after death.</em>
        </li>
      </ul>
    </div>
  </details>
</div>
```

**填空定位方式**：程序在 HTML 字符串中搜索 `id="{para_id}"` 定位到段落块，然后用计数括号匹配算法找到对应的 `</div>` 闭合标签，整块替换。

---

### 3. LLM 请求/响应协议

#### System Prompt 摘要

Claude 被要求以**纯 JSON**（无 markdown 代码块）回复，结构如下：

```json
{
  "translation": "完整中文翻译，自然流畅，保留原著风格",
  "vocabulary": [
    {
      "word":    "单词或词组",
      "ipa":     "IPA 音标",
      "pos":     "词性（n./v./adj./adv./phrase）",
      "cn":      "中文释义",
      "example": "英文例句"
    }
  ],
  "chunks": [
    {
      "chunk":   "地道短语/搭配/句型",
      "cn":      "中文释义及用法说明",
      "example": "英文例句"
    }
  ]
}
```

#### 选词标准

- `vocabulary`：仅选 **IELTS 难度 ≥ 6.5（C1/C2）** 的词汇，每段 3~8 个，跳过基础词汇
- `chunks`：选 2~5 个母语者常用的搭配、固定表达或有学习价值的句型

#### 错误容错

- 如果模型意外包裹了 ` ```json ` 代码块，程序自动剥离
- 单段 JSON 解析失败时：跳过该段，打印警告，继续下一段
- 网络/API 错误：最多重试 3 次，指数退避（2s、4s、6s）

---

### 4. 断点续传机制

```
第一次运行：
  ┌─────────────┐    解析 epub     ┌─────────────┐
  │  .epub 文件  │ ─────────────── │  Book 结构   │
  └─────────────┘                  └──────┬──────┘
                                          │ 生成骨架 HTML
                                   ┌──────▼──────┐
                                   │  .html 文件  │  ← 全部 data-status="pending"
                                   └─────────────┘
                                   ┌─────────────┐
                                   │ _state.json  │  ← {}（空）
                                   └──────┬──────┘
                                          │ 逐段调用 Claude API
                                          │ 每段完成后：
                                          │  1. patch HTML（替换占位符）
                                          │  2. 写 _state.json（记录完成）
                                         ...
                    ← Ctrl+C 中断 →
第二次运行（续传/重复运行）：
  1. 从磁盘读取已有 .html 文件（保留所有已填充内容）
  2. 读取 _state.json，计算 pending = 全部段落 − 已完成段落
  3. 仅对 pending 段落调用 Claude API 翻译并 patch HTML
  4. 若全部段落已完成：打印 "All paragraphs already translated." 并跳过（不调用 API）
```

**崩溃安全写入顺序**：`内存patch → 写HTML.tmp → rename → 写state`
即使在任意步骤崩溃，都不会出现"state说完成但HTML是占位符"的永久空洞。

状态文件示例（`_state.json`）：

```json
{
  "completed": {
    "overlord-...-ch000-p0000": {
      "translation": "...",
      "vocabulary": [...],
      "chunks": [...]
    },
    "overlord-...-ch000-p0001": { ... },
    ...
  }
}
```

---

### 5. 原子写入机制

每次更新 HTML 文件时，严格按以下顺序操作：

```
内存 patch HTML → 写 .html.tmp → rename(.html.tmp → .html) → 写 _state.json
```

**顺序至关重要**：先写 HTML，再写 state。

| 崩溃时机 | 后果 | 下次运行 |
|---|---|---|
| HTML 写完前崩溃 | state 未记录该段 | 重新翻译该段（多耗一次 API，无数据丢失）|
| HTML 写完、state 未写完 | HTML 已填好 | 重新翻译并覆盖，结果一样（多耗一次 API）|
| state 写完后崩溃 | 完全记录 | 直接跳过 ✓ |

反过来（先写 state 再写 HTML）如果在两者之间崩溃，state 记录"已完成"但 HTML 仍是 `<!-- FILL:xxx -->`，**该段永远不会被填，造成永久空洞**。

rename() 系统调用本身是原子操作（POSIX 保证），所以 `.html` 文件要么是旧版本要么是新版本，不会出现半写状态。

---

## HTML 样式说明

采用 **Tokyo Night** 配色方案（深色主题），无需外部 CSS 文件，样式全部内联。

| 元素 | 视觉效果 |
|---|---|
| 未翻译段落 | 左侧灰色竖线 |
| 已翻译段落 | 左侧绿色竖线 |
| 译文面板 | 深蓝底色，青色文字 |
| 词汇面板 | 深紫底色，紫色标题 |
| 短语面板 | 深绿底色，绿色标题 |
| 单词 | 黄色高亮 |
| 音标 | 等宽字体，灰色 |
| 词性 | 橙色斜体 |
| 顶部进度条 | 随滚动位置实时更新 |

---

## 输出文件说明

| 文件 | 用途 | 可否删除 |
|---|---|---|
| `{slug}.html` | 最终阅读文件，用浏览器打开 | 可删除后重新生成 |
| `{slug}_state.json` | 翻译进度存档 | **不要删除**，删除后下次运行将从头翻译 |

---

## 词汇与短语标准

### IELTS 难度参考（词汇选取标准）

| 级别 | CEFR | 典型词汇示例 |
|---|---|---|
| 选取范围 ✓ | C1/C2 | ephemeral, nefarious, implacable, brandish |
| 不选 ✗ | A1–B2 | good, important, suddenly, however |

### Chunk 选取原则

优先选取：
1. **动词短语搭配**：`hold one's ground`、`lay siege to`
2. **固定表达**：`as if by instinct`、`at the mercy of`
3. **文学句型**：倒装、分词结构等值得模仿的写法
4. **IELTS/TOEFL 写作中可用的高分表达**

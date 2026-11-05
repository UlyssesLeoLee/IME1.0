# 给主流 AI 开放输入法接口（AI_AGENT_API）

> 本文件回答一个问题：**主流 AI 客户端（Claude Code、Codex、Cursor、MCode、Gemini CLI、Cline……）怎么调用 IME1.0？**
>
> 状态（2026-11-05，第十七轮）：**接口已经开放**。有三条通道：**MCP（stdio）**、**CLI**、**本地命名管道 / HTTP**。
>
> **已经验证到哪一步**：**通道本身**（AI 客户端 → `WM_COPYDATA` → 焦点进程里的 `pime_tsf.dll` → 回包）在真实的
> Windows 上跑通了——两侧各有一条**真机往返测试**，DLL 侧那条还是**跨进程**的（子进程发请求、宿主进程应答）。
> **还没有验证的是最后一米**：文字真正落进**记事本 / VS Code 的文档**——新 DLL 还没换上（要注销或重启），
> 而且这一版**没有**在真实宿主里跑过一次「AI 写中文 / 日文 / 代码」。

---

## 0. 一句话

输入法是一个**装在每一个应用里的 TSF 文本服务**：只有它知道当前文档、光标和选区。一个跑在别处的 AI
（一个 MCP 客户端、一个 CLI、一个脚本）没法自己往那个文档里写字——它得**请焦点窗口里的输入法替它写**。

本轮开放的接口就是这么一条请托：

```
AI 客户端 ──MCP/CLI/HTTP──▶ pime-mcp / pime-ime ──WM_COPYDATA──▶ 焦点窗口里的 pime_tsf.dll ──▶ 文档
```

**为什么这样省 token**：AI 不用再逐字符模拟按键（一次调用写一整段，中文/日文一次到位，不需要组字、不需要选词、
不要按坐标点），也不用把长文本拆成几十次工具调用。

---

## 1. 三条通道

| 通道 | 程序 | 适合谁 | 说明 |
|---|---|---|---|
| **MCP（stdio）** | `pime-mcp.exe` | Claude Code / Claude Desktop / Codex / Cursor / MCode / Gemini CLI / Cline / VS Code Copilot …… | **决定性的一条**：工具描述会进模型上下文，模型自己知道什么时候该用它 |
| **CLI** | `pime-ime.exe` | 不吃 MCP 的客户端、脚本、PowerShell | 一次一个操作，`--json` 给机器读 |
| **本地服务** | `pime-ime.exe serve` | 任何语言、任何进程 | 命名管道 `\\.\pipe\pime-agent-v1`（JSON Lines）+ HTTP `127.0.0.1:<端口>`（带令牌） |

三条通道是**同一套操作**，同一个实现（`crates/pime-agent`），行为完全一致。

---

## 2. MCP：七个工具

| 工具 | 作用 | 参数 |
|---|---|---|
| `ime_insert_text` | **把文字写进当前焦点窗口的光标处**（一次调用写一整段） | `text`（必填）、`language`（可选）、`replace`（可选：替换当前选区而不是插入） |
| `ime_read_selection` | 读当前焦点窗口里选中的文字 | 无 |
| `ime_set_language` | 切换输入法语言档（`zh-CN` / `en-US` / `ja-JP`） | `language`（必填） |
| `ime_candidates` | 查输入自带词库的候选（拼音→中文、罗马音→假名） | `text`（必填）、`language`（可选） |
| `ime_predict` | 查代码候选（核心引擎的补全） | `text`（必填）、`language`（可选） |
| `ime_validate` | 校验一段代码（SQL / Cypher / …） | `text`（必填）、`language`（必填）、`dialect`（可选） |
| `ime_curate_dictionary` | 把 AI 整理的词条写进用户词库 | `proposals`（必填，数组） |

每个工具的描述里都写明了**什么时候该用它**——描述是模型读的东西，所以写得短而准。

MCP 服务端还会给模型一段 **instructions**（≤ 4096 字符），说明这个输入法能干什么、什么时候用。

---

## 3. 各家客户端怎么接（现成配置，复制即可）

### 3.1 MCode（本地运行时）

编辑 `C:\Users\<你>\.minimax\mcp.json`，加一段：

```json
{
  "mcpServers": {
    "ime1": {
      "type": "stdio",
      "enabled": true,
      "configured": true,
      "builtin": false,
      "description": "IME1.0 编程输入法：把文字写进焦点窗口的光标处，查词库候选，校验代码",
      "command": "C:\\Program Files\\IME1.0\\pime-mcp.exe",
      "env": { "PIME_CORE_EXE": "C:\\Program Files\\IME1.0\\pime-core.exe" }
    }
  }
}
```

### 3.2 Claude Code

```bash
claude mcp add ime1 -- "C:\Program Files\IME1.0\pime-mcp.exe"
```
或写进 `~/.claude.json` / `.mcp.json`：
```json
{ "mcpServers": { "ime1": { "command": "C:\\Program Files\\IME1.0\\pime-mcp.exe" } } }
```

### 3.3 Claude Desktop（`claude_desktop_config.json`）

```json
{ "mcpServers": { "ime1": { "command": "C:\\Program Files\\IME1.0\\pime-mcp.exe" } } }
```

### 3.4 Codex CLI（`~/.codex/config.toml`）

```toml
[mcp_servers.ime1]
command = "C:\\Program Files\\IME1.0\\pime-mcp.exe"
```

### 3.5 Cursor（`.cursor/mcp.json`）

```json
{ "mcpServers": { "ime1": { "command": "C:\\Program Files\\IME1.0\\pime-mcp.exe" } } }
```

### 3.6 Gemini CLI（`~/.gemini/settings.json`）

```json
{ "mcpServers": { "ime1": { "command": "C:\\Program Files\\IME1.0\\pime-mcp.exe" } } }
```

### 3.7 VS Code / Cline / Roo Code / Continue

`.vscode/mcp.json`（VS Code 自带的 MCP 支持）：
```json
{ "servers": { "ime1": { "type": "stdio", "command": "C:\\Program Files\\IME1.0\\pime-mcp.exe" } } }
```
Cline / Roo Code 用 `cline_mcp_settings.json` / `mcp_settings.json`，键名是 `mcpServers`。

### 3.8 不吃 MCP 的客户端：CLI

```powershell
& "C:\Program Files\IME1.0\pime-ime.exe" insert "你好，世界" --language zh-CN
& "C:\Program Files\IME1.0\pime-ime.exe" status --json
```

### 3.9 任何语言都能连：本地服务

```powershell
& "C:\Program Files\IME1.0\pime-ime.exe" serve --pipe --http 127.0.0.1:8731 --token <令牌>
```
- 命名管道 `\\.\pipe\pime-agent-v1`：一行一个 JSON 请求，一行一个 JSON 回答（JSON Lines）。
- HTTP：`POST /`，请求体是 JSON，**必须带令牌**（`Authorization: Bearer <令牌>` 或 `?token=`）；`GET /health` 是健康检查。
- **只绑 127.0.0.1**，不对外网开放。

---

## 4. CLI 用法

```
pime-ime status                      # 输入法在不在、当前语言、核心在不在
pime-ime insert "你好，世界"          # 写进焦点窗口的光标处
pime-ime insert "替换我" --replace    # 替换当前选区
pime-ime read                        # 读当前选区
pime-ime language ja-JP              # 切语言
pime-ime candidates ni --language zh-CN
pime-ime predict "fn " --language rust
pime-ime validate --language sql --text "SELECT 1"
pime-ime resolve src/main.rs
pime-ime scan --text "token=abc"     # 密钥扫描（发给服务商前先扫一遍）
pime-ime curate --file proposals.json
pime-ime misses                      # 用户打了但没找到的词
pime-ime serve --pipe --http 127.0.0.1:8731 --token <令牌>
pime-ime --help
```
加 `--json` 输出机器可读的 JSON。**成功退出码 0，失败 1**（错误码打在 stderr 和 JSON 里）。

---

## 5. 信任模型（请读这一段）

* 能调用这个接口的，是**你这个用户、同一个会话里运行的程序**。这和**剪贴板**、**模拟按键（SendInput）**是同一个信任级别：
  能模拟按键的程序本来就能往焦点窗口里打字。
* **密码框永远不写**：焦点是密码框时，接口返回 `SECURE_FIELD`，一个字都不写。
* 写入走的是**真实的 TSF 编辑会话**：宿主拒写就返回 `REFUSED`，**不会**在背后重试；写之前先核对当前**不是选区**
  （有选区就拒绝并告诉你，避免把你选中的内容顶掉）；插入之后光标落在写进去的文字后面。
  **注意**：这一版**没有**「写完再读回来核对」——写没写成功，以宿主的应答为准。
* HTTP 服务**只绑 127.0.0.1** 并且**必须带令牌**；命名管道**只给当前用户**（SDDL）。
* 权限更高的窗口（管理员身份运行的编辑器）**收不到**来自普通权限进程的请求——这是 Windows 的完整性级别在保护你，
  不是缺陷。

---

## 6. 错误码

| 码 | 意思 |
|---|---|
| `NO_IME` | 焦点窗口里没有运行着 IME1.0（切一下输入法再来） |
| `NO_CONTEXT` | 没有可写的文档（焦点不在文本框里） |
| `REFUSED` | 宿主拒绝了这次写入 |
| `SECURE_FIELD` | 密码框，不写 |
| `BUSY` | 输入法正忙（TSF 回调里），稍后重试 |
| `TOO_LARGE` | 内容太大（上限 1 MiB） |
| `MALFORMED` | 请求不是本产品的格式 |
| `UNSUPPORTED` | 这个版本还没有这个能力 |
| `CORE_UNAVAILABLE` | 核心程序（`pime-core.exe`）没找到或死了 |

---

## 7. 自检（三十秒）

```powershell
& "C:\Program Files\IME1.0\pime-ime.exe" status
# 期望：{"ok":true,...}，并且 ime.available 是 true
```
然后在记事本里把输入法切到 IME1.0，光标放在中间，再跑：

```powershell
& "C:\Program Files\IME1.0\pime-ime.exe" insert "IME1.0 写入测试"
```
期望：光标处出现这行字。**这一条还没有在真实宿主里验过**（新 DLL 还没换上）。

---

## 8. 代码位置

| 东西 | 在哪 |
|---|---|
| 线上格式（请求 / 回答 / 错误码） | `01_Source/crates/pime-core/src/agent_proto.rs` |
| DLL 侧：桥接窗口 + 写入 | `01_Source/crates/pime-tsf/src/agent_bridge.rs`、`tip.rs` 的 `answer_agent` |
| 客户端 crate（MCP / CLI / 服务） | `01_Source/crates/pime-agent/` |
| 缺口台账与本轮结论 | `02_Verification/FRAMEWORK_GAPS.md` §5.15 |

---

## 9. 现在验证到哪一步（证据在哪）

| 环节 | 状态 | 证据 |
|---|---|---|
| **通道**（请求跨进程到焦点窗口里的 DLL、答案原路回来） | **已验证**（真机、跨进程） | `crates/pime-tsf/src/agent_bridge.rs` 的 `channel_tests`：宿主进程里是真的桥接窗口与真的 `WM_COPYDATA`，**另一个进程**按协议发请求并等回包 |
| **客户端**（找窗口、发消息、读答案） | **已验证**（真机） | `crates/pime-agent/src/bridge.rs` 的 `channel_tests`：真客户端 + 假桥接窗；外加「桥接不回答时按时放弃并返回 `BUSY`」 |
| **协议本身**（请求 / 回答 / 错误码） | **已验证** | `crates/pime-core/src/agent_proto.rs` 的 10 项往返单测 |
| **MCP / CLI / 本地服务** | **已验证**（在本机用本机构建的 `pime-core.exe`） | `crates/pime-agent/` 的 51 项单测：MCP 的 `initialize` / `tools/list` / `tools/call`、CLI 的各操作、HTTP 的健康检查 / 令牌 / 拒绝、命名管道的请求 |
| **文字真的落进记事本 / VS Code 的文档** | **未验证** | 新 DLL 还没换上（要注销或重启）；跨进程往返测试里的应答函数是**测试用的替身**，不是真的 TSF 写入路径 |
| **装好的安装包里的 MCP 能被客户端拉起来** | **未验证** | 新安装包还没装（上一台机器上的旧 DLL 正被系统占用） |

# Programming IME — 源码 v0.2.0

**状态：VS Code 侧功能完整的编辑器桥接 + 零依赖 Rust 本地核心 + Playwright 用户验收脚本。不是可发布的 Windows 系统输入法。**
Windows TSF 适配器仍是 v0.1 的未编译源码；命名管道 IPC、Credential Manager、签名安装器没有实现。

上游设计基线：`SRS-PIME-001 v1.2` → `BD-PIME-001 v1.0` → `DD-PIME-001 v1.0` → UT/IT/ST。
逐模块的实现/未实现对照见 `docs/IMPLEMENTATION_MATRIX.md`，实际测试结果见 `../02_Verification/STATUS.md`。
**走向纯 Rust 的路线图**（分阶段方案、协议 0.3、JS 胶水上限、待决策事项）见 `docs/RUST_MIGRATION_PLAN.md`。

## 1. 目录

| 路径 | 内容 |
|---|---|
| `language-packs/` | 七种语言的声明式规则包（JSON 数据，JS 与 Rust 共用） |
| `editor-bridges/vscode/` | VS Code 扩展：本地候选、语言锁定/识别、F1 双路径面板、Diff/Hunk、事务应用、BYOK AI、实时 AI、授权/预算/诊断 |
| `editor-bridges/vscode/src/core/` | 与 VS Code 无关的纯逻辑（可单测） |
| `crates/pime-core/` | Rust 核心（零第三方依赖）：同一套语言逻辑 + 输入状态机 + 安装计划 + JSON Lines 协议 0.3（0.2 的超集）；编辑路径（SHA-256/上下文/Diff/事务/授权/预算记账）已进入核心 |
| `tests/` | **所有测试脚本**：`run-all.js` 统一入口、`js/` `rust/` `smoke/` `uat/` `experiments/`、共用夹具与冻结的 v0.2 基线 |
| `tests/uat/` | Playwright 驱动真实 VS Code 的用户验收脚本（32 个场景） |
| `windows/tsf/` | v0.1 的 TSF 实验源码，**未编译、未验证** |

## 2. 在 VS Code 里试用

要求 Windows 11、VS Code 1.94+、Node.js 20+（仅测试需要）。

1. 用 VS Code 打开 `editor-bridges/vscode`，按 **F5** 启动扩展开发宿主。
2. 命令面板运行 `Programming IME: Enable / Disable`。启用前 F1 仍是命令面板。
3. 打开 `.rs/.py/.ts(x)/.ps1/.c` 文件，状态栏显示 `PIME <语言>`（点击可选择/锁定语言；`?` 表示检测到但不确定、保持原语言）。
4. 输入前缀（如 `fnm`、`fori`、`def`、`Ge`）后运行 `Programming IME: Show Code Candidates`（默认 `Ctrl+Alt+Space`）。候选列表标明来源（本地规则 / 用户规则 / 附近代码 / AI）和校验状态。
5. `programmingIme.suggestMode` 设为 `auto` 时会同时显示内联建议（ghost text）。
6. AI（可选）：`Programming IME: Configure AI Provider / API Key`（选择或新建 Provider、HTTPS 端点、模型 ID、Key；Key 只存 SecretStorage）。`aiEnabled` 总开关默认关闭，配置完成后自动打开，可随时用 `Toggle AI` 关闭。
7. **F1**（启用后、编辑器有焦点时）打开面板：`A. 整文件修改` 与 `B. 光标处插入` 各自独立请求、独立授权、独立出错。整文件结果按“修改块”显示，可勾选部分块、预览结果和哈希、打开完整 Diff；点击“应用”才会写入，写入是一个可 `Ctrl+Z` 撤销的编辑，不会自动保存。文档在生成后被修改则拒绝应用。
8. 想保留 F1 给命令面板：`programmingIme.f1Binding` 设为 `shift+f1` 或 `none`；`f1ExcludedLanguages` 可按语言排除。
9. `Manage AI Permissions` 随时撤销授权；`Show AI Usage` / `Show Diagnostics` 查看用量和脱敏诊断（不含源码、提示词、Key）。

用户规则包放在扩展全局存储的 `rules/` 目录（`Programming IME: Open User Rules Folder`），格式同 `language-packs/*.json`（仅允许数据字段，不允许任何脚本）。

## 3. 隐私与安全（实现的行为）

- 本地建议绝不外发代码。AI 默认关闭；开启后每次 F1 都会先显示接收方主机、范围（整文件/附近片段）并要求授权（仅本次 / 当前工作区 24 小时）。
- 发送前扫描疑似密钥/私钥/令牌/`.env`/私人路径：实时 AI 直接阻断高风险内容；F1 让用户选择“脱敏后发送”（占位符，返回后本地还原）、“不脱敏（接受风险）”或取消。出站最后一道闸再次检查，未授权的高风险内容不会被发出。
- 模型回复是不可信数据：只接受 `fullFile/patch/insertCandidates/explanation`，其他字段（如 `tool_calls`、`command`）被忽略并提示；跨文件修改被拒绝；不执行任何命令。
- API Key 只在 SecretStorage；设置、日志、诊断中没有 Key（测试里扫描了用户数据目录）。
- 未受信任工作区禁用 AI；整文件路径需要真实/未命名缓冲区。

## 4. 测试

所有测试脚本都保存在 `tests/` 目录下（说明见 `tests/README.md`）。回归时只**运行**已保存的脚本，不会重新生成夹具、证书或报告（除非显式要求）。

```powershell
node tests/run-all.js              # 快速集合（无 GUI）：语法、夹具比对、黄金基线、Rust(209)、JS(100，含引擎差分)、真实 core.exe 冒烟
node tests/run-all.js --all        # 另加：真实 VS Code 的 UAT（JS 引擎、Rust 引擎各一次，约 8 分钟）
node tests/run-all.js --js         # 单步：--syntax --fixtures --golden --js --rust --smoke --uat --uat-rust
node tests/run-all.js --uat --only-uat "UAT-23"
node tests/run-all.js --report     # 显式：重写 02_Verification 的 UT/IT/ST 追踪报告
node tests/gen-fixtures.js --write # 显式：有意修改行为后才重新生成黄金夹具（默认只比对）
```

UAT 需要 VS Code 和 OpenSSL（Git for Windows 自带）；Rust 构建输出在 `CARGO_TARGET_DIR`（默认系统临时目录），Rust 引擎的 UAT 证据写到临时目录，不覆盖 `02_Verification/uat/`。

Rust 核心可选接入扩展：构建 `pime-core`，把 `programmingIme.coreExecutable` 指向它（只配置可信二进制）。
`programmingIme.engine`：`js`（默认）= 内置 JS 引擎，核心只加速本地候选（协议 0.2 或 0.3 的 `predict`），任何故障自动回退到 JS；
`rust` = **必须**有可用的核心（协议 0.3），核心缺失或中途停止时显示“core unavailable”并停用 IME 功能（不回退到 JS，VS Code 自身输入不受影响）。修改后需重新加载窗口。
用 Rust 引擎跑用户验收：`node tests/run-all.js --uat-rust`（见 `tests/uat/README.md`）。迁移进度见 `docs/RUST_MIGRATION_PLAN.md` 第 13 节。

## 5. Windows TSF 工程

`windows/tsf/` 是 v0.1 的 C++ 实验源码（已被取代，仅作历史参考）。当前的系统输入法是纯 Rust 的 `crates/pime-tsf`（不在主工作区内，用 `tests/run-all.js --tsf` 构建和检查）：按键处理、候选窗、注册代码都已有单元测试，但**从未注册、COM 部分从未在 TSF 里运行过**。要验证 G0 需要一个可丢弃的 Windows 11 虚拟机，步骤见 `docs/TSF_VM_TEST_PLAN.md`。

## 6. 许可

原型源代码为 MIT 许可；不含 Rime/Espanso 等 GPL 项目源码。

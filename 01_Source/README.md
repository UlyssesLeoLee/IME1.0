# Programming IME — 第一版源码 v0.1.0

**状态：可运行的编辑器侧功能原型 + Rust 本地候选引擎源码 + 未验证的 TSF 注册适配器。不是可直接发布的 Windows 系统输入法。**

上游设计基线：`SRS-PIME-001 v1.2` → `BD-PIME-001 v1.0` → `DD-PIME-001 v1.0` → UT/IT/ST。
本包附带的九份设计资料保持原始文件，不把尚未开发的功能宣称为已完成。

## 1. v0.1 已实现与未实现

| 部分 | 已实现 | 尚未实现／待验证 |
|---|---|---|
| Rust Core | 七语言本地候选、文件后缀+保守词法检测、x86-64 5 个明确机器码示例、JSONL CLI | 全语法解析、完整机器码汇编器、TSF 组合 API、签名 |
| VS Code Bridge | 即时候选和语言下拉；BYOK Key / 模型 / Effort；F1 双建议生成；整文件 Diff；光标插入；确认/版本/光标校验 | 多宿主 IDE、Native TSF 浮窗、Patch hunk 编辑、生产级安全事务与全面 E2E |
| Windows TSF | `ITfTextInputProcessor` / `ITfKeyEventSink` 最小 C++ 源码，预设 zh-CN / en-US / ja-JP Profile | Windows 11 编译/安装/注册实测；候选窗口、输入提交、Bridge IPC（当前按键全部直通） |
| 验证 | JavaScript 12 项 `node:test` 通过；JS 文件 syntax check | Rust 14 项需要有 Cargo 的环境执行；Windows 编译、IT/ST 均未执行 |

### 设计与源码差异

- 设计书中的 Named Pipe `PIME-IPC/1` 尚未实现。当前 Rust JSONL 仅用于同机**本地受信进程**试验。
- Windows TSF 组件还不与 Rust Core/VS Code Bridge 通信。
- VS Code 默认关闭扩展及 F1 重绑，启用后 F1 将覆盖编辑器当前帮助快捷键。
- AI 使用 SecretStorage 而不是独立的 Windows Credential Manager worker；Provider 目前支持 OpenAI Responses 与 OpenAI-compatible Chat Completions，其他 Provider 待实现。
- 真正的全文件修改仅在用户点击批准后通过当前 VS Code 文档缓冲区进行，**不会自动保存或执行**。

## 2. 最快体验：VS Code 编辑器原型

要求 Windows 11 + VS Code 1.94+，Node.js 20+（逻辑测试）。无需先编译 Rust Core，扩展带有 JS 本地候选后备。

1. 用 VS Code 打开 `editor-bridges/vscode` 目录。
2. 按 **F5**，选择 `Programming IME: VS Code Extension Host`，打开新的扩展开发宿主。
3. 在新窗口打开 `.rs`、`.py`、`.ts`、`.ps1`、`.c`、`.asm`、`.hex` 任意文件。
4. 命令面板执行 `Programming IME: Enable / Disable` 启用候选；状态栏 `PIME rust` 可下拉切换七种语言。
5. 使用 `Ctrl+Alt+Space` 打开本地候选；如果 VS Code 的行内补全已开启，键入 `fnm`、`fori` 等可看到行内建议。
6. 可选：运行 `Programming IME: Configure AI Provider / API Key`，填写 HTTPS 地址、精确模型 ID、API Key（保存在 SecretStorage）。
7. 使用 `Programming IME: Set Model and Effort` 设定 Effort；只有已核实支持该参数的模型才应声明 `supported`。
8. **F1** 调出 AI 编辑（用户需在每次发送全文前授权）；AI 同时生成整文件和当前位置备选，经选择和确认后才会应用。
9. `Programming IME: Toggle Realtime AI` 默认关闭，首次开启需确认工作区/目标主机及发送附近源码授权。更换 API 目标主机后，旧授权不会自动继承。

> F1 默认键位只有启用扩展时才绑定，不会预先全局拦截。无有效模型及 Key 时，本地规则仍可工作。

也可在 Windows PowerShell 下运行：`scripts/run-vscode-prototype.ps1`。

## 3. Rust Core 编译和检查

安装 Rust stable toolchain 后，在仓库根目录：

```powershell
cargo build --workspace --release
cargo test --workspace
Get-Content .\tests\fixtures\core-jsonl.txt | .\target\release\pime-core.exe
```

在 VS Code 用户配置 `programmingIme.coreExecutable` 指向编译出的 `pime-core.exe`（**仅配置可信任的本地二进制**），即可由扩展用本地 JSONL 子进程获取 Rust 候选；否则使用内置 JS 候选。

最小协议示例：

```json
{"op":"predict","language":"rust","prefix":"fnm","seq":1}
```

## 4. 测试

```powershell
.\scripts\build.ps1
```

在 Linux/macOS（不构建 Windows TSF）可执行：`bash scripts/test.sh`。Node.js 测试单独运行：

```bash
cd editor-bridges/vscode
npm test
npm run check
```

对应详细执行边界详见 `docs/IMPLEMENTATION_MATRIX.md` 和 `docs/MANUAL_E2E.md`。

## 5. Windows TSF 工程（仅隔离实验环境）

`windows/tsf/` 中有 CMake/C++ COM DLL 注册源码，计划使用 Windows SDK 编译。
**该部分尚未编译验证、未数字签名，没有自动注册脚本，也不是正式发行安装器。**
三语言中可见性、注册、激活和解除注册的正确性均需真实 Windows 11 虚拟机验收。

详细说明见 `windows/tsf/README.md`。

## 6. 隐私和安全

- 本地建议绝不向外发送代码。
- 实时 AI 默认关闭；需信任工作区、用户启用并按工作区+目标主机授予许可。
- F1 发送完整文件前逐次提示。文件中若包含密码、密钥或商业秘密，请勿批准发送。
- API Key 放入 VS Code SecretStorage；源代码文件、API Key、AI 回复不写入本包日志。
- 网络或 AI 失败时不应中断本地输入；不执行候选代码、PowerShell 指令或机器码。
- 文件改动后旧 AI 建议被拒绝，不做静默覆盖；按 Ctrl+Z 撤销行为仍需在目标系统验证。
- 本项目原型源代码为 MIT 许可；不含 Rime/Espanso 等 GPL 项目源码。

## 7. 参考规范与设计

- Microsoft TSF 注册：https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration
- Microsoft TSF 键事件：https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfkeyeventsink
- VS Code Extension API：https://code.visualstudio.com/api/references/vscode-api

详见合集内 `00_Documents/` 的九份原始设计及需求追踪文件。

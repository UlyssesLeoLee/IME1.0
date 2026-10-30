# Programming IME — 全部设计文档与第一版源码合集

- 交付版本：**Code PoC v0.1.0**（2026-10-30）
- 上游文档：SRS v1.2 / 基本设计 v1.0 / 详细设计 v1.0 / UT、IT、ST v1.0
- 文档组织：参照日本 IPA 的要件定义、基本设计、详细设计和试验分层
- 状态：**功能原型，不是 Windows 正式安装包，不宣称三语言 TSF 已通过测试**

## 目录

- `00_Documents/`：**9 个上版原始设计文件**（6 MD + 3 XLSX），内容未修改
- `01_Source/`：第一版源码、自动化测试、VS Code 原型和 Windows TSF 实验性适配器
- `02_Verification/`：实际运行的测试结果和明确的未执行项目
- `MANIFEST_SHA256.txt`：文件完整性校验清单

## 主要实现

1. `01_Source/crates/pime-core`：Rust 本地引擎（七语言候选、自动识别、有限机器码验证、JSONL CLI）。
2. `01_Source/editor-bridges/vscode`：VS Code 扩展（状态栏语言切换、输入建议、用户 API Key、模型/Effort、F1 全文 Diff 与光标插入）。
3. `01_Source/windows/tsf`：三语言 Profile（zh-CN/en-US/ja-JP）的 C++ COM/TSF 注册**实验性源码**，当前按键直通，未与 Rust 或 AI 连接。
4. `01_Source/docs/IMPLEMENTATION_MATRIX.md`：逐模块实现/未实现对照。

## 立即试用

在 VS Code 中打开 `01_Source/editor-bridges/vscode`，按 **F5** 运行扩展开发宿主。
在命令面板执行 `Programming IME: Enable / Disable`，然后从右下角状态栏切换语言。
用 `Ctrl+Alt+Space` 获取本地候选；配置自己的 API Key 后才可使用 F1 AI 编辑。
详细步骤请阅读 `01_Source/README.md`。

## 验证事实

- JavaScript 的 **12 个单元测试已通过**；语法检查通过。
- 当前环境没有 Rust Cargo，因此 Rust 14 项测试尚未执行。
- Windows TSF、签名安装、VS Code 真实 E2E、IT/ST 尚未执行。
- 设计书的 168 项 UT/IT/ST 是**计划用例**，不是 168 项已通过测试。

## 安全提醒

AI 模式可能向用户指定的第三方 API 发送光标附近代码或当前文件全文，请仅在确认权限、内容与服务商后授权。F1 仅将用户确认的修改应用于编辑器缓冲区，不自动保存或运行。原型不应在生产工作站上注册未签名 TSF DLL；先在隔离 Windows 11 VM 内验证。

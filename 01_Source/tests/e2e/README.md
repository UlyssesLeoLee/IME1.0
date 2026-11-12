# Windows 11 端到端用例运行器（ST-001～ST-005、ST-035）

本目录是**需要真实 Windows 11 虚拟机 + TSF DLL + 安装包**的设计用例的运行器。
六个用例来自 `00_Documents/02_UT_IT_ST/Programming_IME_ST_Test_Design_v1.0.md`：

| 用例 | 标题 | 判定基准（设计书原文） |
|---|---|---|
| ST-001 | 中文系统语言激活并返回原输入法 | PIME 正常出现/激活/输入；原输入法及默认设置不受损 |
| ST-002 | 英文系统语言激活并返回原输入法 | 同上 |
| ST-003 | 日文系统语言激活并返回原输入法 | 同上 |
| ST-004 | 三系统语言共享同一七语言引擎 | 3×7=21 组合均有效；UI locale 独立于 Profile |
| ST-005 | 普通记事本 Direct 长文本输入 | 宿主内容与标准键盘基准等同，未丢/重字 |
| ST-035 | 签名安装/修复/卸载与重启 | 仅本产品组件被变更；升级失败可回滚 |

运行器**不靠人眼看屏幕**：每一步都落到机器可读的事实上（注册表、文件、进程、Notepad 写到磁盘的字节、
TSF API 的返回值），结果写进 `tests/e2e/results/`，**不写 `02_Verification/`**（证据目录由交付负责人维护）。

> **本仓库现状：本运行器尚未在任何真实 Windows 11 虚拟机上执行过。** 代码已实现并做了语法/静态校验，
> 但「能跑通」这件事必须在真实 guest 上验证。详见下文「哪些还没验证」。

---

## 1. 前置条件

| 项 | 要求 |
|---|---|
| 宿主 | Windows 10/11 + PowerShell 5.1（或 PowerShell 7）；无需管理员 |
| guest | **Windows 11 x64**，可丢弃（Hyper-V 快照 / 全新虚机）。本 DLL 只有 x64 |
| 传输 | WinRM（`Enter-PSSession` 可用）或 OpenSSH（`ssh` 客户端 + 密钥，`BatchMode=yes` 不接受交互式口令） |
| guest 账号 | 能创建计划任务、注册表写 HKLM、写 `C:\Program Files` 的**管理员**账号 |
| 交互账号 | guest 里**已登录的桌面账号**（`interactiveUser`）。输入法、记事本、计划窗口都跑在这个会话里 |
| 安装包 | `node scripts/build-installer.js` 产出的 `01_Source/dist/ProgrammingIME-<版本>-win-x64-setup.exe` |
| 语言 | guest 已装中文（简体）、英语（美国）、日语，以及每种语言的**原输入法**（微软拼音 / US 键盘 / 微软日语） |
| 其它输入法 | 用于 ST-001～003 的「原输入法不受损」判定，guest 里必须已存在 |

**安全规则**（与 `01_Source/docs/TSF_VM_TEST_PLAN.md` 第 0 节一致）：只在可丢弃的虚拟机里跑；装完/卸完都还原快照；
guest 里不要登录真实账号、不要放真实密钥；注册会写 HKLM 并让 Windows 在所有应用里加载这个 DLL。

---

## 2. 指向一台 guest

1. 复制配置：`Copy-Item tests\e2e\vmconfig.example.json tests\e2e\vmconfig.json`（**不要提交** `vmconfig.json`）。
2. 填 `vmconfig.json`：

| 键 | 含义 |
|---|---|
| `transport` | `winrm` 或 `ssh` |
| `host` / `port` / `useSsl` | guest 的地址；WinRM 默认 5985（HTTP）/ 5986（HTTPS） |
| `username` | 连接用的管理员账号（WinRM 用它 + 口令；ssh 用它 + 密钥） |
| `passwordEnvironmentVariable` | 存放该账号口令的环境变量名（默认 `PIME_E2E_PASSWORD`），**口令不写进配置文件** |
| `interactiveUser` | guest 里已登录的桌面账号（`DOMAIN\user` 或 `user`）；计划任务用它建交互式会话 |
| `workDirectory` | guest 里的工作目录，默认 `C:\pime-e2e`；运行器会往这里拷安装包和脚本 |
| `installer` | **本机**上的安装包路径（运行器会拷进 guest） |
| `olderInstaller` | 可选：用于 ST-035「升级」步骤的旧版本安装包；留空则该步骤记 `NOT RUN` |
| `expectedVersion` | 升级后「应用和功能」里应显示的版本号 |
| `composition.keys` / `composition.commitKey` | 提交候选的按键（默认 `ctrl+alt+space` 进入组合、`enter` 提交），见 `docs/TSF_VM_TEST_PLAN.md` 第 8 节 |
| `restart.enabled` | 是否允许重启 guest（ST-001～003、ST-035 的「重启重复」需要） |

3. 设置口令：`$env:PIME_E2E_PASSWORD = '…'`。

## 3. 运行

```powershell
cd <仓库>\01_Source
node scripts/build-installer.js                 # 先产出安装包（unsigned，除非你自己签了名）

powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config tests\e2e\vmconfig.json
powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config tests\e2e\vmconfig.json -Case ST-005
powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config tests\e2e\vmconfig.json -PlanOnly
powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config tests\e2e\vmconfig.json -NoRestart
```

退出码：`0` 全部通过；`1` 有用例失败；`2` 配置或 guest 不可用（**大声失败**，不产出结果）；`3` 没有失败但有检查没执行成。

结果：`tests/e2e/results/e2e-results.json` 与 `e2e-results.md`。

---

## 4. 每个脚本做什么

```
tests/e2e/
├── Invoke-E2E.ps1          入口：读配置、探活 guest、准备 guest 目录、逐个跑用例、出报告
├── vmconfig.example.json   配置样例
├── lib/
│   ├── Common.ps1          配置校验、日志、结果模型（Pass / Fail / NOT RUN）、报告输出
│   ├── Guest.ps1           传输层：WinRM 与 ssh；计划任务（S4U 提权 / Interactive 桌面会话）；重启与等待
│   └── Cases.ps1           用例用到的宿主侧动作（拷文件、取系统/用户状态、装/卸、改默认输入法、打字、快照对比）
├── guest/                  拷进 guest 执行的脚本
│   ├── PimeKeys.ps1            键盘注入（Unicode 逐字符 + 组合键）与「往记事本打字并读回磁盘」
│   ├── Invoke-SystemState.ps1  注册表/文件/签名/七语言包/AI 进程
│   ├── Invoke-UserState.ps1    语言列表、默认输入法、LastProfile、产品设置里的 UI locale
│   ├── Invoke-SetImeState.ps1  把键盘挂到某个语言上、把默认输入法切到某个 Profile
│   ├── Invoke-NotepadTyping.ps1 在真实记事本窗口里打字并保存
│   ├── Invoke-CandidateWindow.ps1 用 UI Automation 窥视候选窗口
│   ├── Invoke-ActiveProfile.ps1 用 ITfInputProcessorProfiles 问「现在激活的是哪个 Profile」
│   ├── Invoke-Snapshot.ps1     注册表与程序列表快照（用于「只有本产品组件变化」）
│   └── Invoke-Installer.ps1    跑安装器/卸载器，返回退出码与日志
└── cases/                  六个用例（每个一个文件）
```

**为什么用计划任务**：WinRM / ssh 的会话在 session 0，**没有桌面**，SendKeys、UIAutomation、输入法激活都做不了。
所以 `Guest.ps1` 用计划任务把脚本投到 guest 的**交互式会话**里跑（`LogonType Interactive`，需要该账号已登录）；
需要写 HKLM / Program Files 的脚本用 `LogonType S4U` + `RunLevel Highest` 提权运行。

---

## 5. 六个用例逐步怎么被判定

### ST-001 / ST-002 / ST-003（中文 / 英文 / 日文系统语言）

设计步骤：`安装勾选该语言 → Win+Space 切换至 PIME → 输入 Rust → 切回微软/系统原 IME → 重启重复`。

| 步骤 | 机械判定 |
|---|---|
| 环境 | guest 必须是 Windows 11（build ≥ 22000），否则 Fail |
| 安装 | 产品没装就先跑安装器；退出码必须是 0（或 3010） |
| 出现 | `HKLM\SOFTWARE\Microsoft\CTF\TIP\{43F95ED1-…}\LanguageProfile\0x00000804 / 0x00000409 / 0x00000411` 下必须有本产品自己的 Profile GUID |
| 勾选该语言 | 交互会话里 `Get-WinUserLanguageList` 中该语言必须带 `0804:{CLSID}{PROFILE}` 形式的键盘；没有就补上（记下是补的还是本来就有的） |
| 激活 | `Set-WinDefaultInputMethodOverride`（Win+Space 的机械等价）后，`Get-WinDefaultInputMethodOverride` 必须等于本产品的 tip |
| 输入 | 在**真实记事本**里用 `keybd_event` 注入 `Rust`，读回磁盘内容，必须**逐字等于** `Rust`（不丢、不重、不被吞） |
| 返回原输入法 | 切回原输入法后，再用同样方式打一遍；两次的**文件哈希必须相同**（原输入法行为与切走之前完全一致） |
| 默认设置不受损 | 切走再切回后，原语言列表里的每一个键盘都还在；默认输入法回到原值 |
| 重启重复 | 重启 guest，再走一遍激活 + 输入，结果必须一致（`-NoRestart` 时该步记 `NOT RUN`） |

### ST-004（3×7=21）

| 层 | 判定 |
|---|---|
| A 注册 | 三个语言 Profile 必须挂在**同一个 TIP** 下；CLSID 必须已注册；安装后的二进制里必须找得到七个语言包（assembly / c / machine-code / powershell / python / rust / typescript） |
| B 21 格 | 对 3 个系统语言 Profile × 7 种编程语言：切到该 Profile → 输入该语言的前缀 → `ctrl+alt+space` 进组合 → `enter` 提交 → 读回记事本内容，与仓库自己的金标准 `tests/fixtures/predict-cases.json` 里该前缀的**首选候选**比对（比对时忽略空白，因为提交文本里的换行是 `docs/TSF_VM_TEST_PLAN.md` 12.4 第 3 条记录的已知遗留问题，不该由它决定结论） |
| C 下拉菜单 | 用 UI Automation 窥视候选窗口：能找到编程语言列表 → Pass；找不到 → **NOT RUN**（写明窗口里看到了什么），不假装通过 |
| UI locale 独立 | 切换前后产品设置里的 UI locale 必须一致；guest 里根本没有持久化 locale 时记 `NOT RUN` |

### ST-005（记事本 Direct 长文本）

| 步骤 | 机械判定 |
|---|---|
| AI 关闭 | guest 里不得有 `pime-ai-worker.exe` 进程 |
| 基准 | 先用**原输入法**打一遍 1000 字符（含中英文符号）+ CRLF + 3 次退格，存下的文件即「标准键盘基准」（含哈希） |
| 对照 | 再用产品打**完全相同**的按键序列，两个文件的 **SHA-256 必须相同**、长度必须相同 → 「未丢/重字」 |
| 开头 | 文件开头 200 字符必须与输入一致 |

### ST-035（签名安装/修复/卸载与重启）

| 步骤 | 机械判定 |
|---|---|
| 签名 | `Get-AuthenticodeSignature` 必须为 `Valid`（**当前仓库产出的安装包未签名，这一步会如实 Fail**） |
| 新装 | 退出码 0/3010；三个 Profile 与 CLSID 就位；「应用和功能」里有条目 |
| 只变本产品 | 安装前后导出 `HKLM\…\CTF\TIP`、`HKCU\…\User Profile`、`HKLM\…\Uninstall` 与程序列表逐行对比，**每一处差异都必须包含本产品的 GUID 或安装目录**，否则 Fail |
| 修复 | 再跑一次同一个安装器：退出码 0，且文件哈希集合不变 |
| 升级 | 配了 `olderInstaller` 才跑：先装旧版再装新版，「应用和功能」里的版本必须等于 `expectedVersion`；没配 → NOT RUN |
| 回滚 | 用 `icacls /deny` 让安装目录写不进去 → 安装器必须失败（非 0 退出）且**注册表与基线完全一致**（无残留） |
| 卸载 | 退出码 0；TIP 键、CLSID 键、「应用和功能」条目都消失 |
| 重启 | 重启后安装目录真的没了（重启时删除的文件被执行完） |

---

## 6. 能验证什么 / 不能验证什么

**能（机械判定）**：注册与枚举、三个 Profile 的挂载与激活、真实记事本里的逐字符输入、
原输入法行为前后一致、注册表/程序列表的差异范围、安装/修复/升级/卸载的退出码与残留、
回滚、签名状态、21 格的候选提交。

**不能（本运行器不做，诚实记 `NOT RUN` 或在 README 里说明）**：

- **候选窗口的手工操作**（鼠标点下拉、滚动、选词）——UI Automation 能看见列表时只做「可见性」判定，
  不代替人手点选；产品若不暴露可访问元素，该项记 `NOT RUN`。
- **录屏、截图**——设计书要求保存录屏，本运行器只产出机器可读的事实与文件哈希。
- **多显示器、DPI 100/150/200%、读屏软件**——属于 ST-032，不在本目录范围。
- **30 分钟长跑、基准机 P95/CPU**——属于 ST-038/039/040，需要基准机与长时间运行。
- **商店应用 / 开始菜单搜索 / 密码框的负向路径**——属于 IT-005 与兼容性分级报告，需要专门的自写宿主。
- **未签名安装包在真实机器上的 SmartScreen 提示**——当前构建未签名（见 `01_Source/scripts/build-installer.js` 的说明），
  ST-035 的签名判定会直接报 Fail。

## 7. 还没验证的部分（本仓库现状）

- 运行器**从未在真实 Windows 11 guest 上跑过**：传输层、计划任务的交互式会话、UIAutomation 的窥视、
  `keybd_event` 注入的实际效果，都只有静态校验（PowerShell 语法解析 + ASCII 约束）。
- `composition.keys`（默认 `ctrl+alt+space`）取自 `docs/TSF_VM_TEST_PLAN.md` 第 8 节，**未与当前构建核对**；
  若当前构建的组合键不同，改 `vmconfig.json` 的 `composition` 即可，不需要改代码。
- ST-004 的 21 格按「输入前缀触发候选」实现，**不是**设计书里「下拉里选语言」的那条路径；
  两者的差异见上文第 C 层。

## 8. 约定

- `.ps1` 一律 **ASCII**（与仓库其它脚本一致）：Windows PowerShell 5.1 在非 UTF-8 代码页上会把无 BOM 的 UTF-8 读坏。
  中文只出现在本 README 和生成的结果文件里。
- **控制台上的中文标题会显示成乱码**（GBK/932 代码页的宿主）：运行器在运行时从设计书里取用例标题，
  而控制台按自己的代码页渲染。**这不影响判定**——判定读的是 `tests/e2e/results/` 里的 UTF-8 结果文件；
  脚本自己的日志串全是 ASCII。
- 结果文件写在 `tests/e2e/results/`；`02_Verification/` 由交付负责人维护，本目录不写。
- 跑完请还原 guest 的快照。

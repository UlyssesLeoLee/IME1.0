# TSF 虚拟机测试手册（里程碑 M4 / M5 的前置验证）

> **状态（如实记录）：这份手册从未被执行过。** `DllRegisterServer` / `DllUnregisterServer` 已经写好并通过了内存假后端的单元测试，
> 但**没有在任何真实的 Windows 注册表或真实的 TSF 上运行过**。下面每一个“预期”都是设计推断，不是观察结果。
> 以实测为准，和预期不符的地方就是这次测试要找的东西。
>
> 当前 DLL 的能力：能注册、能被 Windows 加载并激活，**不处理任何按键**（`tip` 模块还是桩：所有按键留给宿主）。
> 所以第 6 步以前的检查现在就能做；第 7–9 步里“功能”类检查要等 M3（按键、组合、候选窗接入 TIP）完成后才有意义，
> 在此之前记 `NOT RUN (stub)`，“直通”类检查（打字照常、什么都不吞）现在就能做。
>
> TSF UI 元素协议（候选列表告知宿主，第 5.1 步的 `UIELEMENTENABLED` 与第 9.1 步）已经写好，同样**只在假宿主和进程内调用上测过，从未在真实 TSF 里运行**。

## 0. 安全规则（先读）

1. **只在可丢弃的虚拟机里做。绝不在日常工作机上注册这个 DLL。** 注册会写 `HKLM`，并让 Windows 在所有应用里加载它。
2. **动手前先做检查点（Hyper-V）或确认沙盒是新开的。** 回滚办法见第 11 步。
3. 虚拟机里不要登录真实账号，不要放真实密钥。
4. DLL 没有签名。部分受保护或强制签名的进程（开始菜单搜索、商店应用、浏览器的某些进程）可能拒绝加载它，这是预期内的现象，要记录，不要绕过。
5. 本手册只用到 `regsvr32`、`reg query/export` 和只读的 PowerShell 命令。唯一会修改输入法列表的命令在第 5 步，带了警告。

## 1. 选环境

| 环境 | 适合 | 限制 |
|---|---|---|
| **Hyper-V 虚拟机（推荐）** | 全部检查，包括重启（ST-001..003 要求“重启重复”）、检查点回滚 | 需要 Windows 11 Pro/Enterprise 主机，开启 Hyper-V（管理员 + 重启主机） |
| Windows Sandbox | 快速冒烟（注册、列表、切换、卸载） | 关闭即丢失全部状态；在沙盒里重启可能结束会话（取决于系统版本，没验证），所以**重启类检查要用 Hyper-V**；多显示器、DPI 切换受限 |

主机上启用功能（管理员 PowerShell，需要重启主机；这是你自己决定的唯一一次主机改动）：

```powershell
# Hyper-V
Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All
# Windows Sandbox
Enable-WindowsOptionalFeature -Online -FeatureName Containers-DisposableClientVM -All
```

虚拟机要求：

- Windows 11 **x64**（本 DLL 只有 x64；ARM64 虚拟机和 32 位程序看不到它，见第 9 步“已知限制”）。
- 已经装好并能用的其它输入法，用来验证“不破坏原输入法”（IT-002）：en-US 的 US 键盘、zh-CN 的微软拼音、ja-JP 的微软日语输入法。
  在 设置 > 时间和语言 > 语言和区域 > 添加语言 里添加中文（简体，中国）、日语；**不要勾选“设为我的 Windows 显示语言”**。
- 主机端 Hyper-V 虚拟机已做好检查点，名字例如 `before-pime`。

## 2. 在主机上构建并确认 DLL

在仓库的 `01_Source` 目录运行（只构建和检查，不注册、不加载）：

```powershell
cd <仓库>\01_Source
node tests\run-all.js --tsf
```

DLL 在 `%TEMP%\pime-cargo-target\x86_64-pc-windows-gnu\release\pime_tsf.dll`（若设置了 `CARGO_TARGET_DIR`，则在那里）。确认它是 x64：

```powershell
node scripts\pe-info.js "$env:TEMP\pime-cargo-target\x86_64-pc-windows-gnu\release\pime_tsf.dll"
```

预期输出：`machine: x64 (AMD64)`，`kind: DLL`，导出恰好 `DllCanUnloadNow, DllGetClassObject, DllRegisterServer, DllUnregisterServer`，
导入只有系统 DLL（`advapi32`、`ole32`、`kernel32` 等）。不是 x64 就停下来。

DLL 的来源现在只有这一条（上面的构建）。安装器（M5）还不存在，将来用安装器装出来的 DLL 要重复一遍本手册。

记录哈希和提交号，写进第 12 步的环境表：

```powershell
Get-FileHash "$env:TEMP\pime-cargo-target\x86_64-pc-windows-gnu\release\pime_tsf.dll" -Algorithm SHA256
git rev-parse HEAD
```

## 3. 把 DLL 放进虚拟机

- Hyper-V：增强会话里复制粘贴，或主机上 `Copy-VMFile -Name <虚拟机名> -SourcePath <dll> -DestinationPath C:\Temp\pime_tsf.dll -FileSource Host -CreateFullPath`（需要开启“来宾服务”）。
- Sandbox：用 `.wsb` 文件只读映射一个文件夹，并关掉网络：

```xml
<Configuration>
  <Networking>Disable</Networking>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>D:\ime-vm-share</HostFolder>
      <SandboxFolder>C:\share</SandboxFolder>
      <ReadOnly>true</ReadOnly>
    </MappedFolder>
  </MappedFolders>
</Configuration>
```

然后在虚拟机里（管理员 PowerShell）把它放到 **`C:\Program Files\PIME\`**，不要放桌面或用户目录：

```powershell
New-Item -ItemType Directory -Force 'C:\Program Files\PIME' | Out-Null
Copy-Item C:\Temp\pime_tsf.dll 'C:\Program Files\PIME\pime_tsf.dll'
Get-FileHash 'C:\Program Files\PIME\pime_tsf.dll' -Algorithm SHA256   # 必须和主机上的哈希一致
```

原因：商店应用和开始菜单搜索在 AppContainer 里运行，通常读不了用户目录里的 DLL（那里没有“所有应用程序包”的读取权限）。放在用户目录里，沉浸式检查会因为权限而失败，和代码无关。

之后的 PowerShell 命令都假定先运行了这段变量定义（每开一个新的管理员窗口都要重新运行）：

```powershell
$Dll    = 'C:\Program Files\PIME\pime_tsf.dll'
$Regsvr = "$env:SystemRoot\System32\regsvr32.exe"      # 64 位版本；SysWOW64 里的是 32 位版本，会失败
$Clsid  = '{43F95ED1-95B4-421D-8F5A-837128C2D854}'
$Prof   = @{ zh = '{223FD919-F8CB-476C-A852-351E7381C801}'; en = '{68ED92DE-77CF-43B1-A7B2-D3BE2686E2A0}'; ja = '{22E54B52-5082-42D3-9E41-1C2E0CE74DFB}' }
New-Item -ItemType Directory -Force C:\pime-test | Out-Null
[Environment]::Is64BitProcess    # 必须是 True：在 32 位 PowerShell 里 System32 会被悄悄重定向到 SysWOW64，$Regsvr 就成了 32 位版本

function Save-ImeState($label) {
  $d = "C:\pime-test\$label"; New-Item -ItemType Directory -Force $d | Out-Null
  Get-WinUserLanguageList | ForEach-Object { "$($_.LanguageTag): $($_.InputMethodTips -join ', ')" } | Out-File "$d\langlist.txt"
  Get-WinDefaultInputMethodOverride | Out-File "$d\override.txt"
  reg export 'HKCU\Control Panel\International\User Profile' "$d\hkcu-userprofile.reg" /y | Out-Null
  reg export 'HKCU\Software\Microsoft\CTF' "$d\hkcu-ctf.reg" /y | Out-Null
  reg export 'HKLM\SOFTWARE\Microsoft\CTF\TIP' "$d\hklm-ctf-tip.reg" /y | Out-Null
  reg query "HKLM\SOFTWARE\Classes\CLSID\$Clsid" /s 2>&1 | Out-File "$d\clsid.txt"
  Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\CTF\TIP' | Select-Object -ExpandProperty PSChildName | Out-File "$d\tip-list.txt"
  "saved to $d"
}
```

## 4. 注册前的基线（IT-002 的“前”）

```powershell
Save-ImeState before
```

同时：截图 设置 > 时间和语言 > 语言和区域，记下任务栏右下角当前的输入法指示和三个语言各自的键盘列表；
记录系统版本（`winver`，或 `[Environment]::OSVersion.Version`）。再做一个检查点 `registered-baseline`（可选）。
`before\clsid.txt` 应该是“找不到”之类的错误，说明 CLSID 此前未注册。

## 5. 注册

在**管理员** PowerShell 里：

```powershell
& $Regsvr $Dll
```

预期弹窗：`DllRegisterServer in ...pime_tsf.dll succeeded.` 截图。失败时弹窗里有十六进制错误码，常见的：

| 代码 | 含义 | 处理 |
|---|---|---|
| `0x80070005` | 拒绝访问：窗口没有提权 | 重新以管理员运行。此时一个注册项都不会写（第一步就被拒绝） |
| `0x800700C1` | 位数不对：用了 32 位 regsvr32，或虚拟机不是 x64 | 用 `System32` 里的 regsvr32 |
| `0x8007007E` | 找不到模块或它依赖的 DLL | 确认路径；DLL 只导入系统 DLL |
| `0x80070057` | 参数无效：`RegisterProfile` 拒绝了某个参数 | 最可疑的是空的图标路径（目前不带图标，没有验证 TSF 接受空串）；记下是哪个语言 |
| `0x80004005` 等 | TSF 拒绝某一步 | 记下代码。失败时 DLL 会回滚已写入的内容；用 `Save-ImeState after-failed` 确认没有残留 |

写脚本时可以加 `/s`（静默），再看 `$LASTEXITCODE`：0 成功；3 加载 DLL 失败；5 `DllRegisterServer` 返回了失败（按 regsvr32 的文档，实测为准）。

想知道是哪一步失败：在虚拟机里运行 Sysinternals **DebugView**（管理员，Capture > Capture Global Win32），
失败时会看到 `pime-tsf register: <步骤名> failed: 0x........`（只有步骤名和代码，没有路径和值）。

### 5.1 检查写入的内容（只读）

```powershell
reg query "HKLM\SOFTWARE\Classes\CLSID\$Clsid" /s
reg query "HKLM\SOFTWARE\Microsoft\CTF\TIP\$Clsid" /s
```

预期（实测为准；CTF 下的键布局是 Windows 内部的，没有公开文档）：

| 位置 | 预期 |
|---|---|
| `HKLM\SOFTWARE\Classes\CLSID\{43F95ED1-...}` 默认值 | `Programming IME` |
| 其下 `InprocServer32` 默认值 | `C:\Program Files\PIME\pime_tsf.dll`（**没有引号**） |
| `InprocServer32` 的 `ThreadingModel` | `Apartment` |
| `HKLM\SOFTWARE\Microsoft\CTF\TIP\{43F95ED1-...}\LanguageProfile` | 三个语言键 `0x00000804`、`0x00000409`、`0x00000411`，各含一个本产品的 profile GUID |
| 同一 TIP 键下的 `Category` | 只含下面三个类别；**不含 SECUREMODE、IMMERSIVESUPPORT**（见下） |

类别 GUID：`TIP_KEYBOARD {34745C63-B2F0-4784-8B67-5E12C8701A31}`、`SYSTRAYSUPPORT {25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}`、`UIELEMENTENABLED {49D2F9CF-1F5E-11D7-A6D3-00065B84435C}`。

`UIELEMENTENABLED` 是新增的第三个类别：候选列表通过 TSF UI 元素协议（`ITfUIElementMgr` / `ITfCandidateListUIElement`）告知宿主，宿主回答“我来画”时本输入法的自绘候选窗保持隐藏。
协议的提供方一侧已实现并只在假宿主上测过，**从未在真实 TSF 里运行**；这一项在虚拟机里怎么查见第 9.1 步。

**有意不注册**（每一项都是对宿主的承诺，实现了才加）：
`IMMERSIVESUPPORT`（没有它 Windows 不会在商店应用和开始菜单搜索里提供本输入法。UI 元素协议是必要条件但不充分：本输入法从未在 AppContainer 里运行过，自绘候选窗不是被拥有的窗口，也没有 IME WinEvent、UI Automation 候选窗属性和品牌图标；逐项原因见 `RUST_MIGRATION_PLAN.md` 13.11），
`SECUREMODE`（不让未审查的 DLL 被加载进登录界面和 UAC 提示）。
**不应出现** `SECUREMODE {49D2F9CE-1F5E-11D7-A6D3-00065B84435C}`（有意不注册：否则 Windows 会在登录界面和 UAC 的安全桌面里加载这个未经审查的 DLL）。
也**不应出现** `IMMERSIVESUPPORT {13A016DF-560B-46CD-947A-4C3AF1E0E35D}`。

再运行两次注册（共三次，对应 IT-001 的“调用三次注册”），每次都应成功，且键值不变（幂等）：

```powershell
& $Regsvr $Dll ; & $Regsvr $Dll
Save-ImeState after-register
Compare-Object (Get-Content C:\pime-test\before\langlist.txt) (Get-Content C:\pime-test\after-register\langlist.txt)
```

### 5.2 在设置里确认三个键盘（IT-001、ST-001..003 的“出现”）

设置 > 时间和语言 > 语言和区域，对每个语言点 `…` > 语言选项 > 键盘：

| 语言 | 预期出现的键盘名 |
|---|---|
| 中文（简体，中国） | `编程输入法 (Programming IME)` |
| 英语（美国） | `Programming IME` |
| 日语 | `プログラミングIME (Programming IME)` |

- 若没有自动出现：点“添加键盘”，选它。**记录这一点**：它回答了“`bEnabledByDefault` 是否让键盘自动出现”。
- 图标：现在没有图标资源，预期是 Windows 的默认键盘图标。记录实际看到的（空白也算一个结果）。
- 若某语言没有安装，对应键盘不会出现，这是预期；先添加语言再检查。
- 命令行替代（**会改写这个虚拟机里当前用户的语言列表，只在虚拟机里用**）：

```powershell
$l = Get-WinUserLanguageList
$zh = $l | Where-Object LanguageTag -like 'zh-Hans*'
$zh.InputMethodTips.Add("0804:$Clsid$($Prof.zh)")
Set-WinUserLanguageList $l -Force
```

## 6. 激活与切换（ST-001、ST-002、ST-003）

每个语言重复一遍（zh-CN、en-US、ja-JP）：

1. 打开记事本，`Win+Space`，选本产品的键盘。任务栏指示应显示它。**Win+Space 是 Windows 的，不是本输入法的**，所以它必须照常工作。
2. 输入 `Rust`。当前桩：字符照常出现（直通）。功能版：见第 8 步的清单。
3. `Win+Space` 切回微软拼音 / US / 日语输入法，确认原输入法照常输入。
4. 重启虚拟机，再做一遍 1–3（ST 要求“重启重复”）。
5. 做 `Save-ImeState after-switch-<语言>`，和 `before` 比较：除本产品 GUID 外不应有差异；默认输入法没变（IT-002）。

### 6.1 三个语言共用同一引擎（ST-004，功能版；桩阶段记 NOT RUN (stub)）

在三个输入语言（zh-CN、en-US、ja-JP）下，各从下拉里选一种编程语言，输入标准片段并选一个候选提交。每格填 `Pass` / `Fail` / `NOT RUN`，截图放证据列。
同时确认：改变 Windows 显示语言或输入语言，不会改变本产品界面的语言选择（UI locale 独立于 Profile）。

| 输入语言 \ 编程语言 | Assembly | C | Machine code | PowerShell | Python | Rust | TypeScript |
|---|---|---|---|---|---|---|---|
| zh-CN | | | | | | | |
| en-US | | | | | | | |
| ja-JP | | | | | | | |

## 7. 宿主应用里输入（ST-005、ST-006 以及兼容性）

对下面每一种宿主，切到本产品键盘，各输入一段文字；记录“可用 / 受限 / 不支持”和现象。受限或不支持**不是失败**，要写清原因（设计书对兼容性只要求分级报告）。

| 宿主 | 怎么测 | 备注 |
|---|---|---|
| 记事本 | 输入 1000 个字符加换行（ST-005），再保存，比较哈希 | 见下面的脚本 |
| 浏览器文本框 | Edge 打开一个本地 HTML 页，里面有 `<textarea>` | 浏览器自己也处理 TSF |
| VS Code | 输入 `fn`，看候选，Esc，接受，Ctrl+Z（ST-006） | 桩阶段只能确认“字符照常输入、没有崩溃” |
| UWP / 商店应用 | 例如“设置”应用顶部的搜索框，或任意带文本框的商店应用 | **预期仍然不可用**：UI 元素协议已实现，但 `IMMERSIVESUPPORT` 仍有意不注册（原因见 `RUST_MIGRATION_PLAN.md` 13.11：从未在 AppContainer 里运行、自绘候选窗不是被拥有的窗口、没有 IME WinEvent / UI Automation 属性 / 品牌图标），Windows 不会在商店应用里提供本输入法；确认它没有出现、也没有影响其他输入法即可。**不要把这一行记成“宿主绘制的候选列表已可用”：商店应用不会加载本输入法** |
| 开始菜单搜索 | 按 Win 键直接输入 | 同上；还可能被代码完整性策略拒绝，见下 |
| 密码框 | 在本地 HTML 页里放 `<input type=password>`，在 Edge 中输入 | **必须什么都不显示、什么都不吞**（IT-005）：不出现候选窗，字符正常变成圆点，按键没有被消耗 |
| 登录/锁屏、UAC 提示 | `Win+L`、以管理员运行一个程序 | 键盘列表里**不应**出现本产品（没有注册 SECUREMODE） |

ST-005 的 1000 字符输入可以用脚本发送（在虚拟机里，先把光标点进记事本）：

```powershell
Add-Type -AssemblyName System.Windows.Forms
Start-Sleep 5   # 5 秒内点进记事本
$text = ('abcdefghij' * 100)
[System.Windows.Forms.SendKeys]::SendWait($text)
```

保存为 `C:\pime-test\st005.txt` 后：`(Get-Content C:\pime-test\st005.txt -Raw).Length` 应为 1000，哈希和直接用美式键盘输入的文件一致。

某个宿主里看不到输入法或被拒绝加载时，看事件查看器：应用程序和服务日志 > Microsoft > Windows > CodeIntegrity > Operational，
有没有该 DLL 的拒绝记录；并记录进程名。

## 8. 按键清单（功能版 TIP；桩阶段记 NOT RUN (stub)）

候选框弹出后逐项验证，每项在记事本和 VS Code 各做一次：

| 按键 | 预期 |
|---|---|
| `PageDown` / `PageUp` | 候选翻页；**只有多于一页时才消费**，否则交还宿主（编辑器照常滚动） |
| 数字 `1`–`9` | 只在**当前页**内选词，不跨页 |
| `Up` / `Down`、`Shift+Tab` | 移动高亮（`Shift+Tab` 向前） |
| `Tab` / `Enter` | 确认当前候选 |
| `Esc` | 取消；**不向宿主写入任何字符**；没有候选时 `Esc` 原样交给宿主 |
| `Space` | 只在组合状态确认；非组合状态是普通空格 |
| 单击 `Shift` | 暂停 / 恢复；`Shift` 本身不被吞，`Shift+字母` 照常输入大写 |
| `Ctrl+C/V/Z`、`Alt+Tab`、`Alt+F4`、其它 `Ctrl`/`Alt` 组合 | 完全不受影响 |
| `Win+Space`、`Ctrl+Shift`（切换输入法） | 属于 Windows；必须照常切换 |
| `= . ]` / `- , [` | 默认**不**翻页，按普通字符输入（`a.b`、`x - 1` 不能被破坏） |

## 9. 候选窗、DPI、多显示器、已知限制

- 候选窗出现在光标下方；靠近屏幕下边缘时翻到光标上方；**不抢焦点**（输入的字符继续进宿主，宿主标题栏不变灰）。
- DPI：设置 > 系统 > 显示 > 缩放，分别设为 100 %、150 %、200 %（有的应用要注销后重新登录才生效）。每档看窗口大小、文字清晰度和位置。
- 两个显示器：Hyper-V 增强会话可以用“使用所有显示器”；把记事本拖到第二个显示器再输入，窗口应在第二个显示器上跟随光标。做不到就记 `NOT RUN` 并写明原因。
- **已知限制（预期，不记为失败）**：只有 x64 版本，32 位应用看不到本输入法；32 位版需要另行构建并用 `SysWOW64\regsvr32.exe` 注册（尚未构建）。

### 9.1 UI 元素协议（`UIELEMENTENABLED`；下面全是设计推断，这条路径从未在真实 TSF 里运行）

候选列表出现时本输入法调用 `ITfUIElementMgr::BeginUIElement`，列表里有宿主看得到的变化时调用 `UpdateUIElement`，列表消失时调用 `EndUIElement`。
宿主在 `BeginUIElement` 里回答“显示 = FALSE”（它自己画）时，自绘候选窗保持隐藏；回答 TRUE，或线程管理器不提供 `ITfUIElementMgr`，照旧自绘。
宿主随后调用元素的 `Show(TRUE/FALSE)` 会切换同一个决定，`IsShown` 只在自绘窗确实在屏幕上时回答 TRUE。

**调试痕迹**（DebugView，管理员，Capture > Capture Global Win32；每行只有编号、显示答案和错误码，没有候选文本；`UpdateUIElement` 成功时不记录，失败才记录）：

| 行 | 含义 |
|---|---|
| `pime-tsf uielement: ITfUIElementMgr available` | 激活时线程管理器提供了管理器（每个激活的线程一行） |
| `pime-tsf uielement: no ITfUIElementMgr: the native popup draws` | 没有管理器，只用自绘窗 |
| `pime-tsf uielement: begin id=N show=1` / `show=0` | 候选列表出现；宿主让本输入法自绘（1）或自己画（0） |
| `pime-tsf uielement: end id=N` | 这个编号的列表结束 |
| `pime-tsf uielement: begin failed code=0x........`、`update id=N failed ...`、`end id=N failed ...` | 宿主拒绝；begin 失败时改用自绘窗，且同一个列表不再重试 |

检查（每项都要在 DebugView 里对照，不要只看屏幕）：

1. **Begin/End 配对**：每个 `begin id=N` 恰好有一个 `end id=N`，任何时刻最多一个编号未结束。在记事本里做（提交和取消用的按键见第 8 步；默认配置要先按 `Ctrl+Alt+Space` 进入组合）：
   (a) 打字出候选，`Esc`；(b) 打字出候选，提交一个候选；(c) 打字出候选，`Alt+Tab` 到别的窗口（失去焦点）；(d) 打字出候选，`Win+Space` 切走输入法（Deactivate）；
   (e) 打字出候选，翻页、移动高亮、再多打一个字符：这些只能产生 `UpdateUIElement`，不产生新的 `begin`；(f) 候选消失后再打字：新的编号。
   (g) 杀死宿主进程时没有 `end` 是不可避免的（进程没了），不算失败。
2. **常规桌面宿主**（记事本、Edge 文本框、VS Code）：预期 `show=1`，候选窗外观和第 9 步完全一致（回归检查：引入协议后自绘路径不应有任何变化）。**记录每个宿主实际回答的 show 值**，这是本次测试要回答的问题（12.3）。
3. **密码框**：不应出现任何 `begin`（列表的文字会交给宿主，密码框里本来就不显示候选）。
4. **宿主自己画（`show=0`）**：需要一个对 `BeginUIElement` 回答 FALSE 的宿主，也就是实现 `ITfUIElementSink` 并设置 `*pbShow = FALSE`。**仓库里没有这样的测试宿主**。
   商店应用本该是这样的宿主，但本输入法不会被提供给它们（`IMMERSIVESUPPORT` 没有注册，见 13.11），所以不能用商店应用代替。没有这样的宿主就记 `NOT RUN`，不要用别的现象代替。
   有的话预期：`begin id=N show=0`，从 begin 到 end 之间屏幕上**没有**本输入法的自绘窗；宿主的列表显示当前页的行（每行是 `文本 (来源 · 验证状态)`）并高亮当前行；翻页后宿主的列表换成新一页的行。
   宿主的列表**没有“2/3”页码**（元素只描述当前页，翻页仍由本输入法的按键处理，已知限制），宿主不能改页（`SetPageIndex` 回答 `E_NOTIMPL`）也不能替用户选词（没有实现 `ITfCandidateListUIElementBehavior`）。
5. **宿主切换绘制者**：同样需要自写宿主。在列表显示期间宿主调用元素的 `Show(FALSE)`：自绘窗立刻消失；再调用 `Show(TRUE)`：用最新一页重新出现；两种情况都不留残影，且不影响 begin/end 配对。

## 10. 故障测试（ST-037）

目标：输入法出问题时，打字必须继续，不丢字、不重复。

1. **杀宿主**：在记事本里让候选窗显示，然后 `taskkill /f /im notepad.exe`。重新打开记事本：输入正常，没有残留的候选窗，其它应用不受影响。
2. **杀别的宿主**：对浏览器、VS Code 各做一次同样的事。
3. **让预测器失败**：现在 TIP 把状态机放在进程内（没有单独的 Core/Worker 进程），没有现成的故障注入开关。
   等 P3（命名管道）之后：`Stop-Process -Name pime-core -Force`（再对 Worker、UI 各做一次），然后立刻输入 100 个字符（用第 7 步的脚本，`'abcdefghij' * 10`），
   计时到字符恢复直通，并核对文本：直通可用 **≤ 2 s**，**无丢字、无重复字符**。在那之前这一项记 `NOT RUN`，原因写“无故障注入点”。

## 11. 卸载、重启、回滚（IT-001 的“卸载后为 0”，IT-002 的“后”）

管理员 PowerShell：

```powershell
& $Regsvr /u $Dll
& $Regsvr /u $Dll      # 第二次：没有东西可删，应当仍然成功。记录弹窗/代码
Save-ImeState after-unregister
reg query "HKLM\SOFTWARE\Classes\CLSID\$Clsid" /s       # 预期：找不到
reg query "HKLM\SOFTWARE\Microsoft\CTF\TIP\$Clsid" /s   # 预期：找不到；若还有空的键，记录它们的路径
```

然后：

1. 重启虚拟机。设置里三个语言的键盘列表中**不再有**本产品；任务栏输入法列表里也没有。
2. 比较前后：`Compare-Object (Get-Content C:\pime-test\before\langlist.txt) (Get-Content C:\pime-test\after-unregister\langlist.txt)` 应无差异；
   对 `hkcu-userprofile.reg`、`hklm-ctf-tip.reg` 也做比较，**差异只能涉及本产品的 GUID**（`43F95ED1...`、三个 profile GUID）。
3. 其它输入法（微软拼音、US、日语）的 GUID 和默认设置原样保留（IT-002）。
4. HKCU 里可能留下本产品 GUID 的残留（`HKCU\Software\Microsoft\CTF\...`、`HKCU\Control Panel\International\User Profile`）：这是用户自己的状态，
   DLL 不碰它。记录有没有残留、设置界面是否因此出现“幽灵”键盘。
5. DLL 文件能否删除：`Remove-Item $Dll`。因为 `DllCanUnloadNow` 总返回 `S_FALSE`，已经加载过它的进程在退出前会一直占着文件；注销或重启后才能删。这是预期。

**回滚**：Hyper-V 里还原检查点 `before-pime`（主机管理员 PowerShell：`Restore-VMCheckpoint -VMName <虚拟机名> -Name before-pime -Confirm:$false`，或在 Hyper-V 管理器里“应用”检查点）；
Sandbox 里直接关闭窗口。卸载成功不代替回滚：测试完一律还原检查点。无论测试结果如何，都不要在主机上重复这些步骤。

## 12. 记录表

### 12.1 环境

| 项 | 值 |
|---|---|
| 日期 / 测试人 | |
| 虚拟机类型（Hyper-V / Sandbox）与检查点名 | |
| Windows 版本与构建号 | |
| 输入语言与已有的其它输入法 | |
| 仓库提交号（`git rev-parse HEAD`） | |
| `pime_tsf.dll` SHA256（主机 / 虚拟机，须一致） | |
| 阶段（`registration-only 桩` / `M3 已接入`） | |
| DPI / 显示器 | |

### 12.2 结果

“结果”列只写 `Pass`、`Fail` 或 `NOT RUN`（后面可加原因）。初始全部是 `NOT RUN`。证据写文件名（截图、录屏、`C:\pime-test\...` 里的文件、DebugView 日志）。

| # | 检查（本手册步骤） | 设计用例 | 判定标准（摘自设计书） | 结果 | 证据 | 备注 |
|---|---|---|---|---|---|---|
| 1 | 三次注册成功且幂等；设置里出现三个键盘（5、5.1、5.2） | IT-001 | 每个 LANGID 精确对应 1 个自有 Profile；卸载后为 0 | NOT RUN | | |
| 2 | 卸载后三个语言下 profile 数为 0，注册表无残留（11） | IT-001 | 同上 | NOT RUN | | |
| 3 | zh-CN：Win+Space 切换到本产品，输入 `Rust`，切回微软拼音，重启后重复（6） | ST-001 | PIME 正常出现/激活/输入；原输入法及默认设置不受损 | NOT RUN | | 桩阶段“输入”只能验证直通 |
| 4 | en-US：同上，切回 US 键盘（6） | ST-002 | 同上 | NOT RUN | | |
| 5 | ja-JP：同上，切回微软日语输入法（6） | ST-003 | 同上 | NOT RUN | | |
| 6 | 三个语言共用同一引擎，七语言下拉各提交一次，填 21 格矩阵（6.1） | ST-004 | 3×7=21 组合均有效；UI locale 独立于 Profile | NOT RUN | | 需要 M3 和七语言下拉 UI |
| 7 | 记事本输入 1000 字符 + CRLF 并回退（7） | ST-005 | 宿主内容与标准键盘基准等同，未丢/重字 | NOT RUN | | |
| 8 | VS Code：输入 `fn`，看候选，Esc、接受、Ctrl+Z（7、8） | ST-006 | 取消零写入；接受精确位置；Undo 可恢复 | NOT RUN | | 需要 M3 |
| 9 | 安装/切换/卸载前后现有输入法对比（4、6、11） | IT-002 | 非本产品输入法 GUID 与默认设置原样保留 | NOT RUN | | |
| 10 | 密码框里什么都不显示、什么都不吞；登录和 UAC 界面无本产品（7） | IT-005 | 未发 Snapshot/PredictAI，文本不缓存 | NOT RUN | | 这里只能观察“无候选窗、不吞键”；“未发 Snapshot/PredictAI”要看日志，等 Core 接入后补 |
| 11 | 杀宿主后重开，输入正常（10.1、10.2） | ST-037（宿主部分） | 直通可用≤2s；无丢字/重复字符 | NOT RUN | | |
| 12 | 杀 Core/Worker/UI 后输入 100 字符并计时（10.3） | ST-037 | 直通可用≤2s；无丢字/重复字符 | NOT RUN | | 需要 P3，当前无故障注入点 |
| 13 | 按键清单（8）逐项 | 设计书 DD §5.3 / UT-009（按键规则）；ST-006 的候选选择和取消 | 见第 8 步表格 | NOT RUN | | 需要 M3 |
| 14 | 候选窗位置、100/150/200 % DPI、双显示器（9） | 兼容性分级，不属于上面的用例 | 光标处出现、不抢焦点、大小合适 | NOT RUN | | |
| 15 | 商店应用、开始菜单搜索的可用性（7） | 兼容性分级 | 记录 可用 / 受限 / 不支持 及原因 | NOT RUN | | 仍**不支持**：UI 元素协议已实现，但 `IMMERSIVESUPPORT` 仍有意不注册（13.11 列出了缺项）；补齐缺项之后再测 |
| 16 | UI 元素 Begin/End 配对，密码框里没有 begin（9.1 的 1、3） | 设计推断，不属于上面的用例 | 每个 `begin id=N` 恰好一个 `end id=N`；常规宿主 `show=1` 且候选窗与第 9 步一致 | NOT RUN | | 需要 M3 接入；证据是 DebugView 日志 |
| 17 | 宿主回答 FALSE 时自绘窗不出现，`Show(TRUE/FALSE)` 切换无残影（9.1 的 4、5） | 设计推断，不属于上面的用例 | begin 到 end 之间没有自绘窗；切换后没有残影 | NOT RUN | | 没有这样的测试宿主：不要用商店应用代替 |

### 12.3 这次测试要回答的问题（实测填写）

| 问题 | 为什么重要 | 实测答案 |
|---|---|---|
| `bEnabledByDefault = TRUE` 时，键盘会自动出现在已添加的语言下，还是要手动“添加键盘”？ | 决定安装器要不要再提示用户 | |
| 第二次 `regsvr32 /u` 是否仍显示成功？ | 代码假定“已不存在”的 Win32 码（文件/路径未找到）被容忍；TSF 对不存在项的真实返回码没有验证 | |
| 卸载后 `HKLM\SOFTWARE\Microsoft\CTF\TIP\{CLSID}` 下有无残留空键？ | 决定是否还要额外清理 | |
| 卸载后 HKCU 里有无残留，设置里是否出现“幽灵”键盘？ | DLL 不碰 HKCU 的用户状态 | |
| 图标显示成什么？ | 目前没有图标资源 | |
| 商店应用、开始菜单搜索里本输入法是否出现，被拒绝时的原因（签名 / `ITfTextInputProcessorEx` / 其它）？ | 决定下一步要补什么 | |
| 各桌面宿主对 `BeginUIElement` 回答 `show` 是 1 还是 0？`ITfUIElementMgr` 在每个宿主里都可用吗？ | 宿主自己画的路径（`show=0`）有没有真实使用者；没有管理器时是否确实退回自绘窗 | |
| 宿主调用元素的 `GetDocumentMgr`、`GetString`、`GetPageIndex` 时有没有拿到合理的答案，有没有 `E_FAIL` / `E_INVALIDARG`？ | 元素的答案（只描述当前页）宿主是否接受 | |
| 逐用户注册（`Scope::User`）是否可行？ | M5 要先确认；当前没有入口：`regsvr32` 只会走机器级注册，要先在 `lib.rs` 增加 `DllInstall` 导出才能试 | 不在本手册范围 |

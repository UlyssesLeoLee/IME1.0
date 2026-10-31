# Windows 安装器（P5 / 里程碑 M5）

> 状态：**预发布，未签名，未在真实 Windows / 虚拟机上运行过。** 安装器的逻辑（计划、事务、回滚、升级、卸载）由内存中的假后端
> 做了完整的单元测试，真实文件系统路径在临时目录里做了集成测试；但**真实的注册表写入、`regsvr32` 注册、UAC 提权、`MoveFileEx`
> 重启时删除、`code` 命令**都没有在任何真实系统上执行过（本机规则禁止）。它安装的文本服务 DLL（`crates/pime-tsf`）本身也**尚未在虚拟机里验证**
> （见 `RUST_MIGRATION_PLAN.md` 13.7，M3/M4 未完成），所以这个安装包只能作为预发布版本在**可丢弃的测试机**上试用。
> 对应用例：ST-035（新装/修复/升级/卸载、失败可回滚）、IT-039（升级故障注入）、UT-002/UT-061（只登记本产品，不改默认输入法）、FR-048。
>
> 一次独立评审（只读代码）提出了 6 项问题（重启时删除的路径、环境变量、重解析点、日志文件、重装清单丢失、DLL 搜索路径与注销回退）。
> 修复、仍未解决的风险、以及**在虚拟机里必须确认的事项**见**第 14 节**。这些修复同样只经过假后端、临时目录和子进程测试，**没有在真实系统上运行过**。

## 1. 形态与理由

决策（`RUST_MIGRATION_PLAN.md` 第 10 节第 4 项）：**Rust 自包含安装器**，不用 Inno Setup / WiX / NSIS（本机一个都没有，也不允许安装）。

- `crates/pime-setup`：独立 crate（自己的 `[workspace]` 与 `Cargo.lock`，`windows` 钉 `=0.58.0`，原因同 `VERSIONS.md`），目标 `x86_64-pc-windows-gnu`，
  只导入系统 DLL（实测：`advapi32, api-ms-win-core-synch-l1-2-0, kernel32, msvcrt, ntdll, shell32, user32`）。依赖只有 `windows` 与 `pime-core`
  （仅为复用已审计的 SHA-256 和严格 JSON 解析器，安装器不执行核心的任何功能）。
- **载荷（payload）用「附加在 exe 末尾的 overlay」携带**，不用 `include_bytes!`：Rust 桩程序只编译一次、没有载荷也能构建和跑测试；
  `scripts/build-installer.js` 把文件追加到桩程序后面得到最终安装包，不需要为每个载荷重新编译。Windows 加载器忽略最后一个节之后的字节。
  签名兼容：代码签名（Authenticode）会把证书表追加在 overlay 之后，安装器按 PE 头里的安全目录定位载荷末尾并容忍 ≤ 7 字节的对齐填充（有单元测试，尚未用真实签名验证）。
- 控制台子系统：从终端运行时 shell 会等待它，`--silent` 才有可用的退出码、`--dry-run` 才能在终端打印；双击（交互模式）时程序立刻 `FreeConsole()`，只剩消息框。

## 2. 它做什么 / 不做什么

安装（默认选项，顺序即执行顺序，`--dry-run` 会逐条打印）：

| # | 步骤 | 说明 |
|---|---|---|
| 0 | 校验载荷 | 整个载荷和每个文件的 SHA-256；不通过则什么都不做 |
| 1 | 创建安装目录 | 默认 `%ProgramFiles%\Programming IME`；记录**真正新建**的目录（含缺失的上级），预先存在的目录不记录、卸载时不删 |
| 2 | 写 `install-manifest.json` | **先于其它文件写入**（预写）：安装被强杀时仍可卸载 |
| 3 | 复制 `pime_tsf.dll`、`pime-core.exe`、`LICENSE`、`programming-ime.vsix` | 已存在且内容相同则不重写 |
| 4 | 复制自身为 `uninstall.exe` | 卸载程序 |
| 5 | 注册文本服务 | `<System32>\regsvr32.exe /s "<dll>"`（64 位版本；`<System32>` 来自 `GetSystemDirectoryW`，不读环境变量）。`--no-register` 跳过 |
| 6 | 写「添加/删除程序」项 | `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\ProgrammingIME`（`UninstallString`、`QuietUninstallString`、`EstimatedSize` 等）。`--no-arp` 跳过 |
| 7 | 安装 VS Code 扩展 | 仅当找得到 `code`：`code --install-extension "<vsix>" --force`；**失败不回滚**，只在结果里警告。`--no-vscode` 跳过 |

**不做**：不设置默认输入法、不激活 profile、不读写其它输入法的任何注册（计划里没有这种步骤类型，且有测试检查；`regsvr32` 调用的是 DLL 自己的
`DllRegisterServer`，它写什么由 `pime-tsf` 的注册模块决定，受 `install_plan.rs`/UT-002/UT-061 约束）；不联网；不修改 VS Code 设置
（尤其**不会**设置 `programmingIme.engine` / `programmingIme.coreExecutable`：要用 Rust 引擎需自行把 `coreExecutable` 指向 `<安装目录>\pime-core.exe`）；
不写 `Program Files` 以外的位置（除你给的 `--prefix`）；不创建开始菜单/桌面快捷方式；没有单独的「修复」模式（重新运行同一个安装包即可，幂等）；不提供 32 位版本。

## 3. 命令行

```
pime-setup.exe / ProgrammingIME-<版本>-win-x64-setup.exe [选项]
  （无选项）        交互安装：几个 MessageBox，没有向导
  --silent, /S      无提示
  --prefix <目录>   安装目录（默认 <64 位 Program Files>\Programming IME，文件夹由 SHGetKnownFolderPath 决定，不看环境变量；路径含空格要加引号；--prefix=<目录> 也可）
  --dry-run         只打印计划（并校验载荷），不改变任何东西，不提权，不写日志
  --no-register     不注册文本服务
  --no-arp          不写「添加/删除程序」项
  --no-vscode       不安装 VS Code 扩展（.vsix 仍会复制到安装目录，之后可手动安装）
  --uninstall       卸载（读取安装目录里的 install-manifest.json）
  --help
```

退出码：`0` 成功；`1` 失败（安装已回滚）；`2` 参数错误或安装目录被拒绝；`1223` 用户取消（回答「否」或拒绝 UAC）；`3010` 成功但需要注销/重启才能完成。

内部选项（提权子进程用，不要手工使用）：`--silent --prefix <目录> --vscode-deferred --log <文件>`。`--log` 指向的文件必须**已经存在**、是普通文件、只有一个名字、
内容为空（由未提权的父进程创建，见第 4 节）；否则子进程在做任何事之前就以退出码 1 拒绝。

`--dry-run` 实例（节选，本机实测，什么都没有改变）：

```
Programming IME 0.2.0 - install plan (dry run: nothing is changed)
  folder: C:\Program Files\Programming IME
  administrator rights needed: yes (the setup restarts itself elevated)
  payload: 4 files, version 0.2.0, SHA-256 of every file verified
  steps:
     1. create folder C:\Program Files\Programming IME
     ...
     8. register the text service: regsvr32.exe /s "C:\Program Files\Programming IME\pime_tsf.dll"
     9. add the Add/Remove Programs entry HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\ProgrammingIME
    10. install the VS Code extension if `code` is found (non-fatal): code --install-extension "...\programming-ime.vsix" --force
```

## 4. 提权与 VS Code 步骤的用户上下文

计划写 HKLM 或受保护目录（Program Files、Windows）且当前进程未提权时，安装器用 `ShellExecuteExW` 的 `runas` 重启自己（一次 UAC 提示，子进程隐藏、
静默、报告写到一个临时文件并回显）。临时目录里的 `--no-register --no-arp --no-vscode` 安装**不需要**管理员权限，也不会提权（有测试）。

**报告文件**（评审问题 4）：由**未提权的父进程**在用户 `%TEMP%` 里创建，名字是 128 位随机数（`pime-setup-<32 位十六进制>.log`），`CREATE_NEW`，
并且父进程在子进程运行期间一直以**不共享删除**的方式持有句柄（别的程序不能删除、改名它或在原名处放链接）；结束后父进程读出内容并删除。
提权子进程（`--log`）只**打开**它：不创建、不跟随重解析点，并检查它是普通文件（不是目录/链接）、只有一个名字（不是硬链接）、内容为空；
任何一条不满足就在做任何事之前以退出码 1 拒绝。`std::env::temp_dir()` 本身仍来自 `TMP`/`TEMP` 环境变量，但它只决定父进程在哪里创建文件，提权子进程从不依据环境变量决定路径。
**残余的固有风险**：安装包未签名且会自我提权——父进程让 Windows 以管理员身份重新运行 `current_exe()` 在磁盘上的路径，如果该路径在用户可写的位置（下载文件夹），
同一用户的恶意程序可以在 UAC 提示前后把它换掉。这是任何未签名、自我提权的安装包都有的问题，只有代码签名加上从受保护位置运行才能缓解；UAC 提示里会显示「未知发布者」。

`code --install-extension` 会装进**当前用户的** profile；如果由提权后的（可能是另一个管理员账户的）子进程运行就会装错人。因此提权子进程只把扩展记入清单，
**由未提权的父进程在子进程成功后运行 `code`**。从已提权的 shell 里直接运行时则全部在同一进程内完成。卸载时同理。

## 5. 事务、回滚、升级

- 每个会改变状态的步骤**先**把逆操作压入撤销栈；任何一步失败就逆序撤销，机器回到安装前的状态（单元测试对「安装的每一个后端操作」逐个注入故障：
  全新安装 10 个操作、升级的每个操作，断言回滚后文件、目录、注册表、注册状态与安装前**完全相等**）。
- 已存在的文件**不原地覆盖**：新内容写到 `<名>.pime-new`，旧文件改名为 `<名>.pime-old`，再把新文件改名到位；回滚时改名回去。
  Windows 允许改名**被进程加载的 DLL**，但不允许覆盖或删除它，所以升级一个已被应用加载的 `pime_tsf.dll` 是可行的。
  全部成功后才删除 `.pime-old`；删不掉（仍被占用）就安排重启时删除，退出码 `3010`，并提示注销/重启（已加载旧 DLL 的程序在重启前仍用旧版本）。
  **重启时删除的是文件名，不是文件**（评审问题 1）：`MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` 在开机时删除当时位于该路径的任何文件，所以对一个仍在用的文件
  （正在运行的 `uninstall.exe`、被其它程序加载的 `pime_tsf.dll`、不再随新版本发行的旧文件、`.pime-old`）**从不对原路径调用**：先把它改名为同目录下的
  `<名>.pime-del-<进程号>-<序号>`（Windows 允许改名被映射的映像；`RealBackend::rename` 用不带 `REPLACE_EXISTING` 的 `MoveFileExW`，不会覆盖一个仍在等重启的旧文件），
  再只对**这个新名字**安排删除。改名失败就**不安排任何删除**，在结果里警告并写明该文件仍在原处，请自行删除。安排删除本身失败（没有管理员权限）：文件留在新名字下，警告里写出新名字。
  文件夹仍用自己的路径安排：Windows 只删除到那时已为空的文件夹，所以重启前重新安装（文件夹里有新文件）不会被删。该调用写 HKLM 的挂起重命名列表，需要管理员。
- **安装前检查**（评审问题 1，保守）：需要管理员权限的安装在第 0 步读取 `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager` 的 `PendingFileRenameOperations`
  和 `PendingFileRenameOperations2`（REG_MULTI_SZ，源/目标成对，去掉 `\??\` 与 `!`；只读），列表里有**安装目录之内**的路径就拒绝安装并要求先重启
  （例外：本安装器自己的 `.pime-del-<数字>-<数字>` 名字——任何安装都不会写这种名字——以及安装目录本身和它的上级文件夹）。
  不需要管理员权限的安装（临时目录、三个 `--no-*`）**不读 HKLM**（只有提权进程才能排队这种条目，也让集成测试完全不碰 HKLM）；读取失败只在日志里提示并继续。
- 内容相同的文件不重写；重新安装同一版本不改任何文件（有测试：写入日志为空）。旧版本清单里有、新版本不再提供的文件，成功后才删除，并且只删除
  安装目录内的（清单被篡改指向别处的条目不会被删，有测试）。
- 升级回滚时，如果旧版本已登记过文本服务，会在旧 DLL 恢复**之后**重新 `regsvr32 /s` 旧 DLL，而不是取消登记。
- 已有另一个位置的安装占用着「添加/删除程序」项时，拒绝安装（避免遗留孤儿文件和登记）。
- 本段写于 `pime-tsf` 的 `DllRegisterServer` 返回 `E_NOTIMPL` 的时候：当时带注册的真实安装会在第 5 步失败并**完整回滚**（这条路径有测试覆盖）。
  现在 `pime-tsf` 已有注册实现（`register.rs`），但整条「安装器 → regsvr32 → DLL 注册」路径仍**没有在真实系统上跑过**；`--no-register` 可以只验证文件部分。

## 6. 卸载

`uninstall.exe --uninstall`（「添加/删除程序」调用的命令）读取**自己所在目录**的 `install-manifest.json`；也可 `--prefix <目录>`。只删除清单列出的内容：

1. （`code` 存在时）`code --uninstall-extension <id>`，失败只警告；
2. `regsvr32 /u /s "<dll>"`；**失败不阻止卸载，只警告**（否则 DLL 没有实现注销时产品永远无法卸载）。DLL 文件已经不在、或被第 7 节的链接检查拒绝时 `regsvr32 /u` **不会运行**：
   这时结果里有一条警告，写明「文本服务**没有**被注销，注册可能还在注册表里」和手工清理办法（见 6.1）；
3. 删除「添加/删除程序」项；
4. 逐个删除文件（先 DLL 等，最后 `uninstall.exe`，最后清单）；被占用的文件先改名再安排重启时删除（第 5 节），并提示注销/重启；
5. 从深到浅删除**安装器创建过**的目录，仅当为空；含有用户自己的文件时保留并警告。从不递归删除。

每一步删除、改名、`regsvr32` 之前都要过链接检查（第 7 节）；不过关的目标被跳过并报告，**从不跟随**。安装目录不在 Program Files 之下时，卸载（`--dry-run`、日志、确认框）会警告：
任何能写那里的用户都可能换掉里面的文件，而卸载以管理员身份对那里的 DLL 运行 `regsvr32 /u`、删除那里的文件。

**限制**：从安装目录里的 `uninstall.exe` 运行时，它自己正在运行，无法删除自己，因此它会被改名后在**下次重启**时删除（退出码 3010），安装目录本身也在重启时（为空时）删除。
从别处（例如下载的安装包 `--uninstall --prefix ...`）运行则可一次删净。

**清单不被信任**（它是以管理员身份执行的输入）：卸载计划拒绝任何不在安装目录内的文件、含 `..` 的路径、DLL 不在安装目录内、不是本产品「添加/删除程序」键的注册表键、
不是「安装目录或其上级」的目录条目、清单里记录的目录与清单实际所在目录不一致；`regsvr32` 的路径永远是 `GetSystemDirectoryW` 给出的 `<System32>\regsvr32.exe`，不读清单、不读环境变量。每条规则都有测试。

### 6.1 手工清理（DLL 已不在，或 `regsvr32 /u` 没有运行）

文本服务的注册由 **DLL 自己**的 `DllUnregisterServer` 负责（`crates/pime-tsf/src/register.rs`）；安装器**不复制**这套删除逻辑。DLL 在时按顺序做：

1. **推荐**：用**同一版本**的安装包再安装一次（它会把 DLL 放回原处并再次注册；有了评审问题 11 的修复，清单也会记住这个注册），然后再卸载一次；这时 `regsvr32 /u` 由 DLL 自己完成 TSF 那一半。
2. 做不到时，以管理员身份在**提权的**命令提示符里删除 COM 注册（CLSID 与 `crates/pime-tsf/src/lib.rs` 的 `CLSID_PIME` 一致，有测试核对）：

```
reg delete "HKLM\Software\Classes\CLSID\{43F95ED1-95B4-421D-8F5A-837128C2D854}" /f /reg:64
```

3. **最后手段**：Windows 的 `msctf` 为这个文本服务在 `CTF\TIP` 下写的输入法条目（`pime-tsf` 文档说明这个布局是 msctf 私有的、观察得到而非文档化的）：

```
reg delete "HKLM\SOFTWARE\Microsoft\CTF\TIP\{43F95ED1-95B4-421D-8F5A-837128C2D854}" /f /reg:64
```

`HKCU\Software\Microsoft\CTF\Assemblies` 与 `HKCU\Control Panel\International\User Profile` 是 Windows 为用户建立并拥有的，`pime-tsf` 不写它们，卸载后残留的条目应被 Windows 忽略（待虚拟机确认）。
这些命令**没有**在真实系统上运行过。

## 7. 安装目录的安全检查

拒绝（退出码 2，什么都不做）：空、相对路径、UNC、`\\?\`、含 `..`/`.`/空组件、末尾为点或空格、保留设备名（CON、NUL…）、8.3 短名（`PROGRA~1`）、非法字符；
盘符根目录、Windows 目录及其下、`C:\Program Files`、`C:\Program Files (x86)`、`C:\ProgramData`、`C:\Users`、用户 profile 根本身等系统根。
含空格和非 ASCII（如 `D:\日本語 フォルダ\Programming IME`）的路径正常工作，缺失的上级目录会创建并记录。
注册文本服务时若目录在 Program Files 之外，计划会给出警告（任何能写该目录的用户都能替换被 Windows 加载进其它程序的 DLL）。

**系统文件夹不读环境变量**（评审问题 2）：`SystemPaths` 的 Windows 目录、`System32`、Program Files（64 位/x86）、ProgramData、用户 profile 由
`GetSystemWindowsDirectoryW`、`GetSystemDirectoryW`、`SHGetKnownFolderPath` 取得（`real::system_paths()`）；回答不是本地盘符上的绝对路径、或 `System32` 不在 Windows 目录之内就报错退出（退出码 1），
**没有**回退到猜测。这些值决定提权后运行哪个 `regsvr32.exe`，也决定哪些文件夹算受保护。`SystemPaths` 仍是普通结构，测试可注入。
测试用子进程（带伪造的 `SystemRoot`、`ProgramFiles`、`ProgramData`、`USERPROFILE` 等）确认答案不变；这也确认了 `SHGetKnownFolderPath` 不受这些变量影响。
仍读环境变量的地方：`PATH`/`LOCALAPPDATA`（只用于找 VS Code 的 `code.cmd`，运行它的是未提权的父进程；从已提权 shell 里直接运行时是该用户自己的环境）和 `TMP`/`TEMP`（只决定父进程在哪里建报告文件）。

**重解析点（junction、符号链接）不被跟随**（评审问题 3）：安装目录可以在普通用户可写的位置，那里的 junction 会让管理员身份的删除/`regsvr32 /u` 作用到别处。
每次**写入、删除、改名、`regsvr32`** 之前，`Backend::check_plain(根, 路径)` 从「根」逐级向下（卸载时根是安装器创建的最高一级文件夹，安装时是安装目录）用
`CreateFileW(FILE_FLAG_OPEN_REPARSE_POINT)` 打开每一级，**不跟随**任何东西：遇到重解析点（`FILE_ATTRIBUTE_REPARSE_POINT`，任何种类）就拒绝；
并用 `GetFinalPathNameByHandleW` 核对每一级解析到的位置就是「根的规范路径 + 相对部分」（不区分大小写）。网络位置（`\\?\UNC\…`）和卷 GUID 形式的最终路径被拒绝。
不合格的目标被**跳过并报告**（卸载）或使该步失败并回滚（安装）；安装目录本身若已是 junction，安装在第 1 步就被拒绝，什么都没写。
已知的限制：① 检查和随后的按路径操作之间有竞态（检查后、操作前被换成链接）。更强的做法是用同一个句柄做删除（`FileDispositionInfo`）并在整个 `regsvr32` 期间不带 `FILE_SHARE_DELETE` 地持有 DLL，**未做**；
所以对非默认（用户可写）安装目录的主要保护仍是警告。② 「根」的**上级**文件夹里的链接不被发现（根按它解析后的位置算规范路径）；`subst` 盘符不会因此被拒绝，映射的网络盘会被拒绝。
③ 任何种类的重解析点都被拒绝，包括 OneDrive 占位符和 WOF 压缩文件：这样的安装目录会让卸载留下文件并警告，是安全的失败方式。④ 硬链接不检查（删除硬链接只删这个名字）。
⑤ 用户可写目录里的 DLL 本来就能被换成别的 DLL（不需要任何链接），`regsvr32 /u` 会以管理员身份加载它——这是非默认安装目录的固有风险，靠警告而不是靠这个检查。

## 8. 载荷格式与完整性

```
blob   := u32 个数 | { u16 名长 | 名(UTF-8) | u64 数据长 | sha256(数据)[32] | 数据 }*
footer := "PIME-PAYLOAD-V1\0"[16] | u64 blob 长 | sha256(blob)[32]            （小端）
文件   := PE 桩 | blob | footer [| 填充 | Authenticode 证书表]
```

条目名只允许普通文件名（`pime_tsf.dll`、`pime-core.exe`、`LICENSE`、`programming-ime.vsix`、`meta.json`），其它一律拒绝。`meta.json` 带版本和扩展 id。
**哈希只能发现损坏和截断，不能证明来源**：能改安装包的人也能改哈希。来源证明需要代码签名（未做）。下载后请用同目录的 `.sha256` 文件核对整个安装包：
`Get-FileHash .\ProgrammingIME-0.2.0-win-x64-setup.exe -Algorithm SHA256`。被截断的安装包会报「carries no payload」。

## 9. VS Code 扩展包（.vsix）

`scripts/package-vsix.js` 用 `npx --yes @vscode/vsce@4.0.0` 打包（4.0.0 是 2026-10-30 查到的、发布已满两周的最新稳定版；4.1.0 发布于 2026-10-28，故不用。
首次运行需要访问 npm 源；失败时报清楚的错误并退出 1）。

扩展在运行时从**扩展根目录下的 `language-packs/`** 读取规则包（`src/core/languages.js`），而仓库里它在 `01_Source/language-packs`。直接打包扩展目录得到的 `.vsix`
会找不到规则包，所以脚本把扩展需要的文件（`package.json`、`package.nls*.json`、`LICENSE`、`.vscodeignore`、`src/`、有则 `media/`、以及 `language-packs/`）复制到
`dist/vsix-staging` 再打包，不改动源码树。`scripts/`、`.vscode/` 不进包。`src/` 里没有其它越出扩展根的路径（`require` 全是相对且在包内；
`languages.js` 的第三个候选 `../../../../language-packs` 是开发布局的回退，在包内第二个候选先命中）。

打包后自动验证：列出全部文件；条目名安全；必需文件齐全（含 `LICENSE.txt`、3 个 `package.nls*.json`、7 个规则包）；无 `scripts/`、`.vscode/`；
`package.json` 与 `extension.vsixmanifest` 的版本；用自带的 zip 读取器逐个解压（校验 CRC）并对每个 JS 做 `node --check`；静态检查每个相对 `require` 与
`__dirname` 路径都解析在包内（已知的开发回退单独列为已知项）；最后**功能验证**：在临时目录里解压，去掉 `PIME_LANGUAGE_PACKS`，`loadRegistry()` 必须从包内加载 7 个规则包，
并有反向对照（去掉包内 `language-packs/` 后必须失败），证明检查有效。

**目前扩展目录没有 `media/`**（P6 的产物）：脚本在有时才包含并提示，不伪造。`package.json` 没有 `repository` 字段，打包使用 `--allow-missing-repository`。
卸载安装器不会卸载 `--no-vscode` 时未装的扩展；手动卸载：`code --uninstall-extension pime-local-prototype.programming-ime-vscode-bridge`。

## 10. 构建命令（精确）

```powershell
cd 01_Source
$env:CARGO_TARGET_DIR = '<一个空闲目录>'      # 可选；默认 <temp>\pime-cargo-target
node scripts/package-vsix.js                 # 只打包并验证 .vsix -> dist\programming-ime-vscode-bridge-<版本>.vsix
node scripts/build-installer.js              # 全部：pime-core + pime-tsf + .vsix + 桩 + 载荷 -> dist\ProgrammingIME-<版本>-win-x64-setup.exe + .sha256
node scripts/build-installer.js 0.3.0-pre1   # 指定安装器版本（默认取 editor-bridges/vscode/package.json 的版本）
```

`build-installer.js` 依次：`cargo build --release --target x86_64-pc-windows-gnu`（`pime-core`、`crates/pime-tsf`、`crates/pime-setup`）、`package-vsix.js`、追加载荷，
然后检查结果：x64 PE、控制台子系统、只导入系统 DLL（`scripts/pe-info.js`）、用独立的读取器校验载荷、运行 `--dry-run`（只打印计划，不创建任何东西）证明安装包能找到并校验自己的载荷。
**从不运行安装器本体**。构建输出 `dist/` 已在 `.gitignore`。

crate 自身的检查（在 `crates/pime-setup` 下）：`cargo fmt`、`cargo clippy --all-targets -- -D warnings`、`cargo test`（72 个单元测试 + 12 个 `real_fs` 集成测试 + 5 个 `real_links` 集成测试，
不需要载荷；评审修复前是 50 + 5）。`tests/run-all.js --setup` 只是在这个 crate 里运行 `cargo test`。

2026-10-30 本机一次实测：安装包 2,978,899 字节（桩 505,344 + `pime_tsf.dll` 239,616 + `pime-core.exe` 2,126,963 + `.vsix` 105,452 + 其它），
sha256 `a3dcd330013497c6fa12db8ac634fd1518f420d3446221ed0141fe1df2b9c79d`（仅作记录；构建不保证逐位可复现）。

## 11. 测试

- 单元（`src/*.rs` 内，假后端 `FakeBackend`，带故障注入、被占用文件、重启删除模拟）：精确计划；各 `--no-*` 开关；计划里没有任何非本产品的步骤；
  系统根与 `..` 被拒绝、空格/非 ASCII 路径；清单往返与篡改；每个操作故障注入的回滚（全新安装与升级）；幂等重装；升级（含被占用 DLL、陈旧文件）；
  regsvr32 失败；VS Code 非致命；卸载（只删清单内容、保留用户文件和其它产品、被占用文件重启删除、注销失败不阻断）；载荷格式（位翻转、截断、签名表填充、非法名）；
  命令行；三种语言的消息占位符一致；命令行引用规则；`code.cmd` 查找。
- 集成（`01_Source/tests/setup/real_fs.rs`、`real_links.rs`，共用 `common.rs`，由 `crates/pime-setup/Cargo.toml` 的 `[[test]]` 引用；**真实文件系统，仅临时目录，仅 `--no-register --no-arp --no-vscode`**，
  提权的测试进程会自动跳过）：安装/升级/卸载、含空格和日文的路径、`--dry-run` 不创建任何东西、被拒绝的安装目录退出码 2、没有载荷的桩给出明确错误、真实 exe 的安装与卸载。
  评审修复新增：真实的改名不覆盖已有文件；被映射的 DLL（一份 System32 DLL 的副本，真的加载进测试进程）被改名到旁边、原路径从不被安排删除；带伪造环境变量的子进程得到同样的系统文件夹；
  父进程的随机报告文件、子进程对链接/目录/硬链接/非空文件的每一种拒绝、真实 exe 对不存在的 `--log` 不创建、对硬链接不追加；`SetDefaultDllDirectories` 在子进程里真的让
  「当前目录里的 DLL」不再按裸名被找到（有加载前的对照）；`real_links.rs` 里真实 junction 的各种情形和 Windows 自带的链接（`C:\Documents and Settings`）。
  **junction 与符号链接**：开发机的终端防护阻止测试进程创建它们（`FSCTL_SET_REPARSE_POINT` 与 `CreateSymbolicLink` 返回「拒绝访问」，并先卡住约 1 分钟），这些情形打印 `SKIPPED` 后退出，
  所以开发机上真实 junction 的创建与跟随**没有**被测到；Windows 自带的链接（只读属性，不需要权限）和 fake 后端里四个位置的链接测试是那里的证据。
  `real_links` 是单独的测试程序，正是为了让这种卡顿不影响启动安装包 exe 的那些测试。
  另外：开发机的终端防护有时会删除刚链接出来的测试 exe（`os error 5` / 文件消失，与内容无关，改一行源码重新链接即可），这不是测试失败。
- **没有运行**：真实注册表（HKLM）、`regsvr32` 注册、UAC 提权与父子进程报告回传、真实的 `MoveFileEx(DELAY_UNTIL_REBOOT)` 与重启后的效果、`PendingFileRenameOperations` 的真实内容、`code` 命令、交互模式的消息框、签名后的载荷定位、
  在 Windows 11 虚拟机里的完整安装/升级/卸载（ST-035 的真实部分）。这些需要一台可丢弃的 Windows 11 虚拟机（清单见第 14 节）。

## 12. 已知限制与未做

- **未签名**：SmartScreen 会警告「未知发布者」，企业策略可能直接拦截。签名流程（`signtool`）未做，需要证书。
- **预发布**：被安装的文本服务 DLL 尚未经虚拟机验证；今天的 DLL 不处理按键（见 13.7 的限制）。
- 只有 x64；32 位应用看不到该输入法。
- 请从**本地磁盘**运行安装包：提权后的进程看不到非提权会话映射的网络驱动器。
- 报告文件只在提权子进程场景下存在（父进程在 `%TEMP%` 里以随机名创建、回显后删除，见第 4 节）；没有常驻安装日志。
- 「添加/删除程序」项没有图标和安装日期；没有「修改/修复」按钮（`NoModify`/`NoRepair`）。
- 消息框文案有 zh-CN / en-US / ja-JP 三种（按用户界面语言），控制台和日志只有英文。

## 13. 给集成者

- 根 `01_Source/Cargo.toml` 需要 `exclude = ["crates/pime-tsf", "crates/pime-setup"]`（本工作项不改该文件；`pime-setup` 自带 `[workspace]`，不加 exclude 时从 `01_Source` 运行
  `cargo test --workspace` 也不受影响，因为它不是成员；加上更规范）。
- 集成测试已经在 `01_Source/tests/setup/`（由 `crates/pime-setup/Cargo.toml` 的 `[[test]] path` 引用），`tests/run-all.js --setup` 在 `crates/pime-setup` 里运行 `cargo test`。
- `build-installer.js` 会构建 `crates/pime-tsf`：随着 `DllRegisterServer` 等模块合并，安装包会自动带上新的 DLL。
- 评审修复给 `windows` 依赖加了 feature：`Win32_System_Com`（`CoTaskMemFree`）、`Win32_System_IO`（只有测试用 `DeviceIoControl`）、`Win32_System_LibraryLoader`、
  `Win32_System_SystemInformation`；新增的导入都是 `build-installer.js` 允许的系统 DLL（`ole32.dll`）。

## 14. 独立评审的修复（2026-10-31，分支 `wi/setup-review`）

评审只读了代码，没有运行任何东西；下面每一项先对照代码确认了说法，再修复，每个修复都有「修复前失败、修复后通过」的测试（用还原/变异证明过）。
**所有修复都没有在真实系统上运行过**：没有真实安装/卸载、没有 `regsvr32`、没有写注册表（只有一个只读的 `PendingFileRenameOperations` 查询，而且只在需要管理员权限的安装里）、没有 UAC。

| # | 问题 | 结论与修复 | 仍然开放或未验证 |
|---|---|---|---|
| 1 | 对仍在用的文件的**原路径**安排重启时删除；重启前重装，开机时新文件被删 | 属实。改名为 `<名>.pime-del-<pid>-<n>` 后只对新名字安排；改名失败则不安排并警告；文件夹仍用自己的路径（只删空文件夹）；需管理员的安装前检查 `PendingFileRenameOperations(2)`（第 5 节）。`RealBackend::rename` 不再覆盖已有文件 | 真实的 `MoveFileEx(DELAY_UNTIL_REBOOT)`、开机时的执行顺序、`PendingFileRenameOperations2` 在 Win11 上是否出现、重启后残留的 `.pime-del-*`（应已消失）、一次「卸载 → 不重启 → 重装 → 重启」的完整序列 |
| 2 | `SystemRoot`/`ProgramFiles` 来自环境变量，决定提权后的 `regsvr32` 路径 | 属实（推导部分）。改用 `GetSystemWindowsDirectoryW`/`GetSystemDirectoryW`/`SHGetKnownFolderPath`，失败即报错、无回退；子进程测试确认伪造的环境变量不起作用（第 7 节） | 环境变量是否真会传到提权子进程（评审标为「可能」）未验证；`ShellExecute runas` 换账户时 `FOLDERID_Profile` 是管理员账户的 |
| 3 | 用户可写安装目录里的 junction 让管理员删除/`regsvr32 /u` 作用到别处 | 属实。写入/删除/改名/`regsvr32` 前的 `check_plain`（不跟随任何重解析点 + 最终路径核对）；卸载在非 Program Files 目录时警告；安装目录本身是 junction 就拒绝安装（第 7 节） | 检查与操作之间的竞态（未做句柄式删除）；根的上级链接不被发现；OneDrive/WOF 占位符被保守地拒绝；开发机上真实 junction/符号链接的创建被终端防护阻止，那些测试在那里没有运行（Windows 自带的 `C:\Documents and Settings` 与 fake 测试是证据）；非默认目录里 DLL 被直接替换的风险依然存在（警告） |
| 4 | 提权子进程向可预测的 `%TEMP%\pime-setup-<pid>.log` 追加（跟随链接） | 属实。父进程用随机名 `CREATE_NEW` 创建并以不共享删除的方式持有；子进程只打开、不跟随、检查普通文件/单名字/为空，否则拒绝（第 4 节） | 固有风险：未签名、自我提权的安装包按磁盘路径重新运行 `current_exe()`，同一用户的恶意程序可在 UAC 前后换掉它。真实的 UAC 子进程路径没有运行；文件符号链接的拒绝只在有权限的机器上测到（开发机上被拒绝创建） |
| 11 | 用 `--no-register`/`--no-arp` 重新运行后，清单丢掉先前的登记 | 属实。把旧清单的 `text_service_dlls`、`registry_keys`、`vscode_extension`（和目录）并入新清单，只并入通过 `validate` 检查的条目；测试：安装 → 三个开关重跑 → 卸载，DLL 被注销、键与扩展被删、机器回到安装前 | 一次**被中途杀掉的升级**仍可能让旧版本的 `files` 条目不在清单里（旧文件成为孤儿）；清单中的 `files` 没有做并集，以保持「清单只列真实存在的文件」 |
| 12a | DLL 已不在时跳过 `regsvr32 /u`，登记留在 HKLM | 属实（原来只有一句含糊的警告）。现在警告明确说「没有注销」并给出确切的手工清理命令（6.1）；链接检查拒绝 DLL 时给同样的警告。CLSID 与 `pime-tsf` 的 `CLSID_PIME` 由测试核对。**没有**在安装器里复制 `pime-tsf` 的删除逻辑 | 6.1 里的 `reg delete` 命令没有在真实系统上运行过；`CTF\TIP` 键的布局是 msctf 私有的 |
| 12b | `main` 里没有 `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` | 属实。`main` 第一件事调用；子进程测试证明「当前目录里的 DLL」按裸名不再被找到（有加载前的对照）。安装器自己不从应用目录加载任何 DLL | 静态导入由加载器在 `main` 之前解析，这个调用管不到：release 构建的导入是 `advapi32`、`api-ms-win-core-synch`、`bcryptprimitives`、`kernel32`、`msvcrt`、`ntdll`、`ole32`、`oleaut32`、`shell32`、`user32`，其中只有 **`bcryptprimitives.dll` 不是已知 DLL**（来自工具链的标准库）：与 exe 同目录里的同名 DLL 会在 `main` 之前被加载。要堵上需要链接器的 DependentLoadFlags 之类的选项（MinGW 的 ld 没有），本工作项没有做；默认安装目录（Program Files）不受影响，下载文件夹里的安装包和非默认目录里的 `uninstall.exe` 受影响 |

### 14.1 在虚拟机里必须确认的事（一台可丢弃的 Windows 11，安装包来自 `build-installer.js`）

1. **重启时删除**：带一个已被应用加载的 `pime_tsf.dll`（先用该输入法在记事本里输入）做升级和卸载；确认 `.pime-del-*` 文件出现、`3010`、**注销/重启后消失**；确认 `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager` 的 `PendingFileRenameOperations` 里只有 `.pime-del-*` 名字和文件夹（没有 `uninstall.exe`、`pime_tsf.dll` 的原路径）；
   再做「卸载（`uninstall.exe` 自己运行）→ 不重启 → 重新安装 → 重启」：新装的 `uninstall.exe`、`install-manifest.json`、`pime_tsf.dll` 仍在。再确认安装前检查：在有安装目录内的挂起条目时安装被拒绝并提示重启。
2. **Win11 的 `PendingFileRenameOperations2`** 是否真的出现，内容格式是否与解析一致。
3. **提权路径**：UAC 子进程用的报告文件（`%TEMP%` 下的随机名）能被提权子进程打开（所有者/ACL；换成另一个管理员账户时也要试），父进程能回显内容并删除文件；`ShellExecute runas` 换账户时 `SHGetKnownFolderPath` 的结果合理。
4. **链接**：在用户可写的非默认安装目录里放 junction、符号链接，确认卸载跳过并警告，目标不被动；这台开发机上这些测试被终端防护挡住了。
5. **手工清理命令**（6.1）：先删掉 `pime_tsf.dll` 再卸载，确认警告原文，再按 6.1 清理，确认输入法列表里不再出现该输入法、重新安装可以成功。
6. **DLL 搜索**：从下载文件夹运行安装包，旁边放一个同名的 `bcryptprimitives.dll`（只在 VM 里）验证上面说的残余风险是否属实。
7. **环境变量**：用一个把 `SystemRoot`/`ProgramFiles` 改成别处的未提权 shell 启动安装包，确认提权后的 `regsvr32` 仍然是 `C:\Windows\System32\regsvr32.exe`。

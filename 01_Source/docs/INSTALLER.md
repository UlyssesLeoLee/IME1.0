# Windows 安装器（P5 / 里程碑 M5）

> 状态：**预发布，未签名，未在真实 Windows / 虚拟机上运行过。** 安装器的逻辑（计划、事务、回滚、升级、卸载）由内存中的假后端
> 做了完整的单元测试，真实文件系统路径在临时目录里做了集成测试；但**真实的注册表写入、`regsvr32` 注册、UAC 提权、`MoveFileEx`
> 重启时删除、`code` 命令**都没有在任何真实系统上执行过（本机规则禁止）。它安装的文本服务 DLL（`crates/pime-tsf`）本身也**尚未在虚拟机里验证**
> （见 `RUST_MIGRATION_PLAN.md` 13.7，M3/M4 未完成），所以这个安装包只能作为预发布版本在**可丢弃的测试机**上试用。
> 对应用例：ST-035（新装/修复/升级/卸载、失败可回滚）、IT-039（升级故障注入）、UT-002/UT-061（只登记本产品，不改默认输入法）、FR-048。

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
| 5 | 注册文本服务 | `%SystemRoot%\System32\regsvr32.exe /s "<dll>"`（64 位版本）。`--no-register` 跳过 |
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
  --prefix <目录>   安装目录（默认 %ProgramFiles%\Programming IME；路径含空格要加引号；--prefix=<目录> 也可）
  --dry-run         只打印计划（并校验载荷），不改变任何东西，不提权，不写日志
  --no-register     不注册文本服务
  --no-arp          不写「添加/删除程序」项
  --no-vscode       不安装 VS Code 扩展（.vsix 仍会复制到安装目录，之后可手动安装）
  --uninstall       卸载（读取安装目录里的 install-manifest.json）
  --help
```

退出码：`0` 成功；`1` 失败（安装已回滚）；`2` 参数错误或安装目录被拒绝；`1223` 用户取消（回答「否」或拒绝 UAC）；`3010` 成功但需要注销/重启才能完成。

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
静默、日志写到用户 `%TEMP%` 的临时文件并回显）。临时目录里的 `--no-register --no-arp --no-vscode` 安装**不需要**管理员权限，也不会提权（有测试）。

`code --install-extension` 会装进**当前用户的** profile；如果由提权后的（可能是另一个管理员账户的）子进程运行就会装错人。因此提权子进程只把扩展记入清单，
**由未提权的父进程在子进程成功后运行 `code`**。从已提权的 shell 里直接运行时则全部在同一进程内完成。卸载时同理。

## 5. 事务、回滚、升级

- 每个会改变状态的步骤**先**把逆操作压入撤销栈；任何一步失败就逆序撤销，机器回到安装前的状态（单元测试对「安装的每一个后端操作」逐个注入故障：
  全新安装 10 个操作、升级的每个操作，断言回滚后文件、目录、注册表、注册状态与安装前**完全相等**）。
- 已存在的文件**不原地覆盖**：新内容写到 `<名>.pime-new`，旧文件改名为 `<名>.pime-old`，再把新文件改名到位；回滚时改名回去。
  Windows 允许改名**被进程加载的 DLL**，但不允许覆盖或删除它，所以升级一个已被应用加载的 `pime_tsf.dll` 是可行的。
  全部成功后才删除 `.pime-old`；删不掉（仍被占用）就用 `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` 安排重启时删除，退出码 `3010`，并提示注销/重启
  （已加载旧 DLL 的程序在重启前仍用旧版本）。该调用写 HKLM 的挂起重命名列表，需要管理员；没有权限时只是警告。
- 内容相同的文件不重写；重新安装同一版本不改任何文件（有测试：写入日志为空）。旧版本清单里有、新版本不再提供的文件，成功后才删除，并且只删除
  安装目录内的（清单被篡改指向别处的条目不会被删，有测试）。
- 升级回滚时，如果旧版本已登记过文本服务，会在旧 DLL 恢复**之后**重新 `regsvr32 /s` 旧 DLL，而不是取消登记。
- 已有另一个位置的安装占用着「添加/删除程序」项时，拒绝安装（避免遗留孤儿文件和登记）。
- 当前状态下，`pime-tsf` 的 `DllRegisterServer` 返回 `E_NOTIMPL`（由另一个工作项实现）。在它合并之前，带注册的真实安装会在第 5 步失败并**完整回滚**
  （这正是被测试覆盖的路径）；`--no-register` 可以只验证文件部分。

## 6. 卸载

`uninstall.exe --uninstall`（「添加/删除程序」调用的命令）读取**自己所在目录**的 `install-manifest.json`；也可 `--prefix <目录>`。只删除清单列出的内容：

1. （`code` 存在时）`code --uninstall-extension <id>`，失败只警告；
2. `regsvr32 /u /s "<dll>"`；**失败不阻止卸载，只警告**（否则 DLL 没有实现注销时产品永远无法卸载）；
3. 删除「添加/删除程序」项；
4. 逐个删除文件（先 DLL 等，最后 `uninstall.exe`，最后清单）；被占用的文件用重启时删除安排，并提示注销/重启；
5. 从深到浅删除**安装器创建过**的目录，仅当为空；含有用户自己的文件时保留并警告。从不递归删除。

**限制**：从安装目录里的 `uninstall.exe` 运行时，它自己正在运行，无法删除自己，因此 `uninstall.exe` 和安装目录会在**下次重启**时才被删除
（退出码 3010）。从别处（例如下载的安装包 `--uninstall --prefix ...`）运行则可一次删净。

**清单不被信任**（它是以管理员身份执行的输入）：卸载计划拒绝任何不在安装目录内的文件、含 `..` 的路径、DLL 不在安装目录内、不是本产品「添加/删除程序」键的注册表键、
不是「安装目录或其上级」的目录条目、清单里记录的目录与清单实际所在目录不一致；`regsvr32` 的路径永远由 `%SystemRoot%` 推导，不读清单。每条规则都有测试。

## 7. 安装目录的安全检查

拒绝（退出码 2，什么都不做）：空、相对路径、UNC、`\\?\`、含 `..`/`.`/空组件、末尾为点或空格、保留设备名（CON、NUL…）、8.3 短名（`PROGRA~1`）、非法字符；
盘符根目录、Windows 目录及其下、`C:\Program Files`、`C:\Program Files (x86)`、`C:\ProgramData`、`C:\Users`、用户 profile 根本身等系统根。
含空格和非 ASCII（如 `D:\日本語 フォルダ\Programming IME`）的路径正常工作，缺失的上级目录会创建并记录。
注册文本服务时若目录在 Program Files 之外，计划会给出警告（任何能写该目录的用户都能替换被 Windows 加载进其它程序的 DLL）。

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

crate 自身的检查（在 `crates/pime-setup` 下）：`cargo fmt`、`cargo clippy --all-targets -- -D warnings`、`cargo test`（50 个单元测试 + 5 个集成测试，不需要载荷）。

2026-10-30 本机一次实测：安装包 2,978,899 字节（桩 505,344 + `pime_tsf.dll` 239,616 + `pime-core.exe` 2,126,963 + `.vsix` 105,452 + 其它），
sha256 `a3dcd330013497c6fa12db8ac634fd1518f420d3446221ed0141fe1df2b9c79d`（仅作记录；构建不保证逐位可复现）。

## 11. 测试

- 单元（`src/*.rs` 内，假后端 `FakeBackend`，带故障注入、被占用文件、重启删除模拟）：精确计划；各 `--no-*` 开关；计划里没有任何非本产品的步骤；
  系统根与 `..` 被拒绝、空格/非 ASCII 路径；清单往返与篡改；每个操作故障注入的回滚（全新安装与升级）；幂等重装；升级（含被占用 DLL、陈旧文件）；
  regsvr32 失败；VS Code 非致命；卸载（只删清单内容、保留用户文件和其它产品、被占用文件重启删除、注销失败不阻断）；载荷格式（位翻转、截断、签名表填充、非法名）；
  命令行；三种语言的消息占位符一致；命令行引用规则；`code.cmd` 查找。
- 集成（`crates/pime-setup/tests/real_fs.rs`，**真实文件系统，仅临时目录，仅 `--no-register --no-arp --no-vscode`**，提权的测试进程会自动跳过）：
  安装/升级/卸载、含空格和日文的路径、`--dry-run` 不创建任何东西、被拒绝的安装目录退出码 2、没有载荷的桩给出明确错误、真实 exe 的安装与卸载。
  按总规则这些本应放在 `01_Source/tests/` 下；本工作项的文件范围只含 `crates/pime-setup/**`，故暂放在 crate 内，集成时可挪动（见第 13 节）。
- **没有运行**：真实注册表（HKLM）、`regsvr32` 注册、UAC 提权与父子进程日志回传、`MoveFileEx` 重启删除、`code` 命令、交互模式的消息框、签名后的载荷定位、
  在 Windows 11 虚拟机里的完整安装/升级/卸载（ST-035 的真实部分）。这些需要一台可丢弃的 Windows 11 虚拟机。

## 12. 已知限制与未做

- **未签名**：SmartScreen 会警告「未知发布者」，企业策略可能直接拦截。签名流程（`signtool`）未做，需要证书。
- **预发布**：被安装的文本服务 DLL 尚未经虚拟机验证；今天的 DLL 不处理按键（见 13.7 的限制）。
- 只有 x64；32 位应用看不到该输入法。
- 请从**本地磁盘**运行安装包：提权后的进程看不到非提权会话映射的网络驱动器。
- 日志只在提权子进程场景下写文件（`%TEMP%\pime-setup-<pid>.log`，回显后删除）；没有常驻安装日志。
- 「添加/删除程序」项没有图标和安装日期；没有「修改/修复」按钮（`NoModify`/`NoRepair`）。
- 消息框文案有 zh-CN / en-US / ja-JP 三种（按用户界面语言），控制台和日志只有英文。

## 13. 给集成者

- 根 `01_Source/Cargo.toml` 需要 `exclude = ["crates/pime-tsf", "crates/pime-setup"]`（本工作项不改该文件；`pime-setup` 自带 `[workspace]`，不加 exclude 时从 `01_Source` 运行
  `cargo test --workspace` 也不受影响，因为它不是成员；加上更规范）。
- 可把 `tests/real_fs.rs` 迁到 `01_Source/tests/` 并在 `crates/pime-setup/Cargo.toml` 里用 `[[test]] path` 引用；也可在 `tests/run-all.js` 增加一步
  `cargo test`（在 `crates/pime-setup`）和 `node scripts/package-vsix.js`。
- `build-installer.js` 会构建 `crates/pime-tsf`：随着 `DllRegisterServer` 等模块合并，安装包会自动带上新的 DLL。

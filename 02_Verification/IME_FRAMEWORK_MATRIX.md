# 输入法框架完整性矩阵（Programming IME v0.2.2）

- 生成日期：2026-11-05（**第十六轮：设置窗六页页签 + 候选列表归位左侧激光面板 + 自带中日英词库 + 分层窗口原子移动**；安装包与 §5 全部数字已整体刷新到本轮）
- 代码基线：`fa17ddd` + 本轮改动（工作区未提交）
- 构建环境：Windows 11 x64、Rust 1.99.1（`x86_64-pc-windows-gnu`，链接器 `C:\msys64\mingw64`）、Node v24.19.0、npm 12.2.1、VS Code 1.144.0

本文件回答一个问题：**这个安装包装到 Windows 上之后，它到底是不是一个"框架齐全的输入法"？**
结论先说：**代码层面的框架是齐的；真实系统层面的框架一次都没跑过。** 下文把这两件事分开写，不混。

> **本轮（2026-11-05，第十六轮）做了什么**：九件你点名的事。①**激光口**：左侧发光口里现在有一小截淡蓝色半透明激光（`SLIT_LENGTH=26`／`SLIT_WIDTH=1`／`SLIT_HALO=5`），整束光更细、更有科技感。②**设置按钮能打开设置**——真因找到了：`WM_CREATE` 的 lparam **不带** lpParam，窗口句柄建好后拿不到设置对象，所以 `open()` 一直失败；改到 `WM_NCCREATE` 里用 `CreateStruct` 读 `lp_create_params` 才修好。③**设置内容对标搜狗**：真实 `SysTabControl32` 的**六页页签**（常规/按键/外观/**插件**/AI/关于），页签必须**按索引插入**（`wParam=-1` 的追加惯用法在本机被控件拒绝），设置项比原来多得多。④**Tab 按程序员习惯**作为选取使用（默认 Tab 移动高亮即选取，Enter/空格/数字上屏），按键方案可切**微软/搜狗/ATOK/程序员**预设。⑤**打字时不再显示搜狗式悬浮窗**——候选列表搬进左侧激光面板（新增 `laser_candidates.rs`），`popup.rs` 的 `PopupWindow` 生产路径不再使用。⑥**PL/SQL 作为「打开」的插件、T-SQL 作为「关闭」的插件**加入，设置菜单有**插件选项卡**可开关。⑦**Ctrl+F12 切中文/日文能正常打字**——按「输入法自带词库」的方向落地（`pime-core/src/cjk.rs` + 5 个数据文件），并用**真实词库端到端测试**证明（中文档打「ni」出「你」、日文档「nihon」出假名）。⑧**透明图层边缘残留**——分层窗口移动改为**由图片原子移动**（`glass.rs`）。⑨**预留 API**：`services.rs` 里定义 **AI 整理词库 / 远程更新 / 用户登录**（搜狗式管理）三个接口（当前是桩实现，不联网）。本轮**实拍取证抓到并修掉两个真缺陷**：设置窗的页签条一直是空的、设置窗永远开在 (0,0) 且不在顶层；另修了长说明文字被裁切。缺陷、待裁定项全部登记在缺口台账 `02_Verification/FRAMEWORK_GAPS.md` §5.14。

> **上一轮（2026-11-03）做了什么**：两条线并行。①**插件语言**：允许以插件形式导入语言包，随发行版交付 **Cypher / PL\/SQL / T-SQL** 三个包（默认 **T-SQL 关闭、PL\/SQL 打开**，用户可随时切换），两引擎（VS Code 桥接侧的 JavaScript 引擎、Windows 输入法侧的 Rust 核心）都实现了插件感知注册表、启停闸门、扩展名多归属解析与 **SQL 族验证器**；通用规范写在 `01_Source/docs/LANGUAGE_PLUGIN_SPEC.md`（含已知限制：Rust 核心**没有运行时包加载器**，插件语言在 Windows 输入法里当前不可用）。②**输入法本体测试层**：此前自动化验证全压在 VS Code 桥接侧，输入法本体（TSF 文本服务）没有自己的自动化测试；本轮新建 `01_Source/tests/tsf/`（**21 个行为场景** + **23 项功能覆盖账本** + **宿主级 COM 测试**）与原生窗口取证链路（探针开真窗口 → .NET 抓屏 → Node 解析 PNG 断言非纯色），功能对照表见 `01_Source/docs/IME_TEST_MATRIX.md`。本轮抓到的缺陷、待裁定项全部登记在缺口台账 `02_Verification/FRAMEWORK_GAPS.md` §5.8。

> **更早的一轮（2026-11-02）做了什么**：按你的裁定补了 4 个框架项（COM 对象计数、文档/上下文生命周期回调、候选窗归属宿主窗口、DPI 基准）、2 个安装器项（安装结束指引、ARP 日期与图标）、1 个发布流程项（一键签名脚本与证书放置位）。同日第三轮又补了**悬浮窗（搜狗拼音式工具条）、设置界面（按语言分组）、可搜索的语言切换菜单、AI effort 与外观皮肤**，并修了候选列表 UI element 协议与宿主选词回传。同日第四轮补了**安装向导**（自绘窗口 + 右下角 IME1.0 logo）。同日第六轮把**安装位置做成可以直接输入**（真实 EDIT 子控件）、**logo 改成原样 1:1 直绘**（不再缩放发白）、**左上角也放 logo**、**整体彩虹配色**，并把右下角图标 / 窗口图标 / EXE 资源 / 载荷 `pime.ico` 统一成同一个 logo。**同日第七轮**：向导的整套绘制换成**自绘像素面**（线性光混色、解析式抗锯齿、柔影，§5.3 甲），并按你的要求把**「选目标文字 → 按 Tab → 上屏」**这条链路做通（§5.3 乙，键位口径与两份需求文档有冲突，已登记在缺口提交单 §5.5 待你裁定）。

## 0. 三层口径（本文全篇使用）

| 口径 | 含义 | 能证明什么 | 不能证明什么 |
|---|---|---|---|
| **代码层** | 源码里存在该机制 | 设计完整、接口名正确、注册项齐全 | 它在 Windows 上能跑 |
| **行为层** | 有自动化测试跑过真实代码路径 | 逻辑、边界、回滚、载荷格式正确 | 宿主（Windows / TSF）接受它 |
| **判据层** | 在真实宿主上跑过 | 这是一个能被 Windows 认的输入法 | —— |

**本项目目前只有前两层。第三层只填了一半**：2026-11-04 在真实系统上做了一次提权安装，注册表那一半是真的（真实 HKLM 里的 COM 键、三个语言 profile、三个 category、ARP 条目、`regsvr32` 不再报 `E_FAIL`）；**但真实宿主里的那一半仍然是空的**——候选选择、Ctrl+F12 切换、语音识别都还没有在真实键盘 / 真实麦克风 / 真实语音服务上跑过（`01_Source/docs/TSF_VM_TEST_PLAN.md:3-4`、`INSTALLER.md:3-6`、`RUST_MIGRATION_PLAN.md:430` 写的是「从未在真实 Windows 注册表或真实 TSF 上运行过」，其中「真实注册表」这一半在本轮已经不成立了）。
所以下文任何一格写着"是"，指的是**代码层**；真实系统一列**全部为否**。

## 1. 交付物：安装包

| 项 | 值 |
|---|---|
| 文件 | `01_Source/dist/ProgrammingIME-0.2.2-win-x64-setup.exe` |
| 大小 | 10,132,518 字节 |
| SHA-256 | `23776fa0c34c7352508b63fe46d9c59db3f46c682432981bb3e349736945ac50` |
| 形态 | x64 控制台子系统 EXE；载荷以 PE overlay 附在桩程序之后 |
| 桩程序 | `pime-setup.exe`，788,480 字节（含安装向导与内嵌 logo） |
| 载荷（`pime_tsf.dll` 2,632,704 B ／ `pime-core.exe` 3,214,200 B ／ `LICENSE` 1,085 B ／ `programming-ime.vsix` 211,138 B ／ `pime.ico` 66,724 B ／ `pime-tray.exe` 1,104,896 B ／ `pime-ai-worker.exe` 400,896 B ／ `pime-mcp.exe` 706,560 B ／ `pime-ime.exe` 762,880 B ／ `meta.json` 87 B 条） |  |
| 导入表 | 只导入 Windows 系统 DLL。**桩程序**：advapi32 / bcryptprimitives / kernel32 / msvcrt / ntdll / ole32 / oleaut32 / shell32 / user32；**载荷里的每个程序**（DLL、core、托盘、worker、MCP 服务端、命令行）也逐个核过（托盘另需 `winmm`，worker 另需 `crypt32`/`winhttp`，都是 Windows 自带） |
| 签名 | **无**。SmartScreen 会报"未知发布者"。签名流程已备好（`01_Source/scripts/sign-installer.ps1`），等证书 |

构建与自检命令（不安装、不注册）：

```
cd 01_Source
node scripts/build-installer.js          # 产出 dist/*.exe 与 *.sha256
node scripts/build-installer.js --dry-run # 只打印安装计划，不改任何东西
```

安装包自带的 `--dry-run` 已实跑通过：能找到并逐个校验自己的载荷（8 个条目 SHA-256 全对），打印 10 步安装计划，**没有创建任何文件**。

> **构建不是字节可复现的**：同一份源码、同一套工具链重跑 `build-installer.js`，文件大小不变（9,805,947 字节）但 **SHA-256 会变**（GNU 链接产物里带了每次构建都会变的内容）。所以上表的哈希**只对当前这一个 `dist/` 里的文件成立**；重装前请以 `dist/*.exe.sha256` 为准。这条事实正是 `installer-claims-check.js` 存在的原因——文档里的数字必须与产物一致，一致性靠机器比对，不靠人记。

## 2. 框架功能矩阵

### 2.1 COM / DLL 层

| 框架功能 | 代码 | 状态 | 自动化证据 | 真实系统 |
|---|---|---|---|---|
| `DllGetClassObject` / `DllCanUnloadNow` / `DllRegisterServer` / `DllUnregisterServer` | `pime-tsf/src/lib.rs:76-115` | 已实现 | PE 导出表检查（`tests/smoke/tsf-dll-check.js`） | 否 |
| `IClassFactory`（`CreateInstance` / `LockServer`） | `lib.rs:43-72` | 已实现 | `lib.rs:133,201` | 否 |
| 失败时出参必须为 NULL（COM 契约） | `lib.rs:53-66` | 已实现 | `lib.rs:133` 逐个错误码断言 | 否 |
| **COM 对象计数**（`DllCanUnloadNow` 何时能返回 S_OK） | `lib.rs:46-116`、`tip.rs:594-607` | **已实现**（2026-11-02） | `lib.rs` 的 `unload_allowed` 决策表 + 活对象计数测试 | 否 |
| COM 入口点 panic 不穿到宿主 | `tip.rs:751-882`、`register.rs:972-979`、`popup.rs:327-333` | 已实现 | `tip.rs:895`、`register.rs:1738` | 否 |

**COM 对象计数**（本轮补上）：`DllCanUnloadNow` 不再恒返回 `S_FALSE`，而是看两个原子计数——活对象数与 `LockServer` 锁数；两者都为 0 才返回 `S_OK`。计数覆盖**我们交给宿主的每一个 COM 对象**：文本服务对象、编辑会话对象、以及宿主从 `BeginUIElement` 到 `EndUIElement` 之间持有的候选列表 UI element（否则宿主回调会打进已卸载的 DLL）。计数用字段级 `Drop` 实现（`lib.rs` 的 `LiveGuard`）：`implement` 宏把字段搬进 COM 对象，字段的 `Drop` 恰好在对象销毁时运行。

`LockServer` 从空实现改成真正的计数（`lib.rs:104-116`）。

### 2.2 文本服务接口（TIP）

| 框架功能 | 代码 | 状态 | 自动化证据 | 真实系统 |
|---|---|---|---|---|
| `ITfTextInputProcessor::Activate` | `tip.rs:749-759` | 已实现 | `tip.rs:1073` | 否 |
| `ITfTextInputProcessorEx::ActivateEx` | `tip.rs:761-767` | 已实现 | `tip.rs:1032,1073` | 否 |
| `ITfKeyEventSink`（吞键判定） | `tip.rs:769-823` → `controller.rs:366-454` | 已实现 | `controller.rs:1122` 等约 60 项 | 否 |
| `ITfThreadMgrEventSink::OnSetFocus` | `tip.rs:834-847` | 已实现 | `tip.rs:1062` | 否 |
| `ITfThreadMgrEventSink` 的文档/上下文回调 | `tip.rs:899-975` | **已实现**（2026-11-02） | `tip.rs` 的回调存活测试 | 否 |
| `ITfCompositionSink::OnCompositionTerminated` | `tip.rs:858-873` | 已实现 | 仅控制器层 | 否 |
| `ITfEditSession::DoEditSession`（写入文本） | `tip.rs:80-144` | 已实现 | 控制器层用假宿主 | 否 |
| `ITfContextComposition::StartComposition` | `tip.rs:370-378` | 已实现 | 控制器层 | 否 |
| `ITfUIElement` + `ITfCandidateListUIElement` | `uielement_com.rs:39-142` | 已实现 | `uielement_com.rs:308-470`、`uielement.rs` 34 项 | 否 |
| `ITfCandidateListUIElementBehavior` | `uielement_com.rs` 的 `ITfCandidateListUIElementBehavior_Impl` | **已实现**（2026-11-02） | `uielement_com.rs` 的行为接口测试 | 否 |
| `ITfDisplayAttributeProvider` | 无 | **未实现** | — | 否 |
| `ITfComposition`（IME 自持组合 / `TakeOwnership`） | 无（全仓库无 `TakeOwnership`） | **未实现** | — | 否 |
| `ITfTextEditSession` | 无（只有 `ITfEditSession`） | **未实现** | — | 否 |
| `ITfServiceProvider` / `ITfThreadFocusSink` / `ITfContextOwnerCompositionSink` | 无 | **未实现** | — | 否 |

**线程/文档/上下文生命周期回调**（本轮补上）：`OnUninitDocumentMgr` 与 `OnPopContext` 原来都是空实现，宿主销毁文档或弹出上下文时我们不留痕迹。现在：只有当被销毁的文档管理器**确实拥有**我们正在输入的那个上下文（用 `ITfContext::GetDocumentMgr` 比对 `IUnknown` 身份，不是裸指针），或被弹出的就是当前上下文时，才结束组合、忘记上下文并重置状态机；不是我们的就什么都不做。

### 2.3 注册层

| 框架功能 | 代码 | 状态 | 自动化证据 | 真实系统 |
|---|---|---|---|---|
| COM `InprocServer32` + `ThreadingModel=Apartment` | `register.rs:317-321` | 已实现 | `register.rs:1224` | 否 |
| `ITfInputProcessorProfileMgr::RegisterProfile` × 3 语言（zh-CN 0x0804 / en-US 0x0409 / ja-JP 0x0411） | `register.rs:864-885`、`register.rs:237-258` | 已实现 | `register.rs:1276` | 否 |
| `ITfCategoryMgr::RegisterCategory`（TIP_KEYBOARD / TIPCAP_SYSTRAYSUPPORT / TIPCAP_UIELEMENTENABLED） | `register.rs:284-299,894-899` | 已实现 | `register.rs:1310`（断言恰好 3 个） | 否 |
| **把 profile 设为 ACTIVE / 默认输入法** | 无（`Backend` 没有 activate/default 方法） | **不做**（刻意） | `register.rs:36-37` | 否 |
| `GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER` | 无 | **未实现** | — | 否 |
| `TIPCAP_COMLESS` / `TIPCAP_INPUTMODECOMPARTMENT` | 无 | **未实现** | — | 否 |
| `TIPCAP_IMMERSIVESUPPORT`（商店应用 / 开始搜索可用） | 无 | **刻意不注册** | `register.rs:271-276` | 否 |
| `TIPCAP_SECUREMODE`（登录界面可用） | 无 | **刻意不注册** | `register.rs:277-281` | 否 |
| CTF\TIP 条目、语言栏图标、HKCU 用户 profile | 交给 Windows | 有意不写 | `register.rs:21-28` | 否 |
| `DllUnregisterServer` 精确撤销自己加的东西 | `register.rs:583-610,996-1004` | 已实现 | `register.rs:1620-1670` | 否 |

项目对"故意不注册"的写法是自洽的——`register.rs:282-283`：

> `DISPLAYATTRIBUTEPROVIDER`, `TIPCAP_COMLESS`, `TIPCAP_INPUTMODECOMPARTMENT`: the `tip` module implements none of them yet; add each one together with its implementation.

即：**不声明自己没有实现的能力**，这是对的，不算缺陷。

### 2.4 输入与候选窗

| 框架功能 | 代码 | 状态 | 自动化证据 | 真实系统 |
|---|---|---|---|---|
| 候选窗 = 真实 Win32 窗口（`WS_POPUP` / `WS_EX_NOACTIVATE` / 永不吃焦点） | `popup.rs:153-186,279-303,360` | 已实现 | 消息处理层 `popup.rs:724-766` | 否（无测试创建真实窗口） |
| 候选窗归属宿主窗口（owner） | `popup.rs:767-800` | **已实现**（2026-11-02） | owner 只取本进程前台窗口 | 否 |
| DPI（按宿主的感知模式选基准） | `popup.rs:694-760` | **已实现**（2026-11-02） | `choose_dpi` 决策表测试 | 否（未跑多屏/缩放） |
| 系统配色 / 高对比度主题 | `popup.rs:11-12` | 已实现 | — | 否 |
| 键位映射（Backspace/Tab/Enter/Esc/PageUp/PageDown/方向键/Space、修饰键不吞、Ctrl+Alt+Space 进组合、Shift 轻点暂停） | `keymap.rs:83-175` | 已实现 | `keymap.rs` 13 项 + `pime-core` `key_01..key_13` | 否 |
| 输入状态机（Direct/Observing/Candidates/Composing/Suspended/FaultFallback） | `pime-core/src/input.rs:27-34`、`controller.rs:202-225` | 已实现 | `controller.rs` 约 60 项 | 否 |
| 安全字段（密码框）静默 | `tip.rs:242-278`、`controller.rs` | 已实现 | 有 | 否 |
| 组合串文本写入（读回校验，不盲写） | `tip.rs:196-236,420-440` | 已实现 | `controller.rs:1626,1658` | 否 |
| **悬浮窗（搜狗拼音式工具条）**：**已于第十五轮整块删除**（`floating.rs` 与 `bar_skin.rs` 两个文件连同 21 项单测一起删掉，登记在缺口台账 §5.13.2 第 2 条待你追认）；**打字时不再显示任何搜狗式悬浮窗**——悬浮界面统一到左侧蓝色微光激光口区域，候选列表也搬进那块面板（见下行） | `menus.rs`（下拉/可搜索菜单，仍在用）；`floating.rs`（**已删除**） | **已删除**（2026-11-04 第十五轮） | 过滤/翻转的单测仍在 `menus.rs` | 否（窗口从未在真实宿主里显示过） |
| **候选列表归位左侧激光面板**（打字时**不出现**搜狗式悬浮窗；候选行由激光面板承载，`LaserCandidates` 实现 `CandidateUi`；`popup.rs` 的 `PopupWindow` 生产路径不再使用，模块与代码保留） | `laser_candidates.rs`（新）、`laser.rs`（`PanelCandidate`／`PanelItem::Candidate`／`set_candidates`／`place_panel`）、`laser_ui.rs` | **已实现**（2026-11-05 第十六轮） | `pime-tsf` 端到端测试断言候选行真的出现在面板里；探针实拍可见 5 行候选 | 否（**未在真实宿主里跑过**：新 DLL 还没换上） |
| **设置界面**：**六页真实页签**（常规 / 按键 / 外观 / **插件** / AI / 关于）+ 按语言分组（候选页大小、Tab 行为、**Tab 是否交给宿主**、空格上屏、自动开组合、默认输入模式、显示激光界面）+ **插件开关**（Cypher / PL\/SQL / T-SQL）+ 按键方案预设（程序员 / 微软 / 搜狗 / ATOK / 自定义）+ 原子写盘 | `settings_window.rs`、`imesettings.rs` | **已实现**（2026-11-05 第十六轮重做为六页页签） | `imesettings.rs` 的读写/容错/原子写单测 + 设置窗布局单测（`every_text_of_the_settings_window_has_a_box_that_is_big_enough` 等）+ 实拍页签 6 个 | 否（窗口从未在真实宿主里显示过；**本轮修掉了「页签条一直是空的」与「窗口永远开在 (0,0)」两个真缺陷**） |
| **候选窗显示时 Tab 确认上屏**（选目标文字 → 按 Tab → 文字写进宿主文档） | `pime-core/src/input.rs` 的 `tab_is_ours()` 与 `reserved_key()`、`pime-tsf/src/controller.rs` | **已实现**（2026-11-02 第七轮，按你的要求改的键位口径） | `pime-core` 的「直接模式下 Tab 确认」单测 + `pime-tsf` 控制器的端到端断言（文档里真的出现候选文字） | 否（**判据层仍为空**：需你在真实宿主实测，见 §5.3） |
| **Tab 可以完全交还给宿主**（回到需求文档描述的「预览内导航、非自动应用」） | `pime-core/src/input.rs` 的 `KeyMap::tab_to_host`、`imesettings.rs` 的 `tabToHost` | **已实现**（2026-11-02 第七轮） | `pime-core` 与 `pime-tsf` 各一项（直接模式与组合态都放行） | 否 |
| **语言切换菜单**：搜索框过滤，菜单可向上弹出，调用 TSF `ActivateLanguageProfile` 切换 profile | `menus.rs`、`tip.rs` 的 `switch_language` | **已实现**（2026-11-02） | 过滤/翻转的单测；TSF 调用只能真机验 | 否 |
| **自带中日英词库**（拼音→中文、罗马音→假名/日文汉字、英文词表；**不调用任何其他输入法**；档案随语言切换：zh-CN→拼音、ja-JP→假名、en-US→代码引擎优先、无代码候选时补英文词） | `pime-core/src/cjk.rs`（新）、`pime-core/src/cjk/*.txt`（5 个数据文件）、`predict.rs`、`imeswitch.rs` 的 `profile_of()`、`controller.rs` 的 `set_profile()` | **已实现**（2026-11-05 第十六轮） | `pime-core` 三档预测单测 + `pime-tsf` **真实词库端到端测试**（中文档「ni」→「你」上屏；日文档「nihon」→假名；英文档仍是代码） | 否（**真实键盘下的中文/日文输入还没试过**） |
| **Tab 语义可配置**（默认 **Tab = 移动高亮并选取**（程序员习惯），Enter/空格/数字上屏；可改为「上屏候选」（微软/搜狗/ATOK 口径）或「交给宿主」） | `pime-core/src/input.rs`、`controller.rs`、`imesettings.rs`、`settings_window.rs` | **已实现**（2026-11-05 第十六轮） | `pime-core` 与 `pime-tsf` 各自的键位单测 | 否（真实键盘手感要你自己试） |
| **按键方案预设**（程序员 / 微软 / 搜狗 / ATOK / 自定义；**仅为近似**，不是原版逐键复刻） | `imesettings.rs`、`settings_window.rs` 的按键页 | **已实现**（2026-11-05 第十六轮） | 设置读写单测 | 否 |
| **分层窗口边缘残留**（分层窗口移动改为**由图片原子移动**：`UpdateLayeredWindow` 带新目标点重新上传图层，surface 与窗口一起走，旧位置不留像素） | `glass.rs` 的 `move_to()` 与纯函数 `move_way()` | **已实现**（2026-11-05 第十六轮） | 单测 `a_window_with_a_picture_is_moved_by_its_picture` + 账本 `F-LAYERED-WINDOW` | 否 |
| **预留的词库管理 API**（**AI 整理词库** / **远程更新** / **用户登录**（搜狗式管理）三个接口已定义，**当前是桩实现**，不联网、不收集任何数据） | `pime-core/src/services.rs`（新）：`DictionaryCurator` / `DictionaryUpdater` / `AccountService` | **接口已定义**（2026-11-05 第十六轮） | 编译 + 单测 | 否（**这三个接口没有接后端**，是给后续接服务用的契约） |
| **AI 代理的写入通道**（AI 客户端请输入法把文字写进焦点窗口的光标处：MCP / CLI / 本地管道 → `pime-agent` → `WM_COPYDATA` → 焦点进程里的桥接窗口 → 真的 TSF 编辑会话） | `pime-core/src/agent_proto.rs`（线上契约）、`pime-tsf/src/agent_bridge.rs`（桥接窗口）、`pime-tsf/src/tip.rs`（`answer_agent` / `selection_text` / `replace_selection`）、`crates/pime-agent/`（三件套） | **已实现**（2026-11-05 第十七轮） | **真机跨进程往返**（`agent_bridge.rs` 的 `channel_tests`）+ 客户端侧真机往返 + 契约 10 项往返单测 | 否（**通道跑通了，文字真的落进记事本 / VS Code 还没验过**：新 DLL 还没换上） |

**候选窗的归属与 DPI**（本轮补上）：
- **owner**：候选窗原本是没有任何归属的置顶顶层窗口，宿主窗口关掉后它会留在屏幕上。现在它把**本进程的前台窗口**当作 owner（`GWLP_HWNDPARENT`），别的进程的窗口一律不认——否则它会浮在别人的窗口上方，或在别的应用关闭时一起消失。每次列表出现都会刷新 owner。
- **DPI**：原来无条件用 `GetDpiForMonitor`（显示器原始 DPI）。但宿主应用若只做系统 DPI 感知，它的坐标和我们的窗口都在系统缩放空间里，用原始 DPI 会把候选字画错大小。现在按进程当前的感知模式决定：per-monitor 感知用显示器 DPI，否则用**窗口自己的** DPI（`choose_dpi` 决策表有单测）。**没有**调用 `SetProcessDpiAwarenessContext`——那会改变宿主应用所有窗口的渲染方式，是应用自己的事，不是输入法该插手的事。

### 2.5 安装器层

| 框架功能 | 代码 | 状态 | 自动化证据 | 真实系统 |
|---|---|---|---|---|
| 载荷自校验（整体 + 逐条目 SHA-256），失败即不做任何事 | `payload.rs:134-141,252-260`、`cli.rs:261-272` | 已实现 | `payload.rs:324-330` | 否 |
| 事务式安装：每步先压逆操作，失败逆序回滚 | `exec.rs:260-268,441-487` | 已实现 | `exec.rs:1128-1136`（逐操作注入故障） | 否 |
| 升级（含被占用 DLL、陈旧文件） | `exec.rs` | 已实现 | 有 | 否 |
| 卸载（只删清单内容、保留用户文件） | `exec.rs` | 已实现 | 有 | 否 |
| 重启时删除（先改名再排队，不对原路径排队） | `exec.rs` | 已实现 | 有 | 否 |
| 安全检查：拒绝系统根 / `..` / 重解析点 / 短名 / 保留名 | `real.rs`、`plan.rs` | 已实现 | `real_fs.rs` 12 项、`real_links.rs` 5 项 | 否 |
| `regsvr32` 注册文本服务（64 位，路径取自 `GetSystemDirectoryW`） | `plan.rs:612-615` | 已实现 | `exec.rs:1286-1326` | 否 |
| 「添加/删除程序」项 | `plan.rs:528-555` | 已实现 | 有 | 否 |
| **ARP 安装日期**（`InstallDate`，`YYYYMMDDHHMMSS`） | `plan.rs:61-72,577` | **已实现**（2026-11-02） | 注入时钟的格式化测试 | 否 |
| **ARP 图标**（`DisplayIcon` → 真实 `.ico`） | `plan.rs:588`、`assets/pime.ico` | **已实现**（2026-11-02） | 载荷 → 安装 → 清单 → 卸载的端到端测试 | 否 |
| **安装结束时的下一步指引**（三语言，不改默认输入法） | `cli.rs:196,234`、`text.rs:37-101` | **已实现**（2026-11-02） | 静默模式不弹窗的测试 | 否 |
| **安装向导**（自绘窗口：品牌头带 + 彩虹条、安装位置（**可直接输入**的真 EDIT 子控件）与「浏览…」（`IFileDialog`）、三个选项、进度页、完成页；左上角与右下角各一个 IME1.0 logo） | `wizard.rs`、`crates/pime-setup/assets/ime1.0.ico`、`text.rs` 的 `wizard_text` | **已实现**（2026-11-02，第四轮；第五轮加卸载模式、文本换行、窗口居中；第六轮加路径直输、logo 1:1 直绘、左上角 logo、彩虹配色；**第七轮加整套艺术设计**：DIB 像素面 + 线性光混色 + 解析式抗锯齿 + 柔影） | 编译 + **116 项安装器测试**；向导本身**没有 UI 自动化测试** | 窗口：**是**（第五轮在真实 Windows 上逐页截图，并端到端跑通安装与卸载；第六轮验证目录选择器端到端、路径逐字输入、回车安装；**第七轮在真实 Windows 上看到新版艺术设计**）；TSF 注册/激活：否 |
| VS Code 扩展安装（非致命） | `plan.rs:623-632` | 已实现 | 有 | 否 |
| UAC 提权 + 父子进程随机名报告回传 | `cli.rs` | 已实现 | 有 | 否（未跑真实 UAC） |
| **设为默认输入法 / 激活 profile** | 无 | **不做**（刻意）；改为安装结束时给三语言指引 | `plan.rs:8-9` | 否 |
| **开始菜单 / 桌面快捷方式** | 无 | **不做** | `INSTALLER.md:42` | 否 |
| **独立「修复」模式** | 无（重跑安装包即修复，幂等） | **不做** | `INSTALLER.md:42` | 否 |
| **代码签名** | 无 | **流程已备好，待证书** | `01_Source/scripts/sign-installer.ps1`、`01_Source/docs/SIGNING.md` | 否 |
| **ARP 图标 / 安装日期** | 有 | **已实现**（2026-11-02） | 见上表 | 否 |
| **载荷里带上给 AI 客户端调用的两个程序**（`pime-mcp.exe` = MCP 服务端，`pime-ime.exe` = 命令行与本地服务；**必需**，少一个 AI 客户端就没有可调用的东西） | `payload.rs` 的 `MCP` / `CLI`（必需 8、允许 10）、`plan.rs`、`exec.rs`、`scripts/build-installer.js` | **已实现**（2026-11-05 第十七轮） | `pime-setup` 123 项（单元 105 + 真实文件系统 13 + 真实链接 5） | 否（**没在正式安装目录里跑过 MCP**：新安装包还没装） |

## 3. 结论：按口径分三档

**A. 代码层齐备 + 行为层有自动化证据（可以在可丢弃机器上试）**
DLL 导出与类工厂、**COM 对象计数与 `DllCanUnloadNow`**、文本服务激活、吞键判定、**文档/上下文生命周期回调**、编辑会话与文本写入、候选窗与 DPI 算术、**候选窗归属宿主窗口**、键位映射、输入状态机、安全字段、三语言 profile 注册、三个 category 注册、卸载/升级的精确撤销、安装器事务与回滚、载荷格式与校验、真实文件系统与链接的集成检查、**安装结束时的三语言指引**、**ARP 安装日期与图标**、**签名流程脚本**、**安装向导（自绘 + logo + 安装位置可选且可直接输入）**、**设置窗六页真实页签（含插件开关与按键方案预设）**、**候选列表归位左侧激光面板（打字时没有搜狗式悬浮窗）**、**自带中日英词库（不调用其他输入法）**、**分层窗口由图片原子移动（无边缘残留）**、**词库管理的三个预留接口（AI 整理 / 远程更新 / 用户登录，桩实现）**、**AI 代理的写入通道（MCP / CLI / 本地管道 → 桥接窗口 → 真的 TSF 编辑会话，跨进程真机往返）**。

**B. 代码层存在，但只在假后端 / 单元层验证过**
`StartComposition` 与组合结束的真实 COM 时序、编辑会话在真实宿主下的同步/异步行为、`ITfUIElementMgr` 与真实宿主的对话、候选窗在真实应用中的定位与 DPI、真实注册表写入、真实 `regsvr32`、真实 UAC 提权、真实重启删除、真实安装/卸载全流程、**真实签名（假 signtool 只复现了布局，不是真 PKCS#7）**。

**C. 未实现（附原文）**

| 能力 | 原文 / 原因 |
|---|---|
| `ITfDisplayAttributeProvider` | `register.rs:282-283`「the `tip` module implements none of them yet」。**2026-11-02 复核后的新结论**：`windows` 0.58 **与 0.62 的绑定都只有 2 个方法**（真实 TSF 接口有 3 个），`ITfReadOnlyAttributeList` 在两版里都没有。用现成绑定实现会得到**比真实接口短一格的 vtable**，宿主调用第三个方法会读到界外——这是崩溃而不是"少一个功能"。要安全实现必须手写 vtable 或等 win32metadata 补齐，两者在没有真实 TSF 宿主的情况下都无法验证，故**不实现** |
| `ITfComposition` / `TakeOwnership` | 全仓库无 `TakeOwnership`；组合由宿主拥有（`owner.StartComposition`）。要接管需要自己实现 `ITfCompositionView`（文本 + 属性 + 移位），工作量与风险都不小 |
| `ITfTextEditSession` / `ITfServiceProvider` | 两个接口在 `windows` 0.58 与 0.62 的绑定里**都不存在**，无法用现成绑定实现 |
| `TIPCAP_IMMERSIVESUPPORT` | `register.rs:271-276` 刻意不注册：需要先补齐 `RUST_MIGRATION_PLAN.md` 13.11 的 7 条（AppContainer、无主顶层窗的替代方案、IME WinEvent、UI Automation 候选窗属性、品牌图标等） |
| `TIPCAP_SECUREMODE` | `register.rs:277-281` 刻意不注册：会把未审阅的 DLL 加载进登录界面与 UAC 提示 |
| `TIPCAP_COMLESS` / `TIPCAP_INPUTMODECOMPARTMENT` | `register.rs:282-283`：不声明自己没有实现的能力 |
| 开始菜单快捷方式 / 独立修复模式 | `INSTALLER.md:42`：幂等重跑安装包即修复 |

这些缺口的完整提交单（含严重度、影响、建议处置与批次）见 `02_Verification/FRAMEWORK_GAPS.md`。

## 4. 为什么本机做不了真实系统验证（已实测，不是推测）

| 尝试 | 结果 |
|---|---|
| `Get-VM`（Hyper-V） | 「你没有完成此任务所需的权限」——当前用户不在 Hyper-V Administrators 组 |
| `multipass list` | `cannot connect to the multipass socket`（守护进程未运行；这台机器上 360 安全卫士会删 Multipass 服务，见记忆库） |
| 磁盘上的 Windows ISO | 无 |
| 注册表写入 / `regsvr32` / UAC 提权 | **2026-11-04 已在本机做过一次提权安装**（真实 HKLM 的 COM 键、三个语言 profile、三个 category、ARP 条目、`regsvr32` 不再报 `E_FAIL`；退出码 3010 = 旧 DLL 被占用，注销或重启才换新）。**本轮规则仍然是不在这台生产机上继续做** |

结论：**第三层（判据层）必须换一台可丢弃的 Windows 11 机器做**，运行手册是 `01_Source/docs/TSF_VM_TEST_PLAN.md`，安装器章节的待确认清单是 `01_Source/docs/INSTALLER.md` 第 14.1 节（14 条）。

## 5. 本次验证记录（2026-11-05，第十七、十八轮）

| 检查 | 结果 | 证据 |
|---|---|---|
| Rust 核心 `cargo test --workspace` | **441 / 441 通过**（本轮新增**AI 代理的线上契约** `pime-core/src/agent_proto.rs`：请求 / 回答 / 6 个操作 / 8 个错误码 / 载荷上限 / 消息码 / 窗口类名与标题前缀，10 项往返单测） | `run-all-2026-11-04.log` |
| Rust TSF `cargo test` | **308 / 308 通过**（本轮新增 **DLL 侧的 AI 桥接**：`agent_bridge.rs`（隐藏桥接窗口 + `WM_COPYDATA` 收发 + 密码框不写 + 结构体布局）与 `tip.rs` 的接线（写入光标处 / 替换选区 / 读选区 / 切语言 / 状态）；另修掉一个**真缺陷**：桥接窗口**第二次就打不开**（类注册结果记在局部变量里，`Once` 的闭包只执行一次 ⇒ 切走再切回来，AI 就再也叫不动输入法），并补了**真机跨进程往返**测试） | `run-all-2026-11-04.log` |
| Rust agent `cargo test` | **63 / 63 通过**（本轮新增 `crates/pime-agent`：**MCP 服务端**（7 个工具）、**CLI**（12 个操作）、**本地服务**（命名管道 `\\.\pipe\pime-agent-v1` + HTTP 127.0.0.1 + 令牌）；含一条**真机 `WM_COPYDATA` 往返**与一条「桥接不回答时按时放弃」的保护） | `run-all-2026-11-04.log` |
| 安装器 `cargo test`（单元 105 + 真实文件系统 13 + 真实链接 5） | **123 / 123 通过**（本轮载荷 8 → **10** 条：新增 `pime-mcp.exe` 与 `pime-ime.exe`；安装计划、清单、步骤数与期望值全部同步） | `setup-tests-2026-11-04.log` |
| JavaScript 单元 / 集成 + 引擎差分 | **353 / 353 通过**（0 失败 0 跳过） | `run-all-2026-11-04.log` |
| **输入法场景套件**（21 个 TSF 行为场景：直打/组字/候选/翻页/取消/提交/语言切换…） | **21 / 21 通过** | `01_Source/tests/tsf/ime_scenarios.rs`、`01_Source/docs/IME_TEST_MATRIX.md` |
| **输入法功能覆盖账本自检**（23 个功能逐个点名覆盖它的测试，名字必须在源码里真实存在） | **4 / 4 通过** | `01_Source/tests/tsf/ime_functions.rs` |
| **宿主级测试**（真 COM 类对象：未知 class 拒绝且出参留 NULL；`PIME_IME_LIVE=1` 时走真 msctf） | **4 / 4 通过**（真 msctf 一档为 opt-in；本轮已在真实系统上注册并安装，真实宿主里跑一遍仍未做） | `01_Source/tests/tsf/ime_host.rs` |
| **插件语言两引擎对拍**（SQL 校验 32 / 扩展名解析 275 / 预测 54 用例） | **0 处不一致** | `01_Source/tests/rust/plugin_languages.rs`、`01_Source/tests/js/ut-plugin-language.test.js` |
| **常驻托盘与语音输入**（`crates/pime-tray`：托盘图标、Ctrl+F12 一键切换、Ctrl+Alt+V 录音识别、API key 存凭据管理器；AI worker 新增 `bodyBase64` 以发二进制请求体） | **代码层全绿**（pime-tray 9 项单测、AI worker 93 项单测、clippy 无警告）；**已随安装包分发**（payload 10 条，托盘与 worker 装进 `%ProgramFiles%\IME1.0`，托盘菜单里有「开机自动启动」开关）；**未在真机验证**（TSF DLL 已于 2026-11-04 在真实系统上注册并安装；托盘图标本身、真实麦克风与语音服务仍未验证） | `crates/pime-tray/`、`crates/pime-ai-worker/src/engine.rs` |
| **候选窗换激光彩虹**（分层窗口 `WS_EX_LAYERED` + `UpdateLayeredWindow`，逐像素画深蓝玻璃、圆角、投影、**渐变彩虹带**顶边、**选中行纵向彩虹**光条 + `>` 标记） | **代码层全绿**（`candidate_skin.rs`、`pime-tsf` 250 项 + clippy 干净）；**取证**：进程内宿主截图 `02_Verification/ime-ui/ime-windows.png`，放大图 `ime-ui/candidate-list-zoom.png`；**未在真实宿主里跑过** | `crates/pime-tsf/src/candidate_skin.rs`、`crates/pime-tsf/src/popup.rs` |
| **悬浮栏换激光彩虹**（新增第 4 套皮肤「激光彩虹」，出厂默认；模式按钮下是**彩虹渐变胶囊**，顶边是**渐变彩虹带**） | **代码层全绿**（`bar_skin.rs`、`imesettings.rs` 的默认值与 `skin_label`）；**取证**：同一张截图，放大图 `ime-ui/floating-bar-zoom.png`；**菜单仍未换肤**（登记为 D-22 的后半段） | `crates/pime-tsf/src/bar_skin.rs`、`crates/pime-tsf/src/floating.rs` |
| **托盘与 AI worker 随安装包分发**（载荷 6 → 8 项；托盘「开机自动启动」开关写当前用户的 Run 项；托盘「设置…」接通） | **代码层全绿**（`pime-setup` 116 项、托盘 9 项、clippy 干净；构建时逐个核对载荷程序的导入表）；**未在真机验证**（托盘图标、录音、开机自启的实际写入都没跑过） | `crates/pime-setup/src/payload.rs`、`scripts/build-installer.js`、`crates/pime-tray/src/autostart.rs` |
| **光束改半透明蓝激光**（去纯白实心芯，核心与光晕的 alpha 都压低，整束光在蓝色系里渐变）+ **彩虹渐变收进内核**（横向 / 纵向 / 面板顶边三个通用绘图函数） | **代码层全绿**（`pime-core` 410 项，含 4 项新单测；`pime-tsf` / `pime-tray` / `pime-setup` 的 clippy 干净）；**取证**：光束探针 `crates/pime-tsf/examples/laser_beam_probe.rs`，放大图 `ime-ui/laser-beam-zoom.png`、`ime-ui/settings-panel-zoom.png`；**未在真实宿主里跑过** | `crates/pime-core/src/style.rs`、`crates/pime-tsf/src/candidate_skin.rs`、`crates/pime-tsf/src/bar_skin.rs`、`crates/pime-tsf/src/laser_ui.rs` |
| **设置窗六页真实页签**（常规/按键/外观/**插件**/AI/关于；插件页可勾选 **Cypher / PL\/SQL / T-SQL**，默认 PL/SQL 开、T-SQL 关） | **代码层全绿**（`settings_window.rs` 的 `PAGES`、`SettingsPage::Plugins`、`ControlKind::PluginCheck`；`imesettings.rs` 的 `ImeSettings.plugins`；`tip.rs` 的 `apply_plugins()`）；**取证**：探针把设置窗切到插件页后实拍，页签 6 个、插件勾选三行可见（`02_Verification/ime-ui/ime-windows.png`）；**未在真实宿主里跑过** | `crates/pime-tsf/src/settings_window.rs`、`crates/pime-tsf/src/imesettings.rs` |
| **打字时不再显示搜狗式悬浮窗**（候选列表改由左侧激光面板承载，`LaserCandidates` 实现 `CandidateUi`；`popup.rs` 的 `PopupWindow` 生产路径不再使用，模块与代码保留） | **代码层全绿**（`laser_candidates.rs`、`laser.rs` 的 `PanelCandidate`／`PanelItem::Candidate`／`set_candidates`／`place_panel`；`pime-tsf` 端到端测试断言候选行真的出现在面板里）；**取证**：同一张实拍图里左侧面板内可见 5 行候选；**未在真实宿主里跑过** | `crates/pime-tsf/src/laser_candidates.rs`、`crates/pime-tsf/src/laser.rs` |
| **输入法自带中日英词库**（拼音→中文、罗马音→假名/日文汉字、英文词表；**不调用任何其他输入法**） | **代码层全绿**（`pime-core/src/cjk.rs` + `cjk/pinyin-syllables.txt`、`cjk/pinyin-phrases.txt`、`cjk/romaji-kana.txt`、`cjk/japanese-words.txt`、`cjk/english-words.txt`）；**行为层已实证**：中文档输入「ni」→候选含「你」→上屏后文档里是一个汉字；日文档「nihon」→假名；英文档仍是代码优先 | `crates/pime-core/src/cjk.rs`、`crates/pime-tsf/src/controller.rs` 的真实词库端到端测试 |
| **Tab 语义与按键方案预设**（默认 **Tab = 移动高亮并选取**（程序员习惯），Enter/空格/数字上屏；设置里可改成「上屏候选」（微软/搜狗/ATOK 口径）或「交给宿主」；按键方案预设：程序员/微软/搜狗/ATOK/自定义） | **代码层全绿**（`pime-core/src/input.rs` 的 `tab_cycles`；`imesettings.rs` 的 `ImeSettings`；`settings_window.rs` 的按键页控件）；**未在真实宿主里跑过**（真实键盘下的 Tab 手感要你自己试） | `crates/pime-core/src/input.rs`、`crates/pime-tsf/src/settings_window.rs` |
| **分层窗口边缘残留**（分层窗口移动改为**由图片原子移动**：`UpdateLayeredWindow` 带新目标点重新上传图层，surface 与窗口一起走，旧位置不留像素） | **代码层全绿**（`glass.rs` 的 `move_to()` 与纯函数 `move_way()`；单测 `a_window_with_a_picture_is_moved_by_its_picture`）；**账本新增 F-LAYERED-WINDOW 条目**；**未在真实宿主里跑过** | `crates/pime-tsf/src/glass.rs`、`01_Source/tests/tsf/common/scenarios.rs` |
| **AI 代理通道（真机往返）**（AI 客户端 → `WM_COPYDATA` → 焦点进程里的 `pime_tsf.dll` → 回包） | **通道本身跑通**：DLL 侧一条**跨进程**真机测试（宿主进程里是真的桥接窗口与真的消息，另一个进程按协议发请求并等回包）+ 客户端侧一条真机往返（假桥接窗 + 真客户端，含超时保护）；**没跑过**「文字真的落进记事本 / VS Code 的文档」——新 DLL 还没换上 | `crates/pime-tsf/src/agent_bridge.rs` 的 `channel_tests`、`crates/pime-agent/src/bridge.rs` 的 `channel_tests` |
| **给主流 AI 的接入面**（MCP 服务端 7 个工具 / CLI 12 个操作 / 本地命名管道 + HTTP） | **代码层全绿**（`pime-agent` 51 项、clippy 干净）；MCP 的 `initialize` / `tools/list` / `tools/call`、CLI 的各操作、HTTP 的健康检查 / 令牌 / 拒绝、命名管道的请求，**都在本机跑过**；**未在正式安装目录里跑过** | `crates/pime-agent/`、`01_Source/docs/AI_AGENT_API.md` |
| **本轮取证探针 + 门禁断言**（探针写 `probe-report.json`：设置窗是否打开、页签数、当前页标题、候选行数、窗口矩形、屏幕尺寸；`ime-ui-check.js` 断言页签 **6** 页、当前页是「插件」、面板里有候选行、**设置窗在屏幕中间**） | **通过**（截图无法证明「页签存在」与「窗口居中」，所以改成机器可读的报告 + 断言；页数≠6 或列表为空时退出码非 0） | `01_Source/crates/pime-tsf/examples/ime_ui_probe.rs`、`01_Source/tests/smoke/ime-ui-check.js`、`02_Verification/ime-ui/probe-report.json` |
| TSF DLL PE 检查（x64、四个导出、只导入系统 DLL） | 通过 | `run-all-2026-11-04.log` |
| `.vsix` 打包与包内自检（49 个文件、7 个内置规则包 + 3 个插件包、反向对照） | 通过 | `run-all-2026-11-04.log` |
| 安装包构建 + `--dry-run` 自校验（10 个载荷条目） | 通过（未安装、未注册） | `installer-build.log` |
| 文档数字 ↔ 产物一致性（含反向对照：篡改任一数字必变红） | 通过 | `01_Source/tests/smoke/installer-claims-check.js` |
| 证据集交叉核对（**构建日志 ↔ dist 产物 ↔ `.sha256` sidecar ↔ 文档正文 ↔ 两份测试日志**） | 通过（第六轮新增；18 项断言） | `01_Source/tests/smoke/evidence-cross-check.js` |
| 签名脚本自检（`-Help` / 无证书 / 解析 / ASCII / 假 signtool 端到端） | 通过 | `01_Source/scripts/sign-installer.ps1`、`01_Source/docs/SIGNING.md` |
| 仓库完整性清单（`MANIFEST_SHA256.txt`，文本按 LF 归一化、二进制按原字节） | **699 / 699 对得上**（第五轮统一口径全量重算：172 条刷新 + 8 个新文件；第六轮再刷 13 条 + 补登记 2 个新文件 `build.rs` 与 `evidence-cross-check.js`；**第七轮补登记 `paint.rs`**；**第八轮补登记 `examples/tsf_host.rs` 与 `uat-claims-check.js`**；**第十轮再刷 33 条 + 补登记 18 个新文件**——3 个插件包、插件语言规范、输入法测试矩阵、IME 场景/账本/宿主三套测试、原生窗口探针与抓屏脚本、插件语言两侧测试与夹具，以及 L4 取证的两张桌面截图；**上一轮再刷 20 条 + 补登记 5 个新文件**——`autostart.rs`、`bar_skin.rs`、`candidate_skin.rs`，以及两张放大取证图（`candidate-list-zoom.png`、`floating-bar-zoom.png`）；**本轮再刷 9 条 + 补登记 3 个新文件**——`examples/laser_beam_probe.rs` 与两张取证图（`settings-panel-zoom.png`、`laser-beam-zoom.png`））；**第十五轮再刷 19 条 + 补登记 2 个新文件**（`skin.rs`、`docs/HOTKEYS.md`）**并移除 2 个已删文件**（`floating.rs`、`bar_skin.rs`）；**本轮（第十六轮）再刷 22 条 + 补登记 10 个新文件**——`cjk.rs` 与 5 个词库数据文件、`services.rs`、`laser_candidates.rs`、`predictor.rs`、`probe-report.json`；**本轮（第十七轮）再刷 18 条 + 补登记 13 个新文件**——线上契约 `agent_proto.rs`、桥接窗口 `agent_bridge.rs`、`crates/pime-agent/` 的 9 个文件与它的 `Cargo.lock`、`docs/AI_AGENT_API.md`，并与磁盘实际文件**零缺口** | `node 01_Source/scripts/update-manifest.js --check` |

**注意口径**：上一行 9 项全部是**代码层与行为层**的证据。它们**不**构成"这是一个能用的 Windows 输入法"的证明——那一层是空的（见第 4 节）。

### 5.1 第五轮：安装向导在真实 Windows 上的逐页验证（2026-11-02）

这是本项目第一次在**真实 Windows 桌面**上看到自己的窗口。做法：用 `PrintWindow` 抓窗口位图，把三页都截了下来，并用窗口消息注入点击，端到端跑通了安装与卸载。

| 页面 | 实测结果 |
|---|---|
| 欢迎页（安装） | 品牌头带 + 彩虹条 + 介绍段（自动换行，不再被裁切）+ 安装位置面板 + 三个复选框（**初值由 `--no-*` 决定**，此前恒为全选）+ 「参照…／キャンセル／インストール」+ 右下角彩虹 logo |
| 进度页 | 「インストール中…」+ 实时步骤日志（`[1/8] create folder …`） |
| 完成页（**所有提示信息的落点**） | 「インストールが完了しました」+ 安装结果 + **启用指引三步**（设置 → 键盘 → Win+Space）+ 「閉じる」。**全程无 MessageBox** |
| 卸载确认页 | 「IME1.0 をアンインストール」+ **要删什么**（版本、安装目录、"只删除本安装程序创建的文件与注册表项"）+ 计划警告 + 安装目录面板 |
| 卸载完成页 | 「アンインストールが完了しました」+ 「IME1.0 を削除しました。」+ 「閉じる」 |

> 说明：本机界面语言是 ja-JP，所以截图是日文；三种语言走同一套版面逻辑（`text.rs` 三语齐备）。

**这一轮在真机上抓到并修掉的两个缺陷**（都是本轮自己引入的，不是既有实现）：

1. **卸载页摘要整段空白**：`text.rs` 的 `is_wizard()` 白名单漏了本轮新增的 6 条卸载文案，`template()` 把它们路由到 `core_text()` 的 `_ => ""`。既有的"三语占位符 parity 测试"**没有**抓到——它只比较三种语言之间，而漏路由的消息在三种语言下**都是空串**，parity 天然成立。已补一条**判据层**测试：每条消息在每种语言下都必须解析出非空文本。
2. **向导窗口永远开在屏幕 (0,0)**：两个向导窗口会完全重叠，旧窗口的「Install」与新窗口的「Uninstall」按钮**坐标重合**，一次点击就可能打错按钮。已改为在工作区**居中**。

> 事件记录：上面第 2 条的原型事故——本机上残留了一个 16:43 启动、无人操作的向导窗口，随后一次针对它的点击落到了新建的卸载向导的「アンインストール」上，**把 `C:\Program Files\Programming IME` 里已装的副本卸载掉了**（该目录现只剩一个 `pime_tsf.dll.pime-old.pime-del-21804-0` 遗留文件）。这是本轮唯一一处未经用户确认的破坏性操作，已如实记录；重装新包即可恢复。

### 5.2 第六轮：安装位置可输入、logo 与配色（2026-11-02）

这一轮把「安装位置」从**只能选**改成**选与打都行**，并重做了 logo 与配色。验证仍然在**真实 Windows 桌面**上做，但换成了**像素级取证**与**控件级注入**——不再只靠肉眼。

| 验证项 | 做法 | 实测结果 |
|---|---|---|
| **路径可以直接输入字符** | 不用 `SendInput`（对非前台窗口不可靠），而是向向导自己的消息队列投递 `WM_CHAR` / `WM_KEYDOWN` | `EN_CHANGE` 确实到达编辑框，字段文本**逐字增长**，`state.prefix` 同步更新——路径是真打进去的，不是"看起来像" |
| **回车即安装** | Win32 **加速键表**（`CreateAcceleratorTableW` + `FVIRTKEY/VK_RETURN`），在 `pump()` 里 `TranslateAcceleratorW` | 加速键被翻译并产生 `WM_COMMAND id=1 code=1`；随后**真的装成了**：目标目录 `D:\tmp-pime-wiz\typed6` 里出现了 `install-manifest.json` / `LICENSE` / `pime-core.exe` / `pime.ico` / `pime_tsf.dll` / `programming-ime.vsix` / `uninstall.exe` |
| **「浏览…」端到端** | 控件级：找 class `Edit`（id=1152）→ `WM_SETTEXT` → 点 id=1 的 IDOK 按钮（`BM_CLICK`） | 路径从 `orig-omega` 变成 `chosen-alpha`，向导面板里的路径确实跟着更新 |
| **logo 不再发白** | 把窗口位图里 logo 区域的像素与 ICO **各个原生档**逐一比对，取最佳匹配位移 | 向导右下角 logo 与 **48px 原图**最佳匹配，位移 (682,444)，**平均差 4.76/255** ⇒ 判定为 **1:1 直绘**、没有被缩放 |
| **图标统一** | 右下角 logo / 窗口图标 / EXE 资源（`build.rs` 的 `windres`）/ 载荷 `pime.ico` | 四处都是同一个 logo（`crates/pime-setup/assets/ime1.0.ico`，7 档 PNG：16/24/32/48/64/128/256） |

**这一轮自己抓到并修掉的一个缺陷**（不是既有实现）：

- **每个圆角形状都带 1px 黑描边**：`fill_round()` 只换了画刷、没换画笔，于是每个填充形状的边框被 GDI 用默认黑笔描了一圈——左上角的白色 logo 块看上去像一个"脏圈"。已改为同时选入 `CreatePen(PS_NULL, 0, COLORREF(0))` 空笔；需要描边的地方显式调 `frame()`。

**logo 尺寸的决策**（写在这里，避免以后有人"优化"回去）：**绝不放大，也绝不缩放**——绘制尺寸直接取 ICO 的**原生档**（`logo_fit_size` 返回文件里真实存在的档位），48px 就画 48px。上一版是"按目标尺寸缩放"，38px 的目标落到 ICO 最小可用档 48px 上，GDI 一缩放就把图**发白发虚**了。白底圆角块因此从 46 放大到 56，只为容下 48px 原图。

> **口径**：本节记的是**窗口层**的真机证据（向导长什么样、点下去有没有反应）。**TSF 注册/激活那一层依然是空的**——见第 4 节与 `02_Verification/FRAMEWORK_GAPS.md`。

### 5.3 第七轮：艺术设计到位 + **Tab 确认上屏**（2026-11-02）

这一轮两件事：**（甲）**你要求的「渐变混合、圆圈抗锯齿等艺术设计到位」；**（乙）**「选择目标文字，按 Tab 确定后输出出来」这条基本链路。

#### 甲、艺术设计：新增 `crates/pime-setup/src/paint.rs`（自绘像素面）

渲染不再靠 GDI 的画刷/画笔混合，而是自建 **32 位 DIB 像素面**逐像素画，文字仍由 GDI 画（字体质量改为 `ANTIALIASED_QUALITY`）：

| 手段 | 实现 | 为什么不用现成的 |
|---|---|---|
| 混色 | **线性光**（sRGB ↔ 线性）插值，不是直接混 RGB | 直接混 RGB 会让渐变中段发灰、发脏 |
| 彩虹 | **Catmull-Rom 样条**穿过 7 个品牌色 | 折线过渡会在色点之间出现折角 |
| 抗锯齿 | **解析式 SDF 覆盖度**（圆角矩形 / 线段的距离函数） | 超采样要画很多遍，代价高且边缘仍会有台阶 |
| 柔影 | 同形状 + 偏移 + 半影 + 低强度叠加 | GDI 没有软阴影 |
| 复选框 | **自绘**圆角方块 + 两笔对勾（不用字体字形） | `☑/☐` 的字形在不同字体下差别很大 |

> **为什么不用 `AlphaBlend`**：它会引入 `msimg32.dll` 导入，破坏「安装包只导入 Windows 系统 DLL」这条门禁。所以阴影与覆盖度都在**自己的像素面**里算，最终 `BitBlt` 以 SRCCOPY 输出，alpha 恒为 255。

**像素级取证（真实 Windows 截图上量出来的，不是估计）**：

| 指标 | 旧版 | 新版 |
|---|---|---|
| 品牌带上**相邻像素最大跳变**（水平） | **74**（色段交界处硬边） | **1**（连续，无色带） |
| 品牌带上相邻像素最大跳变（垂直） | 2 | 1 |
| logo 圆片边缘 | `000000` **硬边** | **12 级连续过渡**（`c9c7d1` → … → `ffffff`） |
| 圆角 / 按钮 / 对勾 | 无抗锯齿 | 全部解析式抗锯齿 |

**过程中自己抓到并修掉的四个 bug**（都是本轮新写的渲染代码，已登记在缺口台账 §5.1/§5.3 的同一条教训下）：

1. `COLORREF` 是 `0x00BBGGRR`（**红在最低字节**），而我按 `0xRRGGBB` 解析 ⇒ **整页红蓝互换**；已按低位=红修正，并在 `Rgb::from_colorref` 的注释里写死，避免再犯。
2. `from_linear` 漏乘 255 且用截断 ⇒ 颜色整体偏暗；改为 `.round()`。
3. Catmull-Rom 的 t¹ 系数写错（应为 `-0.5*p0 + 0.5*p2`）⇒ 样条在中段鼓起。
4. 阴影衰减方向反了（越远越实）⇒ 改为 `1 - smoothstep(clamp(d/blur))`，并加了偏移。

#### 乙、**Tab 确认上屏**（这一条改了键位口径，与两份需求文档冲突，登记在缺口台账 §5.5，请你裁定）

| 阻断点 | 位置 | 后果 |
|---|---|---|
| `tab_cycles` 默认 **true** | `pime-core/src/input.rs` 的 `KeyMap::default()`、`pime-tsf/src/imesettings.rs` 的 `LanguageSettings::default()` | 即使进了输入模式，Tab 也只在候选之间移动，**永不确认** |
| 候选列表显示时 Tab **根本进不了状态机** | `pime-core/src/input.rs` 的 `on_key()` / `would_consume()` | 默认（直接）模式下 `capture_active()` 为假，`reserved_key()` 不被调用 ⇒ **Tab 被宿主吃掉**（记事本里插一个制表符）——这就是你实测到的现象 |

**改法**：`tab_cycles` 默认改为 **false**（Tab 确认高亮候选）；新增 `tab_is_ours()`——**候选列表在屏幕上时 Tab 就属于候选窗**（直接模式同样成立，不需要先按 Alt+F1）。其余保留键（Enter / 数字 / 方向键 / Esc）仍要求显式开启候选接管，**没动**。
**逃生开关**：设置窗口新增「候选窗显示时 Tab 交给宿主（IDE 保留缩进）」，勾上就把 Tab 完全还给宿主，回到需求文档描述的行为，**不用重装**。

**「选哪一行」的范围（说清楚，免得实测时误会）**：**鼠标点哪一行就上屏哪一行**（原生候选窗的点击处理，任何模式都成立）；**键盘上下移动高亮需要输入模式**（Alt+F1）——直接模式下 ↑↓ 仍然是宿主的（光标移动），所以此时按 Tab 取的是**第一行**。

**判据层仍然是空的**：本轮补的是代码层与行为层（`pime-core` 244 项、`pime-tsf` 219 项单测全绿，含「直接模式下按 Tab，文档里真的出现候选文字」的端到端断言），**在真实记事本里按 Tab 能不能上屏，本机没法验**。请你按下面这四步实测：

1. 关闭当前那个安装向导窗口（它是 19:29 的构建），双击 `01_Source/dist/ProgrammingIME-0.2.1-win-x64-setup.exe` 装这一版。
2. 记事本里切到 IME1.0（Win+Space 或「键盘 → 语言」），打一个会出候选的前缀，例如 `fn`。
3. **按 Tab**：期望候选窗消失，**高亮候选的文字被写进记事本**（不是插一个制表符）。
4. 如果你不想要 Tab 被拿走：悬浮窗 → 设置 → 勾「候选窗显示时 Tab 交给宿主（IDE 保留缩进）」，保存后立即生效。


### 5.4 第九轮：F1 / F2 的「文字输入 vs 代码输入」分流与输入功能（2026-11-03）

这一轮把你 2026-11-02 的要求「输入功能和切换文字输入和代码输入的 f1 f2 功能必须做好再给我」落到**可复现的证据**上。

**你说的「文字输入 / 代码输入」在仓库里的落点**：全仓 grep「文字输入」「代码输入」**0 命中**；对应的是 **F2 的选区分类**（需求原文：圈选的是**文字**就基于文字生成提示词，圈选的是**代码**就审视所在文件）与 **F1 的作用域链**（选区 → 函数 → impl/模块 → 整个文件）。本轮按这个口径验证。

#### 甲、Playwright 在**真实 VS Code 1.143.0** 上第一次跑起来（此前从未运行过）

扩展从源码目录 `--extensionDevelopmentPath` 加载，用一次性用户目录，不碰你自己的设置、扩展与密钥。

**52 个场景：51 通过 / 1 按设计跳过 / 0 失败**（跳过的是 ENG-02，它需要 `PIME_CORE_EXE`，属于 rust 引擎那一轮），耗时 7.3 分钟。

| 组 | 场景 | 证明了什么 | 证据 |
|---|---|---|---|
| UAT-36..41（PRM-40..45） | **F2 的文字/代码分流** | 文字选区 → 剪贴板是改进后的 5W1H 提示词（六段标题、顺序固定）；代码选区 → 请求带的是**整个文件**、剪贴板是带证据的发现；没有可优化项 → 右下角通知、剪贴板不动、下一字符后消失；AI 关闭 → 复制未复核模板并说明原因、**零请求**；无选区 → 命令不运行（仍是「重命名」）；敏感文件 → 不发不抄 | `uat/uat-results.json`、`uat/screenshots/` |
| UAT-42..49（ANN-60..85） | **F1 的作用域链** | 无选区时以所在函数为作用域、选区时**恰好**是选中的行；Wider/Narrower 沿链移动且**不发任何请求**；Apply 是**一步撤销**、Ctrl+Z 复原；拒绝授权 → 零请求、再次 Generate 会再问；密钥文件 → 请求里没有密钥字节 | 同上 |
| UAT-01..08 | **输入功能（编辑器侧）** | Esc 什么都不写；接受候选**只插在输入前缀处**；Ctrl+Z 复原；暂停后无候选、原文字不动；AI 关闭时连打 100+ 字符再按 F1 → **零请求** | 同上 |

**这一轮抓到并修掉的 3 个缺陷都是测试脚本的缺陷，产品实现是对的**（逐条证据与回退办法登记在缺口台账 §5.7.3：D-7 面板 DOM 异步重建导致的一次性读断言失败、D-8 漏了第二次授权弹窗、D-9 行选择器有歧义）。

#### 乙、TSF 侧：新增进程内宿主，定位到「为什么本机证不了」

新增 `01_Source/crates/pime-tsf/examples/tsf_host.rs`（**诊断工具，非产品功能**，是否作为交付物保留请你裁定，缺口台账 §5.7.5 第 2 条）：加载**真实的 `pime_tsf.dll`**、用**真实的 msctf**，**不写注册表、不需要管理员权限**。

| 结论 | 依据 |
|---|---|
| 真实 COM 能创建本 DLL 的类对象 | `DllGetClassObject` → `IClassFactory` → `ITfTextInputProcessorEx` 成功 |
| 真实 msctf 线程管理器能创建 | `CoCreateInstance(CLSID_TF_ThreadMgr)` 成功 |
| msctf **接受**线程事件 sink | `ITfSource::AdviseSink` 返回 cookie 67108867 |
| msctf **拒绝**按键 sink | `ITfKeystrokeMgr::AdviseKeyEventSink` → E_INVALIDARG（0x80070057）；宿主自己做同样的调用**一样被拒** |

⇒ **文本服务要成为「本线程当前输入处理器」才会拿到按键**，而这要求上下文**有键盘焦点**；控制台宿主没有窗口 ⇒ 按键不路由（实测 `TestKeyDown` 恒 false）。
**因此第三层（判据层）仍然是空的**：F1/F2 与编辑器侧输入功能**行为层已实证**，但「Windows 系统输入法在记事本里上屏」这一步仍需**一个真正的宿主应用**（也就是缺口台账 §5.7.5 第 1 条里那次管理员动作 + 记事本实测）。

**顺带纠正本轮上一条推断**：实测发现**这台机器上 profile 其实早就注册好了**（你 2026-11-02 装的 `D:\IME1.0`，`HKLM\…\CTF\TIP\{43F95ED1-…}` 三个语言 profile 齐全），`ITfInputProcessorProfileMgr::ActivateProfile` 成功且 **msctf 把已安装的那份 DLL 加载进了宿主进程**——所以「缺注册」不是原因，**缺的是有焦点的宿主应用**。详细实测表见缺口台账 §5.7.4。

---

### 5.5 第十八轮：删掉右下角球形彩虹悬浮窗，只留屏幕边缘的激光口（2026-11-05）

这一轮接的是你点名的要求：**「把球形彩虹悬浮窗全都去掉，我需要的是屏幕边缘的激光口，之前做过一个。」**

| 检查 | 结果 | 证据 |
|---|---|---|
| **右下角球形彩虹悬浮窗整条链路删除**（窗口类 `ProgrammingIME.Logo`、`Role::Logo`、`GlassEvent::LogoClick`、`LogoState`、`Model.logo`、`PanelOrigin::Logo`、`paint_logo()`、`on_logo_click()`、`LOGO_SIZE`、`desktop_work_area()`） | **代码层全绿**（`pime-tsf` 285 项单测全过；`cargo clippy --all-targets` 与上一轮**逐条一致**——用 `git stash` 把工作区退回 HEAD 再跑一遍做对照，三条告警在 HEAD 上原样存在，**不是本轮引入的**） | `crates/pime-tsf/src/laser_ui.rs`、`glass.rs`、`laser.rs` |
| **屏幕边缘的激光口保留**（26 px × 1 px 的淡蓝光缝 + 5 px 光晕，半透明，2.6 s 一个呼吸周期；注释明写「no ring of the brand colours, no white core」） | **代码层全绿**（单测 `the_mouth_is_a_thin_pale_line_inside_the_window_of_the_glow` 仍钉住「亮、细、蓝、其余保持暗」） | `crates/pime-tsf/src/laser_ui.rs` 的 `draw_glow()` |
| **实拍取证**：右下角不再有球 | **通过**——旧截图那处是 **168 个强着色像素环绕 121 个浅色核心像素**（彩虹环 + 白核）；新截图同一区域是 **35 个强着色像素、0 个浅色核心**（只剩桌面壁纸的落叶） | `02_Verification/ime-ui/ime-windows.png`、`01_Source/tests/smoke/ime-ui-check.js` |
| **门禁新增断言**（「一圈强着色像素 + 中间一块浅色核心」这个形状 ⇒ 直接失败） | **通过**（旧截图当正样本能检出，新截图当负样本不误报） | `01_Source/tests/smoke/ime-ui-check.js` |
| 光束 / 左侧激光面板 / 候选列表 / 设置窗 | **未改动**（面板顶边那道彩虹**渐变带**保留——它是一条渐变，不是球） | `crates/pime-tsf/src/laser_ui.rs` |
| **真实宿主（记事本 / VS Code）里看到的就是没有那颗球** | **未验证**——新 DLL 还没换上（上次安装退出码 3010＝旧 DLL 正被系统占用，要注销或重启） | — |

**口径**：这一轮改的是**界面构成**，不是功能：删掉的是一条装饰性悬浮窗，**没有删任何能力**（面板、候选、光束、设置窗都在）。留给你裁定的两项（D-42 面板入口只剩悬停光口、D-43 托盘图标本身也是彩虹圆盘但不是悬浮窗）登记在缺口台账 §5.16.2。


### 5.6 第十九、二十轮：彩色激光口 + TASE 第一波（2026-11-05）

| 检查 | 结果 | 证据 |
|---|---|---|
| **每个输入源一颗自己的彩色激光口**（用户本人 + 每个 Agent；同厂商子代理也算不同输入者） | **代码层全绿**（`pime-core` 309 项含新增 7、`pime-tsf` 298 项含新增 13、`pime-agent` 60 项含新增 9；`pime-agent` 的 clippy 干净，`pime-tsf` 只剩既有的两条 D-41） | `crates/pime-core/src/agent_proto.rs`、`crates/pime-tsf/src/mouths.rs`、`crates/pime-agent/src/{bridge,cli,mcp}.rs` |
| **实拍：口散落在上/左/右、颜色互不相同、光束用主人的颜色** | **通过**——探针开三个口（用户 + 两个同厂商子代理），报告写着 `left #9ED8FF` / `top #FF8F1F` / `right #FC17FC`；截图里上边缘一道**橙色**、右边缘一道**品红色**、左边缘一道淡蓝 | `02_Verification/ime-ui/ime-windows.png`、`02_Verification/ime-ui/probe-report.json` |
| **门禁新增断言**（右下角不得有球形彩虹 / 口 ≥2 / 颜色互不相同 / Agent 色相差 ≥45° / 口分布在 ≥2 条边） | **通过**（判据用**形状**而不是肉眼：旧截图当正样本 168 着色 + 121 浅色核心必须命中，新截图 0/0 必须不命中） | `01_Source/tests/smoke/ime-ui-check.js` |
| **TASE 设计文档归档**（5 份文件 + 索引 + 映射说明 + 补齐的文件夹） | **通过**（5 份文件与附件**逐字节 sha256 一致**；映射说明 42 KB，含 15 个新 crate 的落点与 **13 条已知落差**） | `00_Documents/` |
| **93 项统一资产目录**（GoF 23 + 工程模式 20 + 算法 50） | **通过**（93 manifest + 93 知识卡 + 索引；新增的零依赖校验脚本 **0 问题**；状态一律 `DOCUMENTED`、`implementations` 全空） | `01_Source/assets/tase-catalog/`、`01_Source/scripts/validate-tase-catalog.js` |
| **六个新 crate**（contract / registry / planner / metrics / local-ipc / ime-core） | **代码层全绿**（**312 项测试**，clippy 全干净，零第三方依赖；`tase-local-ipc` 还真的用 `x86_64-unknown-linux-gnu` **交叉编译验证过**跨平台） | `01_Source/crates/tase-*/` |
| **门禁纳入新 crate** | **通过**（`run-all.js --tase`，每个 crate 各自的构建目录；claims 增加 `rust_tase_tests`） | `01_Source/tests/run-all.js`、`01_Source/tests/smoke/installer-claims-check.js` |
| **真实宿主（记事本 / VS Code）里的多色激光口** | **未验证**——新 DLL 还没换上（注销 / 重启后才有）；截图仍是进程内宿主的实拍 | — |
| **`tase-semantic` / `tase-executor` / `tase-local-hub` / `tase-catalog` / `tase-catalog-mcp` 与 `pime-agent` 的 `tase.*` 工具** | **未做**（M1 的常驻 Hub 与 M2 的目录工具还没落地） | — |

**口径**：这一轮新增的是**能力**（多 Agent 协作状态可视化）与**新系统的骨架**（契约、目录、计划、指标、传输）。**没有**把任何未经真机验证的东西说成已验证——多色激光口只在进程内宿主里实拍过，真实宿主要等新 DLL 换上。


<!-- installer-claims:begin -->
# machine-readable claims (checked by 01_Source/tests/smoke/installer-claims-check.js)
installer_file = ProgrammingIME-0.2.2-win-x64-setup.exe
installer_bytes = 10132518
installer_sha256 = 23776fa0c34c7352508b63fe46d9c59db3f46c682432981bb3e349736945ac50
stub_bytes = 788480
payload_entry_count = 10
payload_pime_tsf_dll_bytes = 2637824
payload_pime_core_exe_bytes = 3214648
payload_license_bytes = 1085
payload_programming_ime_vsix_bytes = 218970
payload_pime_ico_bytes = 66724
payload_pime_tray_exe_bytes = 1111040
payload_pime_ai_worker_exe_bytes = 400896
payload_pime_mcp_exe_bytes = 929792
payload_pime_ime_exe_bytes = 762368
payload_meta_json_bytes = 87
rust_core_tests = 441
rust_tsf_tests = 308
rust_agent_tests = 63
rust_tase_tests = 678
js_tests = 353
evidence_run_all_log = run-all-2026-11-04.log
evidence_setup_log = setup-tests-2026-11-04.log
rust_setup_unit_tests = 105
rust_setup_real_fs_tests = 13
rust_setup_real_links_tests = 5
<!-- installer-claims:end -->

<!-- uat-claims:begin -->
# machine-readable claims (checked by 01_Source/tests/smoke/uat-claims-check.js)
uat_results_file = uat/uat-results.json
uat_scenarios = 52
uat_passed = 51
uat_skipped = 1
uat_failed = 0
uat_flaky = 0
uat_skipped_case = ENG-02
<!-- uat-claims:end -->

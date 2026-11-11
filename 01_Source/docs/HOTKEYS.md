# 快捷键设计：Ctrl+F12 / Ctrl+F1 / Ctrl+F2 / Alt+F1 / Alt+F2

> 本文是产品**所有快捷键**的唯一设计档案：每一个键的归属层、作用、实现位置、冲突与处置、以及覆盖它的测试。
> 实现改了先改这里；本文与代码不一致时，以本文为准并立即修正代码或本文。
>
> **2026-11-11 用户的裁定**：`Ctrl+F12` 不再是「切换输入法」，而是**在 IME1.0 内部切换「代码输入 ⇄ 自然语言输入」**；
> **出厂模式是代码输入**（输入法一个字符都不吃）。「切换输入法」这个动作还在（托盘右键菜单 + 激光面板的发光口点击），
> 但它**没有快捷键了**——登记为待裁定项 **D-95**。

## 1. 分层原则

产品的快捷键分两层，**同一层内不重叠**：

| 层 | 谁注册 | 何时有效 | 为什么要这一层 |
|---|---|---|---|
| **系统级** | 托盘进程 `pime-tray`（`RegisterHotKey`） | 始终有效，**哪怕 IME1.0 现在不是活动输入法** | 「语音输入」这类手势的意义就是：用户正在别的输入法里，想写代码 |
| **编辑器级** | VS Code 扩展 `programmingIme` | 扩展启用后、编辑器有焦点时 | AI 面板跟着编辑器走，扩展才有上下文、Diff 和授权 |
| **输入法级** | 系统输入法本体（TIP，`pime-tsf`） | 只有 IME1.0 是活动输入法时 | 代码输入/自然语言输入、直输/输入模式、显式组合串这类手势只有在输入法里才有意义 |

由此得到两条硬规则：

1. **不抢编辑器的原生键。** 纯 `F1` 是命令面板（帮助），纯 `F2` 是重命名符号，所以 AI 面板用 `Ctrl+F1` / `Ctrl+F2`。
   **`Ctrl+F12` 也不再抢**：它是输入法级的手势（模式切换），托盘**不许**注册它——系统级热键会在键到达文本服务之前
   就把它吞掉，那这个手势就永远不生效（见 §3.1）。
2. **能关就关。** 编辑器侧的键可以用 `programmingIme.f1Binding` / `programmingIme.f2Binding` 换成 `ctrl+shift+f1` / `ctrl+shift+f2` 或 `none`；系统侧的键目前写死在代码里（登记为待办，见 §6）。

## 2. 总表

| 快捷键 | 作用 | 层 | 实现位置 | 覆盖它的测试 |
|---|---|---|---|---|
| **Ctrl+F12** | **代码输入 ⇄ 自然语言输入**（出厂是**代码输入**：输入法一个字符都不吃；自然语言输入里打字开组合、候选窗接管数字/Tab/空格/回车） | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_mode_chord` → `crates/pime-tsf/src/controller.rs::set_input_mode` | `keymap.rs::ctrl_f12_is_the_mode_shortcut_and_nothing_else_is`、`controller.rs::ctrl_f12_switches_between_the_code_input_and_the_natural_language_input`、`tests/tsf/ime_selection.rs::ctrl_f12_switches_the_mode_and_the_keys_follow`、`tests/tsf/common/scenarios.rs` 的 `IME-S06B` |
| **Ctrl+Alt+V** | 语音输入：录音、识别、写入当前文档 | 系统级 | `crates/pime-tray/src/main.rs`（`HOTKEY_VOICE`） | 见 §6（未覆盖） |
| **Alt+F1** | 代码输入 ⇄ 自然语言输入（与 Ctrl+F12 同一个动作的第二条路） | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_input_mode_chord` → `crates/pime-tsf/src/controller.rs::set_input_mode` | `controller.rs::alt_f1_switches_between_direct_mode_and_input_mode` |
| **Alt+F2** | 切换语言：zh-CN → en-US → ja-JP → zh-CN | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_language_chord` → `controller.rs`（`language_hook`）→ `tip.rs`（`Pending::SwitchLanguage`）→ `pime-core/src/imeswitch.rs::next_language` → `tip.rs::switch_language`（`ActivateLanguageProfile`） | `keymap.rs::alt_f2_is_the_language_shortcut_and_nothing_else_is`、`controller.rs::alt_f2_is_eaten_and_asks_for_the_next_language`、`controller.rs::alt_f2_stays_with_the_host_in_a_secure_field`、`pime-core/src/imeswitch.rs::the_switch_key_walks_through_the_three_languages` |
| **Ctrl+Alt+Space** | 显式组合串：把当前输入当成候选来选 | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_compose_chord` → `controller.rs` | `keymap.rs::only_ctrl_alt_space_is_the_compose_chord`；`docs/IME_TEST_MATRIX.md` 的 `IME-S08` |
| **Ctrl+F1** / **Ctrl+Shift+F1** | F1 AI 面板：给光标所在作用域（或选区）加注释、整文件修改、光标处插入 | 编辑器级 | `editor-bridges/vscode/package.json`（`programmingIme.f1Edit`）→ `src/f1.js`、`src/ui/f1panel.js`、`src/core/scope.js`、`src/core/annotate.js` | `docs/F1_ANNOTATE.md`（测试 id `ANN-60..85`、`UAT-42..49`） |
| **Ctrl+F2** / **Ctrl+Shift+F2** | F2 提示词：把选中的代码变成 5W1H 提示词 | 编辑器级 | `editor-bridges/vscode/package.json`（`programmingIme.f2Prompt`）→ `src/f2.js`、`src/core/promptcraft.js` | `docs/F2_PROMPT.md` |

## 3. 逐键设计

### 3.1 Ctrl+F12：代码输入 ⇄ 自然语言输入

* **场景**：用户在写代码（出厂模式，代码原样进编辑器），忽然想打一段中文或者日文——按一次 Ctrl+F12，输入法进入**自然语言输入**；
  写完再按一次，回到**代码输入**。
* **归属**：**输入法级**，TIP 的按键入口（`keymap.rs::is_mode_chord` → `controller.rs::set_input_mode`）。
  理由：模式是**文本服务自己的状态**（每个应用一份），只有活动输入法里的 TIP 知道它。
* **为什么托盘不许注册它**：`RegisterHotKey` 是系统级的，它会在键到达文本服务**之前**就把键吞掉——那这个手势就永远不生效。
  托盘因此**释放**了 `Ctrl+F12`（`crates/pime-tray/src/main.rs` 里没有它的 `RegisterHotKey`）。
* **和「切换输入法」的关系**：「切换输入法」（IME1.0 ⇄ 用户刚才用的那个）这个动作还在，但**没有快捷键了**：
  托盘右键菜单的第一项与激光面板的发光口点击都做它。**D-95 待你裁定**：要不要给它一个键。
* **安全字段**（密码框，`IS_PASSWORD`）里**不吃**这个键：那里什么都不收集，键留给宿主。
* 面板里的「输入模式」那一行是同一个动作的第二条路，走的是同一份状态。

### 3.2 Alt+F1：同一切换的第二条路

* **代码输入**（出厂模式）：输入法一个字符都不吃，代码原样进编辑器；候选窗只是**预览**，不接管任何键。
* **自然语言输入**：主流输入法行为——输入开组合，候选窗接管数字、方向键、Tab、空格。
* **默认值是代码输入**（`TipConfig::input_mode = false`、`LanguageSettings::input_mode = false`），
  设置文件里的 `inputMode` 是「**默认**进入自然语言输入」那个复选框；**v1 的设置文件里的 `true` 视为旧出厂值**
  （迁移到 v2 时不再读它，见 `imesettings.rs::language_settings_of`）。
* **安全字段**里不吃这两个键：那里什么都不收集，键留给宿主。
* 面板里也有同一个开关（`input_mode` 那一行），两条路走的是同一份状态。

### 3.3 Alt+F2：切换语言

* 三个语言档（zh-CN / en-US / ja-JP）按顺序循环，走一圈回来。
* 判定是纯函数 `pime_core::imeswitch::next_language`，Windows 部分只负责 `ActivateLanguageProfile`。
* **为什么不是 Ctrl+F12**：Ctrl+F12 换的是**输入模式**（代码输入 ⇄ 自然语言输入），Alt+F2 换的是**我们自己的语言档**（设置跟着变，引擎不变）。
* 面板的「语言」一行是同一个动作的第二条路。

### 3.4 Ctrl+F1 / Ctrl+F2：编辑器里的 AI

* 设计、设置、边界条件见 `docs/F1_ANNOTATE.md` 与 `docs/F2_PROMPT.md`；本文只归档**快捷键层面的约定**。
* 两个键都用 `Ctrl` 变体，**不抢** `F1`（命令面板）与 `F2`（重命名）。
* 绑定可以搬走：`programmingIme.f1Binding` / `programmingIme.f2Binding` = `ctrl+f1` / `ctrl+shift+f1` / `none`（F2 同理）。

## 4. 冲突与处置

| 冲突 | 处置 |
|---|---|
| VS Code 的 `Ctrl+F12` =「转到实现」 | **不再冲突**（2026-11-11）。托盘不再注册它，这个键到得了文本服务，VS Code 的「转到实现」也回来了。代价：「切换输入法」失去了快捷键（**D-95**） |
| IntelliJ 等的 `Alt+F1` =「选择打开的文件」 | **接受**。这是主流输入法键位（直输/中英文），用户可以在设置里改输入法的行为，但键位本身不动 |
| 纯 `F1` / 纯 `F2` | **不占**。AI 一律用 `Ctrl` 变体 |
| `Ctrl+Alt+Space` 在部分程序里有别的含义 | 输入法只在**候选窗正在显示**且**不在安全字段**时吃它，其余一律放行 |

## 5. 切换时的悬浮界面（与快捷键直接相关）

切输入法 / 切语言之后，屏幕上**不能留下**输入法的东西（激光界面、菜单、设置窗、候选窗）。这条规则由三个机制共同保证（实现见 `crates/pime-tsf/src/tip.rs`）：

1. **TSF 停用我们**（`Deactivate`）→ 全部收工。
2. **别的输入法接管**（`ITfActiveLanguageProfileNotifySink::OnActivated`）→ 全部收工。
3. **我们自己发起切换**（发光口点击、Alt+F2）→ **先收工再切**：TSF 会在我们自己的回调里同步回调我们，此时状态被借用，收工动作做不了，所以必须提前做。

## 6. 未覆盖与待办

* **真实宿主里一次都没跑过**：`Ctrl+F12`、`Alt+F1`、`Alt+F2` 在真实 TSF 宿主里的行为只有单元测试覆盖（`docs/TSF_VM_TEST_PLAN.md` 有验证步骤）。
* `Ctrl+Alt+V`（语音输入）没有单元测试：它要麦克风和 AI 后端。
* **「切换输入法」没有快捷键了**（2026-11-11 的裁定把 `Ctrl+F12` 给了模式切换）：它只在**托盘右键菜单**和**激光面板的发光口点击**里。
  **D-95 待你裁定**：要不要给它一个键（例如 `Ctrl+Space` 或 `Ctrl+F12` 之外的别的），以及是否要加一个「托盘→文本服务」的转发通道。
* 系统级的键（`Ctrl+Alt+V`）的键位写死在代码里，**没有设置项**；编辑器侧的两个键有。

# 快捷键设计：Ctrl+F12 / Ctrl+F1 / Ctrl+F2 / Alt+F1 / Alt+F2

> 本文是产品**所有快捷键**的唯一设计档案：每一个键的归属层、作用、实现位置、冲突与处置、以及覆盖它的测试。
> 实现改了先改这里；本文与代码不一致时，以本文为准并立即修正代码或本文。

## 1. 分层原则

产品的快捷键分两层，**同一层内不重叠**：

| 层 | 谁注册 | 何时有效 | 为什么要这一层 |
|---|---|---|---|
| **系统级** | 托盘进程 `pime-tray`（`RegisterHotKey`） | 始终有效，**哪怕 IME1.0 现在不是活动输入法** | 「切换输入法」「语音输入」这类手势的意义就是：用户正在别的输入法里，想写代码 |
| **编辑器级** | VS Code 扩展 `programmingIme` | 扩展启用后、编辑器有焦点时 | AI 面板跟着编辑器走，扩展才有上下文、Diff 和授权 |
| **输入法级** | 系统输入法本体（TIP，`pime-tsf`） | 只有 IME1.0 是活动输入法时 | 直输/输入模式、显式组合串这类手势只有在输入法里才有意义 |

由此得到两条硬规则：

1. **不抢编辑器的原生键。** 纯 `F1` 是命令面板（帮助），纯 `F2` 是重命名符号，所以 AI 面板用 `Ctrl+F1` / `Ctrl+F2`；纯 `F12` 在 VS Code 里是「转到定义」，所以「切换输入法」用 `Ctrl+F12`（系统级，`RegisterHotKey` 会吞掉它，见 §4 的冲突说明）。
2. **能关就关。** 编辑器侧的键可以用 `programmingIme.f1Binding` / `programmingIme.f2Binding` 换成 `ctrl+shift+f1` / `ctrl+shift+f2` 或 `none`；系统侧的键目前写死在代码里（登记为待办，见 §6）。

## 2. 总表

| 快捷键 | 作用 | 层 | 实现位置 | 覆盖它的测试 |
|---|---|---|---|---|
| **Ctrl+F12** | 切换输入法：IME1.0 ⇄ 用户刚才用的那个 | 系统级 | `crates/pime-tray/src/main.rs`（`HOTKEY_SWITCH`, `MOD_CONTROL`, `0x7B`）→ `crates/pime-tsf/src/ime_switch.rs::switch_now` | `pime-core/src/imeswitch.rs`：`the_key_comes_to_us_when_another_input_method_is_active`、`the_key_goes_back_to_the_input_method_we_came_from`；`crates/pime-tsf/src/ime_switch.rs`：`a_guid_survives_the_round_trip_through_its_text` |
| **Ctrl+Alt+V** | 语音输入：录音、识别、写入当前文档 | 系统级 | `crates/pime-tray/src/main.rs`（`HOTKEY_VOICE`） | 见 §6（未覆盖） |
| **Alt+F1** | 直输模式 ⇄ 输入模式 | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_input_mode_chord` → `crates/pime-tsf/src/controller.rs::set_input_mode` | `controller.rs::alt_f1_switches_between_direct_mode_and_input_mode` |
| **Alt+F2** | 切换语言：zh-CN → en-US → ja-JP → zh-CN | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_language_chord` → `controller.rs`（`language_hook`）→ `tip.rs`（`Pending::SwitchLanguage`）→ `pime-core/src/imeswitch.rs::next_language` → `tip.rs::switch_language`（`ActivateLanguageProfile`） | `keymap.rs::alt_f2_is_the_language_shortcut_and_nothing_else_is`、`controller.rs::alt_f2_is_eaten_and_asks_for_the_next_language`、`controller.rs::alt_f2_stays_with_the_host_in_a_secure_field`、`pime-core/src/imeswitch.rs::the_switch_key_walks_through_the_three_languages` |
| **Ctrl+Alt+Space** | 显式组合串：把当前输入当成候选来选 | 输入法级 | `crates/pime-tsf/src/keymap.rs::is_compose_chord` → `controller.rs` | `keymap.rs::only_ctrl_alt_space_is_the_compose_chord`；`docs/IME_TEST_MATRIX.md` 的 `IME-S08` |
| **Ctrl+F1** / **Ctrl+Shift+F1** | F1 AI 面板：给光标所在作用域（或选区）加注释、整文件修改、光标处插入 | 编辑器级 | `editor-bridges/vscode/package.json`（`programmingIme.f1Edit`）→ `src/f1.js`、`src/ui/f1panel.js`、`src/core/scope.js`、`src/core/annotate.js` | `docs/F1_ANNOTATE.md`（测试 id `ANN-60..85`、`UAT-42..49`） |
| **Ctrl+F2** / **Ctrl+Shift+F2** | F2 提示词：把选中的代码变成 5W1H 提示词 | 编辑器级 | `editor-bridges/vscode/package.json`（`programmingIme.f2Prompt`）→ `src/f2.js`、`src/core/promptcraft.js` | `docs/F2_PROMPT.md` |

## 3. 逐键设计

### 3.1 Ctrl+F12：切换输入法

* **场景**：用户正在微软拼音或日文输入法里写代码，想切到 IME1.0；写完再按一次切回来。
* **归属**：系统级，**托盘进程**注册。理由：切换的那一刻 IME1.0 往往**不是**活动输入法——如果由 TIP 注册 `ITfKeystrokeSink`，键根本到不了我们手里。托盘常驻，所以键一直有效。
* **决策**：纯函数在 `pime-core/src/imeswitch.rs`：
  * 现在是别人的输入法 → 切到我们的 profile（优先与当前语言相同的那一个，否则第一个）。
  * 现在是我们的 → 切回**上一个** profile（记在 `HKCU\Software\IME1.0\LastProfile`）。
  * 没有可去的地方 → **什么都不做**（绝不让用户没有输入法可用）。
* **副作用**：切走时 `Deactivate` 会被调用，悬浮界面必须**立刻**消失（见 §5）。

### 3.2 Alt+F1：直输模式 ⇄ 输入模式

* **直输模式**：输入法一个字符都不吃，代码原样进编辑器。
* **输入模式**：主流输入法行为——输入开组合，候选窗接管数字、方向键、Tab、空格。
* **默认值是输入模式**（`TipConfig::input_mode = true`），直输是按需切换的。
* **安全字段**（密码框，`IS_PASSWORD`）里**不吃**这个键：那里什么都不收集，键留给宿主。
* 面板里也有同一个开关（`input_mode` 那一行），两条路走的是同一份状态。

### 3.3 Alt+F2：切换语言

* 三个语言档（zh-CN / en-US / ja-JP）按顺序循环，走一圈回来。
* 判定是纯函数 `pime_core::imeswitch::next_language`，Windows 部分只负责 `ActivateLanguageProfile`。
* **为什么不是 Ctrl+F12**：Ctrl+F12 换的是**输入法**（整套引擎），Alt+F2 换的是**我们自己的语言档**（设置跟着变，引擎不变）。
* 面板的「语言」一行是同一个动作的第二条路。

### 3.4 Ctrl+F1 / Ctrl+F2：编辑器里的 AI

* 设计、设置、边界条件见 `docs/F1_ANNOTATE.md` 与 `docs/F2_PROMPT.md`；本文只归档**快捷键层面的约定**。
* 两个键都用 `Ctrl` 变体，**不抢** `F1`（命令面板）与 `F2`（重命名）。
* 绑定可以搬走：`programmingIme.f1Binding` / `programmingIme.f2Binding` = `ctrl+f1` / `ctrl+shift+f1` / `none`（F2 同理）。

## 4. 冲突与处置

| 冲突 | 处置 |
|---|---|
| VS Code 的 `Ctrl+F12` =「转到实现」 | **接受**。`RegisterHotKey` 是系统级的，托盘在运行时该键到不了 VS Code。换来的是「在任何程序里都能一键切到 IME1.0」。托盘退出后该键自动还给编辑器 |
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
* 系统级三个键（`Ctrl+F12`、`Ctrl+Alt+V`）的键位写死在代码里，**没有设置项**；编辑器侧的两个键有。
* 「切换输入法」与 VS Code 的 `Ctrl+F12` 冲突（§4），如果用户反馈，可考虑改成 `Ctrl+Space` 之类的键，并同样给出设置项。

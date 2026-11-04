# 输入法功能测试矩阵（IME Test Matrix）

> 本文件回答一个问题：**输入法的各个功能，到底有没有被测到？**
>
> 与 VS Code 桥接侧的 UAT（`02_Verification/uat/`，52 个场景）互补：UAT 测的是「编辑器里用起来对不对」，
> 本矩阵测的是**输入法本体**（TSF 文本服务）自己的功能。

---

## 1. 四层测试

| 层 | 在哪 | 跑什么 | 需要 Windows 桌面 | 登记为 |
|---|---|---|---|---|
| **L1 单元** | `crates/pime-tsf/src/*.rs` 的 `#[cfg(test)]` | 每个模块自己的逻辑（键位翻译、布局算法、菜单、注册计划……） | 否 | `cargo test` |
| **L2 场景** | `01_Source/tests/tsf/ime_scenarios.rs` | 把文本服务当整体驱动：**打字 → 候选窗 → 组合串 → 提交 → 失败降级** | 否 | `cargo test --test ime_scenarios` |
| **L3 宿主** | `01_Source/tests/tsf/ime_host.rs` | 真 COM 类对象、真 msctf、真文档上下文、真按键链 | 是（**要开窗**） | `PIME_IME_LIVE=1 cargo test --test ime_host` |
| **L4 真机 UI** | `crates/pime-tsf/examples/ime_ui_probe.rs` + `tests/smoke/ime-ui-capture.ps1` + `tests/smoke/ime-ui-check.js` | 候选窗 / 悬浮栏 / 设置窗的**真实画面**：探针真开窗 → .NET 抓屏 → Node 解析 PNG | 是（要开窗，**opt-in**） | `node tests/run-all.js --tsf-ui` |

L1 已经在门禁链里（`node tests/run-all.js --tsf`）。本轮补上的是 **L2 / L3 / L4 的取证链路**，并给 L1+L2 建了一份**不会烂掉的账本**。

---

## 2. 功能清单与覆盖

「覆盖」列里的名字是**真实的测试名**：`ime_functions.rs` 会去源码里核对这个测试函数确实存在，
所以账本写错、测试被删、函数改名，都会让门禁变红。

| 功能 ID | 功能 | 模块 | 覆盖它的测试 | 层次 |
|---|---|---|---|---|
| F-ACTIVATE | 激活 / 停用 / 焦点切换不留痕迹 | controller | `IME-S13` `IME-S15` + `deactivate_leaves_no_window_and_no_open_composition` | L1+L2 |
| F-INPUT-MODE | 直打 / 输入模式切换（Alt+F1、悬浮栏按钮） | controller | `IME-S10` + `alt_f1_switches_between_direct_mode_and_input_mode` | L1+L2 |
| F-OBSERVE | 键位只观察不吞（直打模式） | controller | `IME-S01` + `typing_code_with_the_defaults_eats_and_commits_nothing` | L1+L2 |
| F-CANDIDATE-WINDOW | 候选窗的行、标签、选中项、页脚 | controller | `IME-S01` + `typing_shows_a_window_of_labelled_rows_without_eating_anything` | L1+L2 |
| F-LAYOUT | 候选窗位置（光标下方 / 上翻 / 夹在工作区） | layout | `ut_055_flips_above_the_caret_when_the_bottom_edge_is_too_close` | L1 |
| F-PAGING | PageUp / PageDown 翻页 | controller | `IME-S02` + `page_down_moves_the_page_and_updates_the_window` | L1+L2 |
| F-SELECT | 数字 / Enter / Space / Tab 选词 | controller | `IME-S03` `IME-S04` `IME-S06` `IME-S07` + `ut_009_a_digit_commits_the_page_relative_candidate_exactly_once` | L1+L2 |
| F-CANCEL | Esc 取消且不写一个字 | controller | `IME-S05` + `ut_004_esc_cancels_and_writes_nothing` | L1+L2 |
| F-COMPOSE | 显式组合串（Ctrl+Alt+Space）与自动组合 | controller | `IME-S08` `IME-S09` `IME-S16` + `ctrl_alt_space_with_candidates_starts_an_explicit_composition` | L1+L2 |
| F-SHIFT-TAP | Shift 轻点暂停 / 恢复 | keymap | `IME-S11` + `ut_009_shift_tap_pauses_and_resumes_and_hides_the_window` | L1+L2 |
| F-SECURE | 密码框 / 安全桌面不采集 | controller | `IME-S12` + `ut_005_a_secure_field_shows_nothing_and_eats_nothing` | L1+L2 |
| F-DEGRADE | 宿主拒写 / 文本不匹配时交还键盘 | controller | `IME-S14` + `ut_064_a_failed_commit_hands_the_key_to_the_host_and_everything_passes_after` | L1+L2 |
| F-HOST-ACTION | 宿主（或用户）对候选列表的动作 | controller | `IME-S17` + `the_behaviour_interface_reports_the_users_choice_to_the_text_service` | L1+L2 |
| F-UI-ELEMENT | 候选列表作为 UI 元素发布给宿主 | uielement | `IME-S18` + `begin_when_a_list_appears_update_on_every_visible_change_end_when_it_hides` | L1+L2 |
| F-POPUP | 原生候选窗的绘制与 DPI | popup | `the_candidate_list_is_drawn_at_the_dpi_of_the_coordinate_space_it_lives_in` | L1 |
| F-FLOATING | 悬浮栏（按钮 / 布局 / 皮肤） | floating | `the_bar_shows_the_product_the_language_and_the_mode` | L1 |
| F-MENU | 菜单（筛选 / 布局 / 命中） | menus | `a_click_finds_the_row_under_it` | L1 |
| F-SETTINGS-WINDOW | 设置窗口布局 | settings_window | `the_controls_do_not_overlap_each_other` | L1 |
| F-SETTINGS-STORE | 设置持久化（每语言 / 全局 / 坏文件） | imesettings | `a_broken_file_does_not_take_the_input_method_down` | L1 |
| F-REGISTER | 注册 / 卸载 / 回滚 | register | `a_failure_at_every_step_rolls_everything_back_to_the_start` | L1 |
| F-RESOLVE | 文件语言解析（扩展名 / 编辑器 id / 手动锁） | resolve | `IME-S19` | L2 |
| F-ENGINE | 候选生成、校验、排序 | predict | `IME-S20` | L2 |
| F-PLUGIN-LANGUAGE | 插件语言（可开可关） | languages | `IME-S21` | L2 |

**23 个功能，全部有测试。** 账本文件：`01_Source/tests/tsf/common/scenarios.rs` 的 `IME_FUNCTIONS`。

---

## 3. 场景清单（L2）

| ID | 场景 | 关键断言 |
|---|---|---|
| IME-S01 | 打字出候选窗，字符原样进文档 | 文档=`fn`、5 行、3 页、**零吞键** |
| IME-S02 | PageDown / PageUp 翻页 | 第 2 页第一行是第 6 个候选 |
| IME-S03 | 数字选词 | 提交 `c1:fn`，文档被替换 |
| IME-S04 | Enter 选词 | 提交高亮项 |
| IME-S05 | Esc 取消 | 一个字都没写，窗口关闭 |
| IME-S06 | Tab 选词（默认） | Tab 被吃掉并提交 |
| IME-S07 | 「Tab 归宿主」设置 | Tab 回到宿主，不提交 |
| IME-S08 | Ctrl+Alt+Space 打开显式组合串 | 开了 1 个组合串，和弦被吃 |
| IME-S09 | 没有候选时和弦归宿主 | 和弦没被吃 |
| IME-S10 | Alt+F1 切换输入模式 | 两次回到直打模式 |
| IME-S11 | Shift 轻点暂停 | 暂停、无候选窗、Shift 不被吃 |
| IME-S12 | 密码框 | 无窗口、**引擎一次都没被问** |
| IME-S13 | 焦点切换 | 窗口关闭、不降级 |
| IME-S14 | 宿主拒写 | 降级、数字回到宿主 |
| IME-S15 | 停用 | 窗口关闭 |
| IME-S16 | 宿主单方面结束组合串 | 组合串被关掉 |
| IME-S17 | 宿主取某一行 | 提交第 2 行 |
| IME-S18 | 宿主取消列表 | 不写、窗口关 |
| IME-S19 | 文件语言决定查询语言 | 引擎被以 `plsql` 询问 |
| IME-S20 | 不可插入的候选不被数字取走 | 数字回到宿主 |
| IME-S21 | 插件语言 | 引擎被以 `cypher` 询问，候选窗出现 |

---

## 4. 怎么跑

```powershell
# 环境（E 盘空间紧张，构建产物一律写 D 盘）
$env:PATH="C:\Users\seiko\.cargo\bin;C:\msys64\mingw64\bin;"+$env:PATH
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"; $env:CARGO_TARGET_DIR="D:\cargo-target-ime"

cd E:\IME1.0\01_Source\crates\pime-tsf

cargo test                      # L1 + L2 + L3（不含 live）
cargo test --test ime_scenarios # 只跑场景套件
cargo test --test ime_functions # 只跑功能账本
$env:PIME_IME_EVIDENCE_DIR="E:\IME1.0\02_Verification\ime"; cargo test --test ime_scenarios   # 落证据
$env:PIME_IME_LIVE="1"; cargo test --test ime_host -- --nocapture    # L3：真 msctf（会开候选窗）
```

证据（每个场景一个 JSON）落在 `PIME_IME_EVIDENCE_DIR`；门禁链**不**设置该变量，所以普通跑不写文件。

L4（会开窗，会在桌面上短暂出现候选窗 / 悬浮栏）：

```powershell
cd E:\IME1.0
node 01_Source\tests\run-all.js --tsf-ui
# 产出 02_Verification/ime-ui/ime-windows.png，并由 ime-ui-check.js 断言
# 尺寸 1920x1080、颜色数 ≥ 50（拒绝纯色图）
```

> **L4 的判据只到「不是一张纯色图」这一层**：它证明「探针真的把窗口画出来了、桌面被采到了」，
> **不**证明画面里的每个像素都对。像素级/文案级断言要等有人看图或做模板匹配，见 §5。
---

## 5. 还没做的（诚实登记）

| 项 | 状态 | 说明 |
|---|---|---|
| **L4 原生窗口 UI 测试** | **部分** | 已建：探针真开候选窗 / 悬浮栏 / 设置窗 → .NET 抓屏 → Node 解析 PNG（1920×1080、颜色数 ≥ 50，拒绝纯色图），证据 `02_Verification/ime-ui/ime-windows.png`。**但判据只到「不是纯色图」**：没有像素/文案级断言，**设置窗本次没有进入取景画面**，也没有人点过这些窗。下一层要靠模板匹配或人工目视（`docs/MANUAL_E2E.md`）。 |
| **L3 真机注册态** | **未做** | `examples/tsf_host.rs --profile` 能证明 msctf 加载了已安装 DLL，但**按键不路由**（控制台宿主没有带焦点的窗口，TSF 只把文本服务挂到**有焦点**的上下文上）。要在记事本/VS Code 里实测，需要注册态 + 一个真窗口宿主。 |
| **L3 未注册态全链** | **部分** | `ime_host.rs` 已能创建真 COM 对象、走真 msctf、真文档上下文（`PIME_IME_LIVE=1`），但**按键是否真路由到文本服务**要看该环境下的实测结果，不能预先断言。 |
| **TSF 侧插件语言开关** | **未做** | Rust 核心已有启用集合与协议 op，但**输入法产品侧**还没有把「插件语言开关」接到设置窗口（现在只有 VS Code 桥接侧有）。 |

---

## 6. 与其它文档的关系

- `docs/MANUAL_E2E.md` —— 人工端到端步骤（本矩阵不替代它）
- `docs/TSF_VM_TEST_PLAN.md` —— 可丢弃 Windows 虚拟机里的注册态实测计划
- `docs/IMPLEMENTATION_MATRIX.md` —— 功能 ↔ 实现 ↔ 测试的追溯表
- `docs/LANGUAGE_PLUGIN_SPEC.md` —— 插件语言的规范（含 §8.3 的已知限制）

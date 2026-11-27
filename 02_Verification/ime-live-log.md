# 真机（live）这条证据的记录

> 这一份只讲**一件事**：你的两条硬不变量（**拼音不与汉字同排**、**代码照原样进编辑器**）在**真实输入法**上的证据，
> 现在在仓库里是什么状态、还差什么、怎么补上。**不夸大，也不含糊**：没有落盘的，就写「没有落盘」。

## 这一步在做什么

`01_Source/tests/tsf/ime_live.rs`（+ `tests/tsf/common/ime_live.rs` 的夹具）是本仓库里**唯一**驱动**真的**输入法的用例：

- 它装在**普通的 Win32 EDIT 控件**里当宿主，通过 **msctf** 激活本产品的文本服务，用**真的键盘事件**打字，
  再逐字读回宿主控件里的文字；
- 它把结果打成一行机器可读的报告：`REPORT mode=... commit=landed write=ok doc_before_click=... doc_after_click=... doc_code=... code_phase=...`；
- 它**断言的正是你的两条**：自然语言上屏后**文档里不能同时出现拼音与那个汉字**（`the_count_of_characters` 增加了汉字，
  而拼音**不与**汉字相邻：`the_pinyin_leaked_into`），代码输入阶段**文档里就是用户打的代码本身**（一字不差）。

**它是 opt-in**：需要 `PIME_IME_LIVE=1`（它会**抢桌面的焦点**、会把输入法的激光界面显示出来），
并且需要**本产品的 zh-CN 语言档已注册在这台机器上**（否则它自己 skip，**什么都不写**）。

## 仓库里现在有什么（按时间）

| 时间 | 发生了什么 | 落盘了吗 |
|---|---|---|
| 2026-11-25 上午 | 跑过一次真机（`PIME_IME_LIVE=1`，普通 EDIT 控件）。**那次跑出来的报告是**：自然语言输入里点候选后 `doc_before_click=""` → `doc_after_click="我"`（**文档里只有那个汉字，拼音不在旁边**）；代码输入阶段文档增量是 `wo`（**代码一字不差**）。 | **没有**。那时候这个用例**还不写文件**（报告只在控制台上），而当天会话的归档里也已经找不到那份输出了。⇒ **这两个数字是对那次运行的陈述，不是仓库里可复核的产物**——这一点必须说清楚。 |
| 2026-11-25 下午 | 给这个用例加了落盘：跑完把报告写进 `02_Verification/ime-live-report.txt`（`ime_live.rs` 里那段写入）。**从这一版起，每一次真机跑都会在仓库里留下文件。** | 机制有了，**文件还没有**（下一次真机跑才会产生）。 |
| 2026-11-27 上午 | 又跑了一次（想补上那份证据）：**9 条用例全部 skip**——理由是「the zh-CN profile of this product is not registered on this machine」。这台机器上产品**没装**（11-25 那次集成测试误把机器上真实的安装卸掉了，见 `DEFECT_SUBMISSION_2026-11-07.md` 的 D-182；2026-11-27 两次尝试安装都因 **UAC 被拒绝**而没装上）。 | **没有**（skip 了就没有报告）。 |

## 所以，现在缺的是什么

**缺的是「一份能复核的真机报告文件」**。别的都不缺：

- **代码层**：两条不变量有单元测试钉着（`crates/pime-tsf/src/tip.rs` 的 `plan_commit` 四个分支：
  `Owned` / `Composition` / `Document` / `Fail`——**定位不到拼音就一个字都不写**），
  `crates/pime-tsf/src/controller.rs` 里也有「自然语言输入替换拼音」「代码输入把字母留给宿主」的回归用例；
- **门禁层**：整轮门禁（2026-11-27）18 步全绿，TSF **402** 项、core **462** 项；
- **真机层**：**用例是好的、机制是好的，缺的是「机器上装上产品 + 跑一次」**。

## 怎么补上（三步，五分钟）

1. **装上 v0.3.4**（`ProgrammingIME-0.3.4-win-x64-setup.exe`，需要管理员），
   并把 `%LOCALAPPDATA%\IME1.0-dev16\pime_tsf.dll` 换成 **0.3.4** 那一份（那是**每用户 COM 覆盖**，优先于装出来的那一份）；
2. **注销或重启**（换 DLL 必须重开应用）；
3. 跑这一条（**它会抢桌面的焦点**：那几秒别动鼠标键盘）：
   ```
   set PIME_IME_LIVE=1
   node 01_Source/tests/run-all.js --tsf-live
   ```
   （或直接 `cargo test --manifest-path 01_Source/crates/pime-tsf/Cargo.toml --test ime_live -- --nocapture --test-threads=1`）

跑完看两处：
- `02_Verification/ime-live-report.txt` —— **这一次有文件了**（就是上面那张表里缺的那一格）；
- 控制台里那行 `REPORT ...`：应当看到 `doc_after_click="我"`（拼音不在旁边）与 `doc_code` / `doc_code_delta` 是代码本身。

> `node 01_Source/scripts/release-preflight.js` 也会报这件事：
> 机器上装了就报「the live step can run on this machine」，没装就报「the live step cannot run on this machine」。

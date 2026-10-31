//! The interface texts of `core/i18n.js` (`S` and `ERR`), transcribed mechanically in the same order. A row is
//! `[en-US, zh-CN, ja-JP]` (the column order of the reference, which is NOT the order of `LOCALES`).
//! `tests/rust/parity_cfg.rs` compares every key and every locale with `tests/fixtures/i18n-cases.json`, which the JS
//! module generates; when a text changes there, regenerate the fixture and update this table.

/// Interface strings by key; `{0}`, `{1}`, ... are placeholders filled by `i18n::text`.
pub(crate) const S: &[(&str, [&str; 3])] = &[
    ("status.off", ["OFF", "关闭", "オフ"]),
    ("status.paused", ["paused", "已暂停", "一時停止"]),
    (
        "status.tooltip",
        [
            "Programming IME menu: language, pause, AI, usage, permissions, diagnostics",
            "Programming IME 菜单：语言、暂停、AI、用量、权限、诊断",
            "Programming IME メニュー：言語、一時停止、AI、使用量、権限、診断",
        ],
    ),
    ("ai.off", ["AI off", "AI 关闭", "AI オフ"]),
    ("ai.on", ["AI on", "AI 开启", "AI オン"]),
    ("ai.paused", ["AI paused", "AI 暂停", "AI 一時停止"]),
    (
        "menu.title",
        ["Programming IME", "Programming IME", "Programming IME"],
    ),
    (
        "menu.language",
        ["Programming language…", "编程语言…", "プログラミング言語…"],
    ),
    (
        "menu.pause",
        ["Pause suggestions", "暂停建议", "候補を一時停止"],
    ),
    (
        "menu.resume",
        ["Resume suggestions", "恢复建议", "候補を再開"],
    ),
    (
        "menu.usage",
        ["AI usage and budget", "AI 用量与预算", "AI 使用量と予算"],
    ),
    (
        "menu.permissions",
        ["Manage AI permissions…", "管理 AI 授权…", "AI 権限の管理…"],
    ),
    (
        "menu.diagnostics",
        ["Show diagnostics", "查看诊断", "診断を表示"],
    ),
    ("menu.f1", ["F1 AI edit", "F1 AI 编辑", "F1 AI 編集"]),
    (
        "lang.pick",
        [
            "Programming IME language (Auto follows the editor language id)",
            "Programming IME 语言（自动＝跟随编辑器语言 ID）",
            "Programming IME 言語（自動＝エディターの言語 ID に従う）",
        ],
    ),
    (
        "lang.auto",
        ["Auto (unlock)", "自动（解除锁定）", "自動（ロック解除）"],
    ),
    (
        "lang.fileLock",
        [
            "lock for this file",
            "锁定到当前文件",
            "このファイルにロック",
        ],
    ),
    (
        "lang.globalLock",
        [
            "lock for all files",
            "锁定到全部文件",
            "すべてのファイルにロック",
        ],
    ),
    (
        "lang.uncertain",
        [
            "Detected {0} (uncertain) - keeping {1}",
            "检测到 {0}（不确定）—保持 {1}",
            "{0} を検出（不確実）— {1} を維持",
        ],
    ),
    (
        "cand.pick",
        [
            "{0}: \"{1}\" - local candidates",
            "{0}：\"{1}\" — 本地候选",
            "{0}：「{1}」— ローカル候補",
        ],
    ),
    (
        "cand.none",
        [
            "No candidates for this prefix",
            "此前缀没有候选",
            "この接頭辞に候補はありません",
        ],
    ),
    (
        "cand.stale",
        [
            "The candidate expired because the document changed",
            "文档已变化，候选已过期",
            "ドキュメントが変更されたため候補は失効しました",
        ],
    ),
    (
        "cand.inserted",
        [
            "Inserted (unsaved). Ctrl+Z undoes it.",
            "已插入（未保存）。Ctrl+Z 可撤销。",
            "挿入しました（未保存）。Ctrl+Z で元に戻せます。",
        ],
    ),
    (
        "cand.paused",
        [
            "Suggestions are paused",
            "建议已暂停",
            "候補は一時停止中です",
        ],
    ),
    ("src.rule", ["local rule", "本地规则", "ローカル規則"]),
    ("src.user_rule", ["your rule", "用户规则", "ユーザー規則"]),
    ("src.context", ["nearby code", "附近代码", "近くのコード"]),
    ("src.ai", ["AI", "AI", "AI"]),
    (
        "val.valid",
        ["syntax ok", "语法检查通过", "構文チェック OK"],
    ),
    ("val.verified", ["verified", "已验证", "検証済み"]),
    ("val.unchecked", ["unchecked", "未检查", "未チェック"]),
    ("val.invalid", ["invalid", "无效", "無効"]),
    (
        "val.context_missing",
        ["context missing", "缺少上下文", "コンテキスト不足"],
    ),
    (
        "consent.full",
        [
            "Send the COMPLETE current document (including unsaved edits) to {0}?",
            "把【整个当前文档】（含未保存修改）发送到 {0}？",
            "【現在のドキュメント全体】（未保存の変更を含む）を {0} に送信しますか？",
        ],
    ),
    (
        "consent.nearby",
        [
            "Send a nearby code excerpt to {0}?",
            "把光标附近的代码片段发送到 {0}？",
            "カーソル付近のコード断片を {0} に送信しますか？",
        ],
    ),
    (
        "consent.both",
        [
            "Send the complete document and a nearby excerpt to {0}?",
            "把整个文档和附近片段发送到 {0}？",
            "ドキュメント全体と近くの断片を {0} に送信しますか？",
        ],
    ),
    (
        "consent.risk",
        [
            "Check the code for secrets first. The provider receives this data; it is not sent anywhere else.",
            "请先确认代码中没有密钥。数据只发送给该服务商，不会发往其他地方。",
            "先にコードに秘密情報がないか確認してください。データはこのプロバイダーにのみ送信されます。",
        ],
    ),
    (
        "consent.once",
        [
            "Authorize this request",
            "仅授权本次请求",
            "このリクエストのみ許可",
        ],
    ),
    (
        "consent.workspace",
        [
            "Authorize for this workspace (24 h)",
            "授权当前工作区（24 小时）",
            "このワークスペースを許可（24 時間）",
        ],
    ),
    (
        "consent.sensitive",
        [
            "Possible secrets found ({0}). Redact them before sending?",
            "发现疑似密钥（{0}）。发送前是否脱敏？",
            "機密情報の可能性があります（{0}）。送信前に伏せ字にしますか？",
        ],
    ),
    (
        "consent.redact",
        ["Redact and send", "脱敏后发送", "伏せ字にして送信"],
    ),
    (
        "consent.raw",
        [
            "Send unredacted (I accept the risk)",
            "不脱敏发送（我接受风险）",
            "伏せ字にせず送信（リスクを承知）",
        ],
    ),
    (
        "consent.untrusted",
        [
            "This workspace is not trusted, so AI is disabled.",
            "当前工作区不受信任，AI 已禁用。",
            "このワークスペースは信頼されていないため AI は無効です。",
        ],
    ),
    (
        "consent.noKey",
        [
            "Configure a provider, model and API key first (Programming IME: Configure AI Provider).",
            "请先配置服务商、模型和 API Key（Programming IME：配置 AI 服务商）。",
            "先にプロバイダー、モデル、API キーを設定してください。",
        ],
    ),
    (
        "consent.blockedFile",
        [
            "This file looks sensitive ({0}); nothing was sent.",
            "此文件疑似敏感（{0}），未发送任何内容。",
            "このファイルは機密の可能性があります（{0}）。何も送信していません。",
        ],
    ),
    (
        "perm.title",
        [
            "AI permissions - choose one to revoke",
            "AI 授权 — 选择一项撤销",
            "AI 権限 — 取り消す項目を選択",
        ],
    ),
    (
        "perm.none",
        [
            "No active AI permissions",
            "当前没有有效的 AI 授权",
            "有効な AI 権限はありません",
        ],
    ),
    (
        "perm.revokedAll",
        [
            "All AI permissions revoked",
            "已撤销全部 AI 授权",
            "すべての AI 権限を取り消しました",
        ],
    ),
    (
        "perm.revokeAll",
        ["Revoke all", "全部撤销", "すべて取り消す"],
    ),
    (
        "f1.title",
        [
            "Programming IME - F1 AI edit",
            "Programming IME — F1 AI 编辑",
            "Programming IME — F1 AI 編集",
        ],
    ),
    (
        "f1.intent",
        [
            "What should change? (optional)",
            "想要怎样修改？（可选）",
            "何を変更しますか？（任意）",
        ],
    ),
    (
        "f1.generate",
        ["Generate proposals", "生成建议", "提案を生成"],
    ),
    ("f1.regen", ["Regenerate", "重新生成", "再生成"]),
    ("f1.cancel", ["Cancel", "取消", "キャンセル"]),
    (
        "f1.tab.full",
        ["A. Whole file", "A. 整文件修改", "A. ファイル全体"],
    ),
    (
        "f1.tab.insert",
        [
            "B. Insert at cursor",
            "B. 光标处插入",
            "B. カーソル位置に挿入",
        ],
    ),
    (
        "f1.scope.full",
        [
            "Scope: full buffer (L3), file {0}, version {1}, sha256 {2}",
            "范围：完整缓冲区（L3），文件 {0}，版本 {1}，sha256 {2}",
            "範囲：バッファ全体（L3）、ファイル {0}、バージョン {1}、sha256 {2}",
        ],
    ),
    (
        "f1.scope.insert",
        [
            "Scope: nearby excerpt (L1) around {0}",
            "范围：附近片段（L1），位置 {0}",
            "範囲：近くの断片（L1）、位置 {0}",
        ],
    ),
    (
        "f1.idle",
        [
            "Nothing is written until you press Apply.",
            "在你点击“应用”之前不会写入任何内容。",
            "「適用」を押すまで何も書き込まれません。",
        ],
    ),
    (
        "f1.loading",
        [
            "Generating… (nothing is written)",
            "生成中…（不会写入任何内容）",
            "生成中…（何も書き込まれません）",
        ],
    ),
    (
        "f1.noChange",
        [
            "The model proposed no change.",
            "模型没有提出修改。",
            "モデルは変更を提案しませんでした。",
        ],
    ),
    (
        "f1.hunks",
        [
            "{0} change blocks: select the ones to apply",
            "{0} 个修改块：勾选要应用的",
            "{0} 個の変更ブロック：適用するものを選択",
        ],
    ),
    (
        "f1.hunk",
        [
            "Block {0} (old lines {1}, {2} removed, {3} added)",
            "修改块 {0}（原第 {1} 行，删除 {2} 行，新增 {3} 行）",
            "ブロック {0}（元の {1} 行目、削除 {2}、追加 {3}）",
        ],
    ),
    (
        "f1.preview",
        [
            "Result preview ({0} chars, sha256 {1})",
            "结果预览（{0} 字符，sha256 {1}）",
            "結果プレビュー（{0} 文字、sha256 {1}）",
        ],
    ),
    (
        "f1.openDiff",
        ["Open full Diff", "打开完整 Diff", "完全な Diff を開く"],
    ),
    (
        "f1.applySelected",
        [
            "Apply selected blocks",
            "应用所选修改块",
            "選択したブロックを適用",
        ],
    ),
    (
        "f1.applyInsert",
        ["Apply this insertion", "应用这条插入", "この挿入を適用"],
    ),
    (
        "f1.target",
        [
            "Target: line {0}, column {1}{2}",
            "目标：第 {0} 行，第 {1} 列{2}",
            "対象：{0} 行 {1} 列{2}",
        ],
    ),
    (
        "f1.replaces",
        [
            " (replaces {0} chars)",
            "（替换 {0} 个字符）",
            "（{0} 文字を置換）",
        ],
    ),
    (
        "f1.zeroLength",
        [" (pure insertion)", "（纯插入）", "（純粋な挿入）"],
    ),
    (
        "f1.validation.ok",
        ["Syntax checks passed", "语法检查通过", "構文チェックに合格"],
    ),
    (
        "f1.validation.bad",
        [
            "Syntax check failed: Apply is disabled",
            "语法检查未通过：已禁用应用",
            "構文チェック不合格：適用は無効です",
        ],
    ),
    (
        "f1.validation.warn",
        [
            "The original file already had structural problems; the check is advisory",
            "原文件本身已有结构问题；此检查仅供参考",
            "元のファイルに既に構造上の問題があります。このチェックは参考です",
        ],
    ),
    (
        "f1.stale",
        [
            "The document changed after this proposal. Nothing was written. Generate again.",
            "文档在生成后已变化，未写入任何内容。请重新生成。",
            "提案後にドキュメントが変更されました。何も書き込まれていません。再生成してください。",
        ],
    ),
    (
        "f1.applied",
        [
            "Applied to the editor buffer (not saved). Ctrl+Z undoes it.",
            "已应用到编辑器缓冲区（未保存）。Ctrl+Z 可撤销。",
            "エディターのバッファに適用しました（未保存）。Ctrl+Z で元に戻せます。",
        ],
    ),
    (
        "f1.unavailable.full",
        [
            "Whole-file change is unavailable: {0}",
            "整文件修改不可用：{0}",
            "ファイル全体の変更は利用できません：{0}",
        ],
    ),
    ("f1.error", ["Failed: {0}", "失败：{0}", "失敗：{0}"]),
    (
        "f1.effort",
        ["Effort note: {0}", "Effort 说明：{0}", "Effort の注記：{0}"],
    ),
    (
        "f1.tooLarge",
        [
            "The file exceeds 2 MiB; the whole-file path is disabled (nothing is truncated).",
            "文件超过 2 MiB，已禁用整文件路径（不会截断）。",
            "ファイルが 2 MiB を超えるため全体変更は無効です（切り詰めません）。",
        ],
    ),
    (
        "f1.redacted",
        [
            "{0} secret-like value(s) were redacted before sending and restored locally.",
            "发送前已对 {0} 处疑似密钥脱敏，并在本地还原。",
            "送信前に {0} 件の機密候補を伏せ字にし、ローカルで復元しました。",
        ],
    ),
    (
        "f1.blocked",
        [
            "Sending was blocked: {0}",
            "已阻止发送：{0}",
            "送信をブロックしました：{0}",
        ],
    ),
    (
        "f1.crossFile",
        [
            "{0} proposed change(s) to other files were rejected: only the open file can change.",
            "已拒绝 {0} 项针对其他文件的修改：只允许修改当前文件。",
            "他のファイルへの変更 {0} 件を拒否しました：変更できるのは開いているファイルのみです。",
        ],
    ),
    (
        "f1.ignored",
        [
            "The model reply contained fields that were ignored ({0}). No command was run.",
            "模型回复中的部分字段被忽略（{0}），未执行任何命令。",
            "モデルの応答の一部のフィールド（{0}）は無視されました。コマンドは実行されていません。",
        ],
    ),
    (
        "f1.usage",
        [
            "AI usage: {0} requests, ~{1} tokens{2}",
            "AI 用量：{0} 次请求，约 {1} tokens{2}",
            "AI 使用量：{0} 回、約 {1} トークン{2}",
        ],
    ),
    (
        "f1.noEditor",
        [
            "Open a code file first",
            "请先打开一个代码文件",
            "先にコードファイルを開いてください",
        ],
    ),
    (
        "f1.disabledHere",
        [
            "Programming IME is disabled. Run \"Programming IME: Enable / Disable\".",
            "Programming IME 未启用。请运行“Programming IME: Enable / Disable”。",
            "Programming IME は無効です。「Programming IME: Enable / Disable」を実行してください。",
        ],
    ),
    (
        "f1.ann.tab",
        ["C. Annotate scope", "C. 为范围加注释", "C. 範囲にコメント"],
    ),
    (
        "f1.ann.intent",
        [
            "What should the comments focus on? (optional)",
            "注释要侧重什么？（可选）",
            "コメントで重点を置く点は？（任意）",
        ],
    ),
    ("f1.ann.run", ["Add comments", "添加注释", "コメントを追加"]),
    ("f1.ann.range.one", ["line {0}", "第 {0} 行", "{0} 行目"]),
    (
        "f1.ann.range.many",
        ["lines {0}-{1}", "第 {0}-{1} 行", "{0}-{1} 行目"],
    ),
    ("f1.ann.count.one", ["1 line", "1 行", "1 行"]),
    ("f1.ann.count.many", ["{0} lines", "{0} 行", "{0} 行"]),
    (
        "f1.ann.scope.decl",
        [
            "Scope: {0} ({1}, {2})",
            "范围：{0}（{1}，{2}）",
            "範囲：{0}（{1}、{2}）",
        ],
    ),
    (
        "f1.ann.scope.selection",
        [
            "Selection: {0} ({1})",
            "选区：{0}（{1}）",
            "選択範囲：{0}（{1}）",
        ],
    ),
    (
        "f1.ann.scope.block",
        [
            "Block around the cursor: {0} ({1})",
            "光标所在的代码块：{0}（{1}）",
            "カーソル周辺のブロック：{0}（{1}）",
        ],
    ),
    (
        "f1.ann.scope.file",
        [
            "Whole file: {0} ({1})",
            "整个文件：{0}（{1}）",
            "ファイル全体：{0}（{1}）",
        ],
    ),
    ("f1.ann.crumb.selection", ["selection", "选区", "選択範囲"]),
    ("f1.ann.crumb.block", ["block", "代码块", "ブロック"]),
    ("f1.ann.crumb.file", ["file", "文件", "ファイル"]),
    (
        "f1.ann.crumbs",
        [
            "Scope path, innermost first",
            "范围路径（由内到外）",
            "範囲のパス（内側から）",
        ],
    ),
    ("f1.ann.wider", ["Wider", "更宽", "広げる"]),
    ("f1.ann.narrower", ["Narrower", "更窄", "狭める"]),
    (
        "f1.ann.widerBlocked",
        [
            "The next wider scope ({0}) is too large ({1} characters, {2} lines; the limits are {3} characters and {4} lines).",
            "更宽的范围（{0}）过大（{1} 个字符、{2} 行；上限是 {3} 个字符和 {4} 行）。",
            "次に広い範囲（{0}）は大きすぎます（{1} 文字、{2} 行。上限は {3} 文字と {4} 行）。",
        ],
    ),
    ("f1.ann.opt.comment", ["Comments", "注释", "コメント"]),
    (
        "f1.ann.opt.graph",
        ["Cypher graph", "Cypher 关系图", "Cypher グラフ"],
    ),
    (
        "f1.ann.nothingRequested",
        [
            "Tick Comments or Cypher graph to say what to add.",
            "请勾选“注释”或“Cypher 关系图”，说明要添加什么。",
            "追加する内容として「コメント」または「Cypher グラフ」を選んでください。",
        ],
    ),
    (
        "f1.ann.unsupported",
        [
            "Adding comments to a scope is not available for the language \"{0}\".",
            "语言“{0}”不支持为范围添加注释。",
            "言語「{0}」では範囲へのコメント追加は利用できません。",
        ],
    ),
    (
        "f1.ann.empty",
        [
            "The document is empty: there is nothing to comment.",
            "文档为空：没有可加注释的内容。",
            "ドキュメントが空です。コメントを付ける対象がありません。",
        ],
    ),
    (
        "f1.ann.tooLarge",
        [
            "The scope {0} is too large ({1} characters, {2} lines; the limits are {3} characters and {4} lines). Select a smaller part of the code and press F1 again.",
            "范围 {0} 过大（{1} 个字符、{2} 行；上限是 {3} 个字符和 {4} 行）。请选中更小的一段代码后再按 F1。",
            "範囲 {0} は大きすぎます（{1} 文字、{2} 行。上限は {3} 文字と {4} 行）。より小さいコード範囲を選択してから F1 を押し直してください。",
        ],
    ),
    (
        "f1.ann.fileTooLarge",
        [
            "The file exceeds 2 MiB; adding comments to a scope is disabled (nothing is truncated).",
            "文件超过 2 MiB，已禁用为范围添加注释（不会截断）。",
            "ファイルが 2 MiB を超えるため、範囲へのコメント追加は無効です（切り詰めません）。",
        ],
    ),
    (
        "f1.ann.consent",
        [
            "Send the COMPLETE current file to {0} so the AI can write comments for the chosen scope? Only comment lines are added; your code is never rewritten.",
            "把【整个当前文件】发送到 {0}，让 AI 为所选范围写注释？只会添加注释行，不会改写你的代码。",
            "選択した範囲のコメントを AI に書かせるため、【現在のファイル全体】を {0} に送信しますか？追加されるのはコメント行のみで、コードは書き換えられません。",
        ],
    ),
    (
        "f1.ann.redacted",
        [
            "{0} secret-like value(s) were redacted before sending; a marker in a comment reads <redacted> and is never restored.",
            "发送前已对 {0} 处疑似密钥脱敏；注释中的标记显示为 <redacted>，不会还原。",
            "送信前に {0} 件の機密候補を伏せ字にしました。コメント内のマーカーは <redacted> と表示され、復元されません。",
        ],
    ),
    (
        "f1.ann.summary",
        [
            "Proposed: {0}. Only comment lines are added; select the blocks to apply.",
            "建议：{0}。只会添加注释行；请勾选要应用的修改块。",
            "提案：{0}。追加されるのはコメント行のみです。適用するブロックを選択してください。",
        ],
    ),
    (
        "f1.ann.commentCount",
        ["{0} comment(s)", "{0} 条注释", "{0} 件のコメント"],
    ),
    (
        "f1.ann.nothingToAdd",
        [
            "Nothing to add: no new comment was proposed for this scope.",
            "没有可添加的内容：模型没有为此范围提出新的注释。",
            "追加する内容はありません：この範囲に新しいコメントは提案されませんでした。",
        ],
    ),
    (
        "f1.ann.replyInvalid",
        [
            "The model reply could not be used (it is not the expected JSON). Nothing was changed. Try Regenerate or another model.",
            "无法使用模型的回复（不是要求的 JSON）。未做任何修改。请重新生成或换一个模型。",
            "モデルの応答を使用できませんでした（期待された JSON ではありません）。何も変更していません。再生成するか別のモデルを試してください。",
        ],
    ),
    (
        "f1.ann.unsafe",
        [
            "The comments could not be placed without touching your code, so they were discarded. Nothing was changed.",
            "这些注释无法在不触及代码的前提下放置，已被丢弃。未做任何修改。",
            "コードに触れずにコメントを配置できなかったため、破棄しました。何も変更していません。",
        ],
    ),
    (
        "f1.ann.skippedPosition",
        [
            "{0} comment(s) skipped: not a safe position.",
            "已跳过 {0} 条注释：位置不安全。",
            "{0} 件のコメントをスキップしました：安全な位置ではありません。",
        ],
    ),
    (
        "f1.ann.skippedDuplicate",
        [
            "{0} comment(s) skipped: duplicates of a comment that is already there or already proposed.",
            "已跳过 {0} 条注释：与已有或已提出的注释重复。",
            "{0} 件のコメントをスキップしました：既にある、または既に提案済みのコメントと重複しています。",
        ],
    ),
    (
        "f1.ann.skippedOther",
        [
            "{0} comment(s) skipped: empty, malformed or over the size limits.",
            "已跳过 {0} 条注释：为空、格式错误或超过大小限制。",
            "{0} 件のコメントをスキップしました：空、形式不正、またはサイズ上限超過です。",
        ],
    ),
    (
        "f1.ann.graphEmpty",
        [
            "No Cypher graph comment was added (none returned, or no safe position at the top of the scope).",
            "未添加 Cypher 关系图注释（模型没有返回，或范围顶部没有安全位置）。",
            "Cypher グラフのコメントは追加されませんでした（返されなかったか、範囲の先頭に安全な位置がありません）。",
        ],
    ),
    (
        "ai.configured",
        [
            "API key saved in VS Code SecretStorage. AI stays off until you authorize a transfer.",
            "API Key 已保存到 VS Code SecretStorage。授权发送之前 AI 不会工作。",
            "API キーを SecretStorage に保存しました。送信を許可するまで AI は動作しません。",
        ],
    ),
    (
        "ai.keyDeleted",
        [
            "API key deleted",
            "API Key 已删除",
            "API キーを削除しました",
        ],
    ),
    (
        "ai.realtimeOn",
        [
            "Realtime AI enabled for this workspace and provider host (nearby code may be sent while typing).",
            "已为当前工作区和服务商主机启用实时 AI（键入时可能发送附近代码）。",
            "このワークスペースとプロバイダーでリアルタイム AI を有効にしました（入力中に近くのコードが送信される場合があります）。",
        ],
    ),
    (
        "ai.realtimeOff",
        [
            "Realtime AI disabled",
            "实时 AI 已关闭",
            "リアルタイム AI を無効にしました",
        ],
    ),
    (
        "err.fallback",
        [
            "Local candidates keep working.",
            "本地候选仍可使用。",
            "ローカル候補は引き続き使えます。",
        ],
    ),
    (
        "annotate.progress",
        [
            "AI is writing comments for the inserted code (Cancel inserts it as is)…",
            "AI 正在为插入的代码生成注释（取消则原样插入）…",
            "AI が挿入コードのコメントを作成中です（キャンセルでそのまま挿入）…",
        ],
    ),
    (
        "annotate.rejected",
        [
            "The AI changed the code itself, so its comments were discarded and the original snippet was inserted.",
            "AI 改动了代码本身，已丢弃它的注释，插入的是原始片段。",
            "AI がコード自体を変更したため、そのコメントを破棄し、元のスニペットを挿入しました。",
        ],
    ),
    (
        "annotate.failed",
        [
            "Comments could not be added ({0}); the snippet was inserted as is.",
            "无法添加注释（{0}），已原样插入。",
            "コメントを追加できませんでした（{0}）。そのまま挿入しました。",
        ],
    ),
    (
        "annotate.skipSensitive",
        [
            "This file looks sensitive ({0}); nothing was sent and the snippet was inserted without annotations.",
            "此文件疑似敏感（{0}），未发送任何内容，已不带注释插入。",
            "このファイルは機密の可能性があります（{0}）。何も送信せず、注釈なしで挿入しました。",
        ],
    ),
    (
        "annotate.consent.file",
        [
            "Send the COMPLETE current file to {0} so the AI can annotate the inserted code?",
            "把【整个当前文件】发送到 {0}，让 AI 为插入的代码添加注释？",
            "挿入コードに注釈を付けるため、【現在のファイル全体】を {0} に送信しますか？",
        ],
    ),
    (
        "annotate.consent.graph",
        [
            "Also send the NAMES (not the code) of symbols found in the other files of this folder, for the Cypher relation comment.",
            "同时发送本文件夹其他文件中的符号【名称】（不含代码），用于生成 Cypher 关系注释。",
            "Cypher 関係コメントのため、このフォルダー内の他のファイルのシンボル【名前】（コードは含まない）も送信します。",
        ],
    ),
    (
        "annotate.done",
        [
            "Inserted with {0}.",
            "已插入，并附带{0}。",
            "{0} 付きで挿入しました。",
        ],
    ),
    (
        "annotate.kind.comment",
        ["detailed comments", "详细注释", "詳細コメント"],
    ),
    (
        "annotate.kind.graph",
        [
            "a Cypher relation comment",
            "Cypher 关系注释",
            "Cypher 関係コメント",
        ],
    ),
    (
        "f2.progress",
        [
            "Building the 5W1H prompt with your AI (Cancel leaves the clipboard unchanged)…",
            "正在请你的 AI 生成 5W1H 提示词（取消则剪贴板保持不变）…",
            "AI に 5W1H プロンプトを作成させています（キャンセルするとクリップボードは変わりません）…",
        ],
    ),
    (
        "f2.noOptCode",
        [
            "No optimisation needed: the AI found no verified problem in the selected code. The clipboard was not changed.",
            "无需优化：AI 没有在所选代码中发现经核实的问题。剪贴板未改动。",
            "最適化は不要です：AI は選択したコードに検証済みの問題を見つけませんでした。クリップボードは変更していません。",
        ],
    ),
    (
        "f2.noOptProse",
        [
            "No optimisation needed: the text already answers who, what, when, where, why and how. The clipboard was not changed.",
            "无需优化：这段文字已经回答了谁、做什么、何时、何处、为什么、怎么做。剪贴板未改动。",
            "最適化は不要です：この文章は、誰が・何を・いつ・どこで・なぜ・どのように、にすでに答えています。クリップボードは変更していません。",
        ],
    ),
    (
        "f2.copied",
        [
            "Copied {0} characters: paste it into your AI assistant.",
            "已复制 {0} 个字符：请粘贴到你的 AI 助手。",
            "{0} 文字をコピーしました：AI アシスタントに貼り付けてください。",
        ],
    ),
    (
        "f2.template",
        [
            "Copied an UNREVIEWED template ({0} characters). Nothing was analysed: {1}. Answer the <bracketed> questions, then send it.",
            "已复制【未经审阅】的模板（{0} 个字符）。没有做任何分析：{1}。请先回答尖括号里的问题再发送。",
            "【未レビュー】のテンプレートをコピーしました（{0} 文字）。分析は行っていません：{1}。山括弧内の質問に答えてから送信してください。",
        ],
    ),
    ("f2.why.aiOff", ["AI is off", "AI 已关闭", "AI がオフです"]),
    (
        "f2.why.untrusted",
        [
            "this workspace is not trusted",
            "当前工作区不受信任",
            "このワークスペースは信頼されていません",
        ],
    ),
    (
        "f2.why.noKey",
        [
            "no AI provider, model and key are configured",
            "尚未配置 AI 服务商、模型和 API Key",
            "AI プロバイダー、モデル、API キーが未設定です",
        ],
    ),
    (
        "f2.why.declined",
        [
            "sending was not authorised",
            "没有授权发送",
            "送信が許可されませんでした",
        ],
    ),
    (
        "f2.why.tooLarge",
        [
            "the file is larger than 2 MiB",
            "文件超过 2 MiB",
            "ファイルが 2 MiB を超えています",
        ],
    ),
    (
        "f2.sensitiveBlocked",
        [
            "Looks sensitive ({0}): nothing was sent to an AI and nothing was copied.",
            "疑似敏感内容（{0}）：未向 AI 发送任何内容，也未复制任何内容。",
            "機密の可能性があります（{0}）：AI には何も送信せず、何もコピーしていません。",
        ],
    ),
    (
        "f2.sensitiveName",
        ["sensitive file name", "敏感文件名", "機密ファイル名"],
    ),
    (
        "f2.selectionRequired",
        [
            "Select some text or code first, then run \"F2 Prompt\".",
            "请先选中一段文字或代码，再运行“F2 提示词”。",
            "先にテキストまたはコードを選択してから「F2 プロンプト」を実行してください。",
        ],
    ),
    (
        "f2.selectionTooLarge",
        [
            "The selection has {0} characters; the limit is {1}. Select less.",
            "选区有 {0} 个字符，上限是 {1}。请缩小选区。",
            "選択範囲は {0} 文字で、上限は {1} 文字です。範囲を小さくしてください。",
        ],
    ),
    (
        "f2.replyInvalid",
        [
            "The AI did not return the requested JSON object, so nothing was copied. Try again or choose another model.",
            "AI 没有返回要求的 JSON 对象，所以没有复制任何内容。请重试或换一个模型。",
            "AI が要求された JSON オブジェクトを返さなかったため、何もコピーしていません。再試行するか別のモデルを選んでください。",
        ],
    ),
    (
        "f2.unverified",
        [
            "The AI reported problems, but none of them could be found in your file, so nothing was copied.",
            "AI 报告了问题，但在你的文件中找不到其中任何一条，所以没有复制任何内容。",
            "AI は問題を報告しましたが、どれもファイル内に見つからなかったため、何もコピーしていません。",
        ],
    ),
    (
        "f2.error",
        [
            "F2 prompt failed, the clipboard was not changed: {0}",
            "F2 提示词失败，剪贴板未改动：{0}",
            "F2 プロンプトに失敗し、クリップボードは変更していません：{0}",
        ],
    ),
    (
        "f2.redacted",
        [
            "{0} secret-like value(s) replaced by PIME_REDACTED_n markers (not restored)",
            "{0} 处疑似密钥已替换为 PIME_REDACTED_n 标记（未还原）",
            "{0} 件の機密候補を PIME_REDACTED_n マーカーに置き換え済み（復元されません）",
        ],
    ),
    (
        "engine.status",
        ["core unavailable", "核心不可用", "コア利用不可"],
    ),
    (
        "engine.unavailable",
        [
            "The Programming IME core is not available ({0}). IME suggestions and F1 are turned off; typing in VS Code is not affected. Set programmingIme.coreExecutable to a built pime-core and reload the window, or set programmingIme.engine to \"js\".",
            "Programming IME 核心不可用（{0}）。输入建议和 F1 已停用，VS Code 自身输入不受影响。请把 programmingIme.coreExecutable 设为已构建的 pime-core 并重新加载窗口，或把 programmingIme.engine 设为 \"js\"。",
            "Programming IME のコアを利用できません（{0}）。入力候補と F1 は無効です（VS Code 自体の入力には影響しません）。programmingIme.coreExecutable にビルド済みの pime-core を設定してウィンドウを再読み込みするか、programmingIme.engine を \"js\" にしてください。",
        ],
    ),
    (
        "engine.reload",
        [
            "Reload the window to apply the new Programming IME engine setting.",
            "请重新加载窗口以应用新的 Programming IME 引擎设置。",
            "新しい Programming IME エンジン設定を適用するにはウィンドウを再読み込みしてください。",
        ],
    ),
    (
        "diag.title",
        [
            "Programming IME diagnostics (no source code, prompts or keys are recorded)",
            "Programming IME 诊断（不记录源码、提示词或密钥）",
            "Programming IME 診断（ソースコード、プロンプト、キーは記録されません）",
        ],
    ),
    (
        "sec.confirm",
        [
            "Programming IME is about to read \"{0}\", which looks like a secret file ({1}). Allow it?",
            "Programming IME 将要读取“{0}”，它看起来是密钥类文件（{1}）。是否允许？",
            "Programming IME が「{0}」を読み取ろうとしています。シークレット系のファイルのようです（{1}）。許可しますか？",
        ],
    ),
    (
        "sec.confirm.detail",
        [
            "Reading is local: reading a file sends nothing anywhere. Sending any of it to an AI provider is asked separately, before it happens. Cancel leaves the file unread.",
            "读取只在本机进行：读取文件本身不会向任何地方发送内容。把其中任何内容发送给 AI 服务商，会在发送前单独询问。取消则不读取该文件。",
            "読み取りはローカルで行われ、ファイルを読むだけでは何も送信されません。AI プロバイダーへの送信は、送信の前に別途確認します。キャンセルするとファイルは読み取られません。",
        ],
    ),
    (
        "sec.allowOnce",
        ["Allow once", "仅允许一次", "今回のみ許可"],
    ),
    (
        "sec.allowSession",
        [
            "Allow for this session",
            "本次会话内允许",
            "このセッションで許可",
        ],
    ),
    (
        "sec.notice",
        [
            "\"{0}\" looks like a secret file ({1}), so Programming IME is not reading it: no suggestions here.",
            "“{0}”看起来是密钥类文件（{1}），因此 Programming IME 不读取它：此处没有建议。",
            "「{0}」はシークレット系のファイルのようです（{1}）。そのため Programming IME は読み取らず、ここでは候補を出しません。",
        ],
    ),
    (
        "sec.notice.allow",
        [
            "Allow this file for this session",
            "本次会话内允许此文件",
            "このファイルをこのセッションで許可",
        ],
    ),
    (
        "sec.notice.keepOff",
        ["Keep off", "保持关闭", "読み取らない"],
    ),
    (
        "sec.status.text",
        ["protected file", "受保护文件", "保護ファイル"],
    ),
    (
        "sec.status.tooltip",
        [
            "\"{0}\" looks like a secret file, so Programming IME does not read it. Click to allow it for this session or to manage your decisions.",
            "“{0}”看起来是密钥类文件，因此 Programming IME 不读取它。点击可在本次会话内允许它，或管理你的决定。",
            "「{0}」はシークレット系のファイルのようなので、Programming IME は読み取りません。クリックすると、このセッションで許可するか、決定を管理できます。",
        ],
    ),
    (
        "sec.refused.f1",
        [
            "F1 did not start: Programming IME may not read this secret-class file. Run \"Programming IME: Secret Files\" to allow it.",
            "F1 未启动：Programming IME 不得读取这个密钥类文件。运行“Programming IME：密钥类文件”可允许读取。",
            "F1 を開始しませんでした：Programming IME はこのシークレット系ファイルを読み取れません。「Programming IME: シークレット系ファイル」で許可できます。",
        ],
    ),
    (
        "sec.refused.f2",
        [
            "F2 did not run: Programming IME may not read this secret-class file, and the clipboard was not changed. Run \"Programming IME: Secret Files\" to allow it.",
            "F2 未运行：Programming IME 不得读取这个密钥类文件，剪贴板未改动。运行“Programming IME：密钥类文件”可允许读取。",
            "F2 を実行しませんでした：Programming IME はこのシークレット系ファイルを読み取れず、クリップボードは変更していません。「Programming IME: シークレット系ファイル」で許可できます。",
        ],
    ),
    (
        "sec.refused.candidates",
        [
            "No candidates: Programming IME does not read this secret-class file. Run \"Programming IME: Secret Files\" to allow it.",
            "没有候选：Programming IME 不读取这个密钥类文件。运行“Programming IME：密钥类文件”可允许读取。",
            "候補はありません：Programming IME はこのシークレット系ファイルを読み取りません。「Programming IME: シークレット系ファイル」で許可できます。",
        ],
    ),
    (
        "sec.menu",
        ["Secret files…", "密钥类文件…", "シークレット系ファイル…"],
    ),
    (
        "sec.pick.title",
        [
            "Secret-class files - choose one to forget the decision",
            "密钥类文件 — 选择一项以忘记该决定",
            "シークレット系ファイル — 決定を取り消す項目を選択",
        ],
    ),
    (
        "sec.pick.allowActive",
        [
            "Allow \"{0}\" for this session",
            "本次会话内允许“{0}”",
            "「{0}」をこのセッションで許可",
        ],
    ),
    (
        "sec.pick.allow",
        ["allowed until {0}", "允许至 {0}", "{0} まで許可"],
    ),
    (
        "sec.pick.deny",
        [
            "kept off until {0}",
            "保持关闭至 {0}",
            "{0} まで読み取らない",
        ],
    ),
    (
        "sec.pick.revokeAll",
        [
            "Forget all decisions",
            "忘记全部决定",
            "すべての決定を取り消す",
        ],
    ),
    (
        "sec.pick.none",
        [
            "Nothing to manage: the active file is not secret-class and no decision was made in this session.",
            "没有可管理的内容：当前文件不是密钥类文件，本次会话也没有做过任何决定。",
            "管理する項目はありません：アクティブなファイルはシークレット系ではなく、このセッションで決定も行われていません。",
        ],
    ),
    (
        "sec.allowed",
        [
            "Allowed \"{0}\" for this session.",
            "已在本次会话内允许“{0}”。",
            "「{0}」をこのセッションで許可しました。",
        ],
    ),
    (
        "sec.revoked",
        [
            "Forgot the decision for \"{0}\": it will be asked again.",
            "已忘记对“{0}”的决定：下次会再次询问。",
            "「{0}」の決定を取り消しました。次回は再度確認します。",
        ],
    ),
    (
        "sec.revokedAll",
        [
            "Forgot all decisions: secret-class files will be asked again.",
            "已忘记全部决定：密钥类文件会再次询问。",
            "すべての決定を取り消しました。シークレット系ファイルは再度確認します。",
        ],
    ),
    (
        "sec.kind.env",
        [
            "environment variables file",
            "环境变量文件",
            "環境変数ファイル",
        ],
    ),
    ("sec.kind.private-key", ["private key", "私钥", "秘密鍵"]),
    ("sec.kind.keystore", ["keystore", "密钥库", "キーストア"]),
    (
        "sec.kind.credential-file",
        ["credentials file", "凭据文件", "認証情報ファイル"],
    ),
    (
        "sec.kind.cloud-credentials",
        ["cloud credentials", "云凭据", "クラウド認証情報"],
    ),
    (
        "sec.kind.token-store",
        [
            "token or login store",
            "令牌或登录信息",
            "トークン／ログイン情報",
        ],
    ),
    (
        "sec.kind.infra-state",
        [
            "infrastructure state or variables",
            "基础设施状态或变量",
            "インフラの状態／変数",
        ],
    ),
    (
        "sec.kind.password-db",
        ["password database", "密码库", "パスワードデータベース"],
    ),
    (
        "sec.kind.user-pattern",
        [
            "matches your secretFilePatterns setting",
            "匹配你的 secretFilePatterns 设置",
            "secretFilePatterns 設定に一致",
        ],
    ),
];

/// One human readable sentence per stable error code.
pub(crate) const ERR: &[(&str, [&str; 3])] = &[
    (
        "STALE_DOCUMENT",
        [
            "The document changed; nothing was written.",
            "文档已变化，未写入任何内容。",
            "ドキュメントが変更されたため、何も書き込まれていません。",
        ],
    ),
    (
        "STALE_SELECTION",
        [
            "The cursor or selection moved; the proposal expired.",
            "光标或选区已移动，建议已过期。",
            "カーソルまたは選択範囲が移動したため、提案は失効しました。",
        ],
    ),
    (
        "STALE_RESULT",
        [
            "The result is out of date and was discarded.",
            "结果已过期并被丢弃。",
            "結果は古いため破棄されました。",
        ],
    ),
    (
        "EDIT_UNKNOWN",
        [
            "The edit result is uncertain. Check the file; it will not be retried.",
            "编辑结果不确定。请检查文件；不会自动重试。",
            "編集結果が不確実です。ファイルを確認してください。自動再試行はしません。",
        ],
    ),
    (
        "EDIT_REJECTED",
        [
            "The editor rejected the edit; nothing was written.",
            "编辑器拒绝了此编辑，未写入任何内容。",
            "エディターが編集を拒否しました。何も書き込まれていません。",
        ],
    ),
    (
        "AI_AUTH_FAILED",
        [
            "The provider rejected the API key (401/403).",
            "服务商拒绝了 API Key（401/403）。",
            "プロバイダーが API キーを拒否しました（401/403）。",
        ],
    ),
    (
        "AI_RATE_LIMIT",
        [
            "Provider rate limit: AI is paused for a while.",
            "服务商限流：AI 暂停一段时间。",
            "プロバイダーのレート制限：AI は一時停止します。",
        ],
    ),
    (
        "AI_QUOTA",
        [
            "The provider reports insufficient balance or quota.",
            "服务商提示余额或额度不足。",
            "プロバイダーが残高またはクォータ不足を報告しました。",
        ],
    ),
    (
        "AI_TIMEOUT",
        [
            "The AI request timed out.",
            "AI 请求超时。",
            "AI リクエストがタイムアウトしました。",
        ],
    ),
    (
        "AI_NETWORK",
        [
            "Network failure while contacting the provider.",
            "连接服务商时网络失败。",
            "プロバイダーへの接続でネットワークエラーが発生しました。",
        ],
    ),
    (
        "AI_SERVER_ERROR",
        [
            "The provider returned a server error.",
            "服务商返回服务器错误。",
            "プロバイダーがサーバーエラーを返しました。",
        ],
    ),
    (
        "AI_BAD_REQUEST",
        [
            "The provider refused the request (check model id and endpoint).",
            "服务商拒绝了请求（请检查模型 ID 与端点）。",
            "プロバイダーがリクエストを拒否しました（モデル ID とエンドポイントを確認）。",
        ],
    ),
    (
        "AI_BUDGET_EXCEEDED",
        [
            "AI budget reached; AI is paused, input is unaffected.",
            "已达 AI 预算上限；AI 暂停，输入不受影响。",
            "AI の予算上限に達しました。AI は停止しますが入力には影響しません。",
        ],
    ),
    (
        "AI_DISABLED",
        ["AI is disabled.", "AI 已关闭。", "AI は無効です。"],
    ),
    (
        "AI_CANCELLED",
        [
            "The AI request was cancelled.",
            "AI 请求已取消。",
            "AI リクエストはキャンセルされました。",
        ],
    ),
    (
        "OUTPUT_INVALID",
        [
            "The model reply was invalid and was not shown as an applicable proposal.",
            "模型回复无效，未作为可应用建议显示。",
            "モデルの応答が無効なため、適用可能な提案として表示しませんでした。",
        ],
    ),
    (
        "PATCH_INVALID",
        [
            "The proposal cannot be applied safely (overlapping or invalid ranges).",
            "建议无法安全应用（范围重叠或无效）。",
            "提案を安全に適用できません（範囲の重複または無効）。",
        ],
    ),
    (
        "TARGET_RANGE_INVALID",
        [
            "The target range is invalid.",
            "目标范围无效。",
            "対象範囲が無効です。",
        ],
    ),
    (
        "CROSS_FILE_DENIED",
        [
            "Only the current file may be changed.",
            "只允许修改当前文件。",
            "変更できるのは現在のファイルのみです。",
        ],
    ),
    (
        "VALIDATION_FAILED",
        [
            "The syntax check failed, so Apply is disabled.",
            "语法检查未通过，已禁用应用。",
            "構文チェックに失敗したため適用は無効です。",
        ],
    ),
    (
        "GRANT_DENIED",
        [
            "The authorisation is missing, expired or revoked.",
            "授权缺失、已过期或已被撤销。",
            "許可がない、期限切れ、または取り消されています。",
        ],
    ),
    (
        "CONSENT_REQUIRED",
        [
            "Authorisation is required before sending.",
            "发送前需要授权。",
            "送信前に許可が必要です。",
        ],
    ),
    (
        "SENSITIVE_BLOCKED",
        [
            "The payload looks sensitive; nothing was sent.",
            "内容疑似敏感，未发送。",
            "内容が機密の可能性があるため送信しませんでした。",
        ],
    ),
    (
        "FULL_CONTEXT_REQUIRED",
        [
            "Whole-file edits need the full editor buffer (trusted bridge).",
            "整文件修改需要完整的编辑器缓冲区（受信任桥接）。",
            "ファイル全体の編集にはエディターのバッファ全体（信頼されたブリッジ）が必要です。",
        ],
    ),
    (
        "PAYLOAD_TOO_LARGE",
        [
            "The payload is too large and was not truncated.",
            "内容过大，未做截断。",
            "内容が大きすぎます（切り詰めていません）。",
        ],
    ),
    (
        "DOCUMENT_CLOSED",
        [
            "The document was closed.",
            "文档已关闭。",
            "ドキュメントが閉じられました。",
        ],
    ),
    (
        "MODEL_UNSUPPORTED",
        [
            "Model or API family is not supported or not configured.",
            "模型或 API 类型不受支持或尚未配置。",
            "モデルまたは API 種別が未対応、または未設定です。",
        ],
    ),
    (
        "VAULT_ERROR",
        [
            "No API key is stored for this provider.",
            "该服务商尚未保存 API Key。",
            "このプロバイダーの API キーが保存されていません。",
        ],
    ),
    (
        "SETTINGS_INVALID",
        [
            "A setting is invalid (HTTPS endpoint required).",
            "设置无效（端点必须是 HTTPS）。",
            "設定が無効です（HTTPS エンドポイントが必要）。",
        ],
    ),
    (
        "TOKEN_INVALID",
        [
            "The apply token is invalid or already used.",
            "应用令牌无效或已使用。",
            "適用トークンが無効、または使用済みです。",
        ],
    ),
    (
        "USER_CONFIRMATION_REQUIRED",
        [
            "Explicit confirmation is required.",
            "需要明确确认。",
            "明示的な確認が必要です。",
        ],
    ),
];

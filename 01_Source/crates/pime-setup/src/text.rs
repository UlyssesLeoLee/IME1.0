//! The few strings of the interactive mode, in the three UI languages of the product (zh-CN, en-US, ja-JP).
//! Console output and the log are English only: they are for people who run the setup from a shell.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhCn,
    Ja,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Title,
    ConfirmInstall,
    ConfirmUninstall,
    InstallDone,
    UninstallDone,
    RestartNeeded,
    Failed,
    ElevationDeclined,
    Notes,
    /// The guidance after a successful install: the setup never changes the default input method, so the person has
    /// to add the IME to the language list themselves.
    NextSteps,
    /// The wizard: the pages, the buttons and the options.
    WizardWelcome,
    WizardIntro,
    WizardFolder,
    /// The typed install folder cannot be used (the window says so and stays on the page). The reason itself is a
    /// technical detail and is written under this line, in the language of the installer that wrote it.
    WizardPathInvalid,
    /// The folder the program really goes to, which is not always the folder that was typed: the product folder is
    /// appended to it. `{0}` is that folder.
    WizardResolvedPath,
    /// The rule behind it: the program always lives in a folder of its own. `{0}` is the name of that folder.
    WizardPathRule,
    /// An installation from before the rename is still on the machine: it has to be removed first. `{0}` is that
    /// folder.
    WizardLegacyFolder,
    WizardOptRegister,
    WizardOptArp,
    WizardOptVscode,
    WizardBrowse,
    WizardCancel,
    WizardInstall,
    WizardWorking,
    WizardClose,
    WizardDone,
    WizardFailed,
    WizardProduct,
    /// The wizard: the uninstall page (the same window, a different job).
    WizardUninstallTitle,
    WizardUninstallIntro,
    WizardUninstallButton,
    WizardUninstallWorking,
    WizardUninstallDone,
    WizardUninstallFailed,
    /// The wizard: the page that is shown when the program is already on the machine. It offers to repair the install,
    /// to install it again or to remove it - the way the setup of another program does it.
    WizardExistingTitle,
    /// What is already installed: `{0}` is the version, `{1}` is the folder it is in.
    WizardExistingInfo,
    /// The line above the three choices of that page.
    WizardExistingHint,
    WizardRepair,
    WizardReinstall,
    WizardRepairWorking,
    WizardRepairDone,
    WizardRepairFailed,
    WizardReinstallWorking,
    WizardReinstallDone,
    WizardReinstallFailed,
}

pub const ALL: [Msg; 45] = [
    Msg::Title,
    Msg::ConfirmInstall,
    Msg::ConfirmUninstall,
    Msg::InstallDone,
    Msg::UninstallDone,
    Msg::RestartNeeded,
    Msg::Failed,
    Msg::ElevationDeclined,
    Msg::Notes,
    Msg::NextSteps,
    Msg::WizardWelcome,
    Msg::WizardIntro,
    Msg::WizardFolder,
    Msg::WizardPathInvalid,
    Msg::WizardResolvedPath,
    Msg::WizardPathRule,
    Msg::WizardLegacyFolder,
    Msg::WizardOptRegister,
    Msg::WizardOptArp,
    Msg::WizardOptVscode,
    Msg::WizardBrowse,
    Msg::WizardCancel,
    Msg::WizardInstall,
    Msg::WizardWorking,
    Msg::WizardClose,
    Msg::WizardDone,
    Msg::WizardFailed,
    Msg::WizardProduct,
    Msg::WizardUninstallTitle,
    Msg::WizardUninstallIntro,
    Msg::WizardUninstallButton,
    Msg::WizardUninstallWorking,
    Msg::WizardUninstallDone,
    Msg::WizardUninstallFailed,
    Msg::WizardExistingTitle,
    Msg::WizardExistingInfo,
    Msg::WizardExistingHint,
    Msg::WizardRepair,
    Msg::WizardReinstall,
    Msg::WizardRepairWorking,
    Msg::WizardRepairDone,
    Msg::WizardRepairFailed,
    Msg::WizardReinstallWorking,
    Msg::WizardReinstallDone,
    Msg::WizardReinstallFailed,
];

/// Whether `msg` belongs to the wizard window (the rest is printed by the console mode).
fn is_wizard(msg: Msg) -> bool {
    matches!(
        msg,
        Msg::WizardWelcome
            | Msg::WizardIntro
            | Msg::WizardFolder
            | Msg::WizardPathInvalid
            | Msg::WizardResolvedPath
            | Msg::WizardPathRule
            | Msg::WizardLegacyFolder
            | Msg::WizardOptRegister
            | Msg::WizardOptArp
            | Msg::WizardOptVscode
            | Msg::WizardBrowse
            | Msg::WizardCancel
            | Msg::WizardInstall
            | Msg::WizardWorking
            | Msg::WizardClose
            | Msg::WizardDone
            | Msg::WizardFailed
            | Msg::WizardProduct
            | Msg::WizardUninstallTitle
            | Msg::WizardUninstallIntro
            | Msg::WizardUninstallButton
            | Msg::WizardUninstallWorking
            | Msg::WizardUninstallDone
            | Msg::WizardUninstallFailed
            | Msg::WizardExistingTitle
            | Msg::WizardExistingInfo
            | Msg::WizardExistingHint
            | Msg::WizardRepair
            | Msg::WizardReinstall
            | Msg::WizardRepairWorking
            | Msg::WizardRepairDone
            | Msg::WizardRepairFailed
            | Msg::WizardReinstallWorking
            | Msg::WizardReinstallDone
            | Msg::WizardReinstallFailed
    )
}

/// The text of `msg` in `lang`: the wizard's messages come from `wizard_text`, the others from `core_text`.
pub fn template(lang: Lang, msg: Msg) -> &'static str {
    if is_wizard(msg) {
        wizard_text(lang, msg)
    } else {
        core_text(lang, msg)
    }
}

/// The texts of the console mode (the prompts and the report of the setup).
fn core_text(lang: Lang, msg: Msg) -> &'static str {
    use Lang::*;
    use Msg::*;
    match (lang, msg) {
        (En, Title) => "IME1.0 Setup",
        (En, ConfirmInstall) => {
            "Install IME1.0 {0} into:\n{1}\n\nThis is a PRE-RELEASE. It registers a text input service (a TSF DLL) with Windows that has not been verified on a clean virtual machine yet, and this installer is not code signed, so Windows may warn about an unknown publisher. It does not change your default input method.\n{2}\nContinue?"
        }
        (En, ConfirmUninstall) => {
            "Remove IME1.0 {0} from:\n{1}\n\nOnly the files and registry entries that the installer created are removed.\n{2}\nContinue?"
        }
        (En, InstallDone) => "IME1.0 {0} was installed into:\n{1}\n\n{2}",
        (En, UninstallDone) => "IME1.0 was removed.\n\n{0}",
        (En, RestartNeeded) => {
            "Some files are in use. Windows removes or replaces them when you restart; sign out or restart the computer to finish."
        }
        (En, Failed) => "The setup did not finish:\n{0}",
        (En, ElevationDeclined) => {
            "The setup needs administrator rights and they were not granted. Nothing was changed."
        }
        (En, Notes) => "Notes:\n{0}",
        (En, NextSteps) => {
            "IME1.0 {0} is installed, but it is not active yet: the setup never changes your default input method.\n\n1. Open Settings > Time & language > Language & region, and choose the language you use.\n2. Under Language options > Keyboards choose Add a keyboard, then pick \"IME1.0\".\n3. Switch to it with Win+Space."
        }

        (ZhCn, Title) => "IME1.0 编程输入法 安装程序",
        (ZhCn, ConfirmInstall) => {
            "将 IME1.0 {0} 安装到：\n{1}\n\n这是预发布版本。它会向 Windows 注册一个文本输入服务（TSF DLL），该组件尚未在干净的虚拟机中验证；安装程序也没有代码签名，Windows 可能提示“未知发布者”。不会更改你的默认输入法。\n{2}\n是否继续？"
        }
        (ZhCn, ConfirmUninstall) => {
            "将从以下位置卸载 IME1.0 {0}：\n{1}\n\n只会删除本安装程序创建的文件和注册表项。\n{2}\n是否继续？"
        }
        (ZhCn, InstallDone) => "IME1.0 {0} 已安装到：\n{1}\n\n{2}",
        (ZhCn, UninstallDone) => "IME1.0 已卸载。\n\n{0}",
        (ZhCn, RestartNeeded) => {
            "部分文件正在使用中，Windows 会在重启时删除或替换它们。请注销或重启电脑以完成。"
        }
        (ZhCn, Failed) => "操作未完成：\n{0}",
        (ZhCn, ElevationDeclined) => "需要管理员权限，但未获授权。系统未做任何更改。",
        (ZhCn, Notes) => "提示：\n{0}",
        (ZhCn, NextSteps) => {
            "IME1.0 {0} 已安装，但还没有启用：安装程序不会更改你的默认输入法。\n\n1. 打开「设置 > 时间和语言 > 语言和区域」，选择你使用的语言。\n2. 在「语言选项 > 键盘」中点「添加键盘」，然后选择“IME1.0”。\n3. 按 Win+空格 切换到它。"
        }

        (Ja, Title) => "IME1.0 セットアップ",
        (Ja, ConfirmInstall) => {
            "IME1.0 {0} を次の場所にインストールします:\n{1}\n\nこれはプレリリース版です。Windows にテキスト入力サービス (TSF DLL) を登録しますが、クリーンな仮想マシンでの検証はまだ完了していません。また、このインストーラーはコード署名されていないため、Windows が「不明な発行元」と警告する場合があります。既定の入力方式は変更しません。\n{2}\n続行しますか?"
        }
        (Ja, ConfirmUninstall) => {
            "次の場所から IME1.0 {0} を削除します:\n{1}\n\nインストーラーが作成したファイルとレジストリ項目だけを削除します。\n{2}\n続行しますか?"
        }
        (Ja, InstallDone) => "IME1.0 {0} を次の場所にインストールしました:\n{1}\n\n{2}",
        (Ja, UninstallDone) => "IME1.0 を削除しました。\n\n{0}",
        (Ja, RestartNeeded) => {
            "使用中のファイルがあります。Windows は再起動時にそれらを削除または置換します。サインアウトまたは再起動して完了してください。"
        }
        (Ja, Failed) => "処理を完了できませんでした:\n{0}",
        (Ja, ElevationDeclined) => {
            "セットアップには管理者権限が必要ですが、許可されませんでした。何も変更していません。"
        }
        (Ja, Notes) => "メモ:\n{0}",
        (Ja, NextSteps) => {
            "IME1.0 {0} はインストールされましたが、まだ有効ではありません。セットアップは既定の入力方式を変更しません。\n\n1. 「設定 > 時刻と言語 > 言語と地域」を開き、使っている言語を選びます。\n2. 「言語のオプション > キーボード」で「キーボードを追加」を選び、「IME1.0」を選択します。\n3. Win+スペースキーで切り替えます。"
        }
        // The wizard's own messages are never printed by the console mode.
        _ => "",
    }
}

/// The texts of the wizard, in the three UI languages.
pub fn wizard_text(lang: Lang, msg: Msg) -> &'static str {
    use Lang::*;
    use Msg::*;
    match (lang, msg) {
        (En, WizardWelcome) => "Install IME1.0",
        (En, WizardIntro) => {
            "IME1.0 is a programming input method. This is a PRE-RELEASE and it is not code signed, so Windows may warn about an unknown publisher. It does not change your default input method."
        }
        (En, WizardFolder) => "Install location",
        (En, WizardPathInvalid) => "This location cannot be used.",
        (En, WizardResolvedPath) => "Will be installed in: {0}",
        (En, WizardPathRule) => "The program always lives in a folder named {0}.",
        (En, WizardLegacyFolder) => {
            "Leftovers of an older version are in {0}: remove that folder first."
        }
        (En, WizardOptRegister) => {
            "Register the input method with Windows (so it can be picked in the language bar)"
        }
        (En, WizardOptArp) => "Add an uninstall entry to \"Apps\"",
        (En, WizardOptVscode) => "Install the VS Code extension (if VS Code is installed)",
        (En, WizardBrowse) => "Browse...",
        (En, WizardCancel) => "Cancel",
        (En, WizardInstall) => "Install",
        (En, WizardWorking) => "Installing...",
        (En, WizardClose) => "Close",
        (En, WizardDone) => "Installation complete",
        (En, WizardFailed) => "Installation failed",
        (En, WizardProduct) => "Programming IME",
        (En, WizardUninstallTitle) => "Uninstall IME1.0",
        (En, WizardUninstallIntro) => {
            "IME1.0 {0} will be removed from:\n{1}\n\nOnly the files and registry entries that the installer created are removed.\n{2}"
        }
        (En, WizardUninstallButton) => "Uninstall",
        (En, WizardUninstallWorking) => "Uninstalling...",
        (En, WizardUninstallDone) => "Uninstall complete",
        (En, WizardUninstallFailed) => "Uninstall failed",
        (En, WizardExistingTitle) => "IME1.0 is already installed",
        (En, WizardExistingInfo) => "Installed version: {0}\nLocation: {1}",
        (En, WizardExistingHint) => "Choose what to do with the installation that is already there.",
        (En, WizardRepair) => "Repair",
        (En, WizardReinstall) => "Reinstall",
        (En, WizardRepairWorking) => "Repairing...",
        (En, WizardRepairDone) => "Repair complete",
        (En, WizardRepairFailed) => "Repair failed",
        (En, WizardReinstallWorking) => "Reinstalling...",
        (En, WizardReinstallDone) => "Reinstall complete",
        (En, WizardReinstallFailed) => "Reinstall failed",

        (ZhCn, WizardWelcome) => "安装 IME1.0",
        (ZhCn, WizardIntro) => {
            "IME1.0 是编程输入法。这是预发布版本且没有代码签名，Windows 可能提示“未知发布者”。安装不会更改你的默认输入法。"
        }
        (ZhCn, WizardFolder) => "安装位置",
        (ZhCn, WizardPathInvalid) => "该安装位置不可用。",
        (ZhCn, WizardResolvedPath) => "实际安装到：{0}",
        (ZhCn, WizardPathRule) => "程序始终位于名为 {0} 的文件夹中。",
        (ZhCn, WizardLegacyFolder) => "{0} 里还有旧版残留，请先删除该目录。",
        (ZhCn, WizardOptRegister) => "向 Windows 注册输入法（可以在语言栏里选到）",
        (ZhCn, WizardOptArp) => "在「应用」里添加卸载入口",
        (ZhCn, WizardOptVscode) => "安装 VS Code 扩展（如果装了 VS Code）",
        (ZhCn, WizardBrowse) => "浏览…",
        (ZhCn, WizardCancel) => "取消",
        (ZhCn, WizardInstall) => "安装",
        (ZhCn, WizardWorking) => "正在安装…",
        (ZhCn, WizardClose) => "完成",
        (ZhCn, WizardDone) => "安装完成",
        (ZhCn, WizardFailed) => "安装失败",
        (ZhCn, WizardProduct) => "编程输入法",
        (ZhCn, WizardUninstallTitle) => "卸载 IME1.0",
        (ZhCn, WizardUninstallIntro) => {
            "将从以下位置卸载 IME1.0 {0}：\n{1}\n\n只会删除本安装程序创建的文件和注册表项。\n{2}"
        }
        (ZhCn, WizardUninstallButton) => "卸载",
        (ZhCn, WizardUninstallWorking) => "正在卸载…",
        (ZhCn, WizardUninstallDone) => "卸载完成",
        (ZhCn, WizardUninstallFailed) => "卸载失败",
        (ZhCn, WizardExistingTitle) => "IME1.0 已安装",
        (ZhCn, WizardExistingInfo) => "已安装版本：{0}\n安装位置：{1}",
        (ZhCn, WizardExistingHint) => "请选择要对现有安装执行的操作。",
        (ZhCn, WizardRepair) => "修复",
        (ZhCn, WizardReinstall) => "重新安装",
        (ZhCn, WizardRepairWorking) => "正在修复…",
        (ZhCn, WizardRepairDone) => "修复完成",
        (ZhCn, WizardRepairFailed) => "修复失败",
        (ZhCn, WizardReinstallWorking) => "正在重新安装…",
        (ZhCn, WizardReinstallDone) => "重新安装完成",
        (ZhCn, WizardReinstallFailed) => "重新安装失败",

        (Ja, WizardWelcome) => "IME1.0 をインストール",
        (Ja, WizardIntro) => {
            "IME1.0 はプログラミング用の入力方式です。プレリリース版でコード署名もされていないため、Windows が「不明な発行元」と警告する場合があります。既定の入力方式は変更しません。"
        }
        (Ja, WizardFolder) => "インストール先",
        (Ja, WizardPathInvalid) => "このインストール先は使用できません。",
        (Ja, WizardResolvedPath) => "実際にインストールする場所: {0}",
        (Ja, WizardPathRule) => "プログラムは常に {0} という名前のフォルダーに置かれます。",
        (Ja, WizardLegacyFolder) => "{0} に古いバージョンの残骸があります。先に削除してください。",
        (Ja, WizardOptRegister) => "Windows に入力方式を登録する（言語バーから選択できます）",
        (Ja, WizardOptArp) => "「アプリ」にアンインストール用の項目を追加する",
        (Ja, WizardOptVscode) => "VS Code 拡張機能をインストールする（VS Code がある場合）",
        (Ja, WizardBrowse) => "参照...",
        (Ja, WizardCancel) => "キャンセル",
        (Ja, WizardInstall) => "インストール",
        (Ja, WizardWorking) => "インストール中...",
        (Ja, WizardClose) => "閉じる",
        (Ja, WizardDone) => "インストールが完了しました",
        (Ja, WizardFailed) => "インストールに失敗しました",
        (Ja, WizardProduct) => "プログラミング入力",
        (Ja, WizardUninstallTitle) => "IME1.0 をアンインストール",
        (Ja, WizardUninstallIntro) => {
            "次の場所から IME1.0 {0} を削除します:\n{1}\n\nインストーラーが作成したファイルとレジストリ項目だけを削除します。\n{2}"
        }
        (Ja, WizardUninstallButton) => "アンインストール",
        (Ja, WizardUninstallWorking) => "アンインストール中...",
        (Ja, WizardUninstallDone) => "アンインストールが完了しました",
        (Ja, WizardUninstallFailed) => "アンインストールに失敗しました",
        (Ja, WizardExistingTitle) => "IME1.0 はすでにインストールされています",
        (Ja, WizardExistingInfo) => "インストール済みバージョン: {0}\n場所: {1}",
        (Ja, WizardExistingHint) => "既存のインストールに対して行う操作を選んでください。",
        (Ja, WizardRepair) => "修復",
        (Ja, WizardReinstall) => "再インストール",
        (Ja, WizardRepairWorking) => "修復中...",
        (Ja, WizardRepairDone) => "修復が完了しました",
        (Ja, WizardRepairFailed) => "修復に失敗しました",
        (Ja, WizardReinstallWorking) => "再インストール中...",
        (Ja, WizardReinstallDone) => "再インストールが完了しました",
        (Ja, WizardReinstallFailed) => "再インストールに失敗しました",
        // The console messages are printed by `core_text`.
        _ => core_text(lang, msg),
    }
}

/// Fills `{0}`, `{1}`, ... in the template of `msg`.
pub fn text(lang: Lang, msg: Msg, args: &[&str]) -> String {
    let mut out = template(lang, msg).to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(t: &str) -> Vec<usize> {
        (0..5).filter(|i| t.contains(&format!("{{{i}}}"))).collect()
    }

    #[test]
    fn every_language_has_the_same_placeholders_for_every_message() {
        for msg in ALL {
            let en = placeholders(template(Lang::En, msg));
            for lang in [Lang::ZhCn, Lang::Ja] {
                assert_eq!(placeholders(template(lang, msg)), en, "{msg:?} in {lang:?}");
            }
        }
    }

    /// The parity test above compares the languages with each other: a message that is routed to a text table that
    /// does not know it is empty in *every* language, so the comparison stays green and the window shows a blank.
    /// This test is the judgement layer: every message must resolve to a real text in every language.
    #[test]
    fn every_message_resolves_to_a_text_in_every_language() {
        for msg in ALL {
            for lang in [Lang::En, Lang::ZhCn, Lang::Ja] {
                let t = template(lang, msg);
                assert!(!t.trim().is_empty(), "{msg:?} has no text in {lang:?}");
            }
        }
    }

    #[test]
    fn the_install_prompt_in_every_language_tells_the_user_it_is_a_pre_release_and_unsigned() {
        assert!(template(Lang::En, Msg::ConfirmInstall).contains("PRE-RELEASE"));
        assert!(template(Lang::ZhCn, Msg::ConfirmInstall).contains("预发布"));
        assert!(template(Lang::ZhCn, Msg::ConfirmInstall).contains("代码签名"));
        assert!(template(Lang::Ja, Msg::ConfirmInstall).contains("プレリリース"));
        assert!(template(Lang::Ja, Msg::ConfirmInstall).contains("コード署名"));
    }

    #[test]
    fn placeholders_are_filled() {
        let s = text(
            Lang::En,
            Msg::InstallDone,
            &["0.2.0", r"C:\Program Files\IME1.0", "ok"],
        );
        assert_eq!(
            s,
            "IME1.0 0.2.0 was installed into:\nC:\\Program Files\\IME1.0\n\nok"
        );
    }

    /// The window tells the person where the program really goes to, and it names the folder it is put in. A
    /// message that does not say that is the message that lets somebody install into the root of a drive.
    #[test]
    fn the_window_says_where_the_program_goes_and_names_the_product_folder() {
        for lang in [Lang::En, Lang::ZhCn, Lang::Ja] {
            let resolved = template(lang, Msg::WizardResolvedPath);
            assert!(resolved.contains("{0}"), "{lang:?}: {resolved}");
            let rule = template(lang, Msg::WizardPathRule);
            assert!(rule.contains("{0}"), "{lang:?}: {rule}");
            // and the folder it names is the one the product uses
            let filled = text(lang, Msg::WizardPathRule, &["IME1.0"]);
            assert!(filled.contains("IME1.0"), "{lang:?}: {filled}");
        }
        // the reason a folder is refused is written under the message, not left out of it
        for lang in [Lang::En, Lang::ZhCn, Lang::Ja] {
            let invalid = template(lang, Msg::WizardPathInvalid);
            assert!(!invalid.trim().is_empty(), "{lang:?}: {invalid}");
            // the message is a sentence in the language of the window; the reason is a technical detail of its own
            // and is written under it (a reason in another language inside a sentence would be hard to read)
            assert!(!invalid.contains("{0}"), "{lang:?}: {invalid}");
        }
    }

    #[test]
    fn st_035_the_guidance_names_the_settings_of_the_language_the_user_types_in() {
        assert!(template(Lang::En, Msg::NextSteps).contains("Time & language"));
        assert!(template(Lang::En, Msg::NextSteps).contains("Language & region"));
        assert!(template(Lang::ZhCn, Msg::NextSteps).contains("时间和语言"));
        assert!(template(Lang::ZhCn, Msg::NextSteps).contains("语言和区域"));
        assert!(template(Lang::Ja, Msg::NextSteps).contains("時刻と言語"));
        assert!(template(Lang::Ja, Msg::NextSteps).contains("言語と地域"));
        // and every language says how to switch to the IME afterwards
        for lang in [Lang::En, Lang::ZhCn, Lang::Ja] {
            assert!(template(lang, Msg::NextSteps).contains("Win+"), "{lang:?}");
        }
    }

    #[test]
    fn st_035_the_guidance_says_that_the_default_input_method_was_not_changed() {
        // the installer never changes it, so the text must not suggest that it did
        assert!(
            template(Lang::En, Msg::NextSteps).contains("never changes your default input method")
        );
        assert!(template(Lang::ZhCn, Msg::NextSteps).contains("不会更改你的默认输入法"));
        assert!(template(Lang::Ja, Msg::NextSteps).contains("既定の入力方式を変更しません"));
    }
}

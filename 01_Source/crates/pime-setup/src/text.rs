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
}

pub const ALL: [Msg; 9] = [
    Msg::Title,
    Msg::ConfirmInstall,
    Msg::ConfirmUninstall,
    Msg::InstallDone,
    Msg::UninstallDone,
    Msg::RestartNeeded,
    Msg::Failed,
    Msg::ElevationDeclined,
    Msg::Notes,
];

pub fn template(lang: Lang, msg: Msg) -> &'static str {
    use Lang::*;
    use Msg::*;
    match (lang, msg) {
        (En, Title) => "Programming IME Setup",
        (En, ConfirmInstall) => {
            "Install Programming IME {0} into:\n{1}\n\nThis is a PRE-RELEASE. It registers a text input service (a TSF DLL) with Windows that has not been verified on a clean virtual machine yet, and this installer is not code signed, so Windows may warn about an unknown publisher. It does not change your default input method.\n{2}\nContinue?"
        }
        (En, ConfirmUninstall) => {
            "Remove Programming IME {0} from:\n{1}\n\nOnly the files and registry entries that the installer created are removed.\n\nContinue?"
        }
        (En, InstallDone) => "Programming IME {0} was installed into:\n{1}\n\n{2}",
        (En, UninstallDone) => "Programming IME was removed.\n\n{0}",
        (En, RestartNeeded) => {
            "Some files are in use. Windows removes or replaces them when you restart; sign out or restart the computer to finish."
        }
        (En, Failed) => "The setup did not finish:\n{0}",
        (En, ElevationDeclined) => {
            "The setup needs administrator rights and they were not granted. Nothing was changed."
        }
        (En, Notes) => "Notes:\n{0}",

        (ZhCn, Title) => "Programming IME 安装程序",
        (ZhCn, ConfirmInstall) => {
            "将 Programming IME {0} 安装到：\n{1}\n\n这是预发布版本。它会向 Windows 注册一个文本输入服务（TSF DLL），该组件尚未在干净的虚拟机中验证；安装程序也没有代码签名，Windows 可能提示“未知发布者”。不会更改你的默认输入法。\n{2}\n是否继续？"
        }
        (ZhCn, ConfirmUninstall) => {
            "将从以下位置卸载 Programming IME {0}：\n{1}\n\n只会删除本安装程序创建的文件和注册表项。\n\n是否继续？"
        }
        (ZhCn, InstallDone) => "Programming IME {0} 已安装到：\n{1}\n\n{2}",
        (ZhCn, UninstallDone) => "Programming IME 已卸载。\n\n{0}",
        (ZhCn, RestartNeeded) => {
            "部分文件正在使用中，Windows 会在重启时删除或替换它们。请注销或重启电脑以完成。"
        }
        (ZhCn, Failed) => "操作未完成：\n{0}",
        (ZhCn, ElevationDeclined) => "需要管理员权限，但未获授权。系统未做任何更改。",
        (ZhCn, Notes) => "提示：\n{0}",

        (Ja, Title) => "Programming IME セットアップ",
        (Ja, ConfirmInstall) => {
            "Programming IME {0} を次の場所にインストールします:\n{1}\n\nこれはプレリリース版です。Windows にテキスト入力サービス (TSF DLL) を登録しますが、クリーンな仮想マシンでの検証はまだ完了していません。また、このインストーラーはコード署名されていないため、Windows が「不明な発行元」と警告する場合があります。既定の入力方式は変更しません。\n{2}\n続行しますか?"
        }
        (Ja, ConfirmUninstall) => {
            "次の場所から Programming IME {0} を削除します:\n{1}\n\nインストーラーが作成したファイルとレジストリ項目だけを削除します。\n\n続行しますか?"
        }
        (Ja, InstallDone) => "Programming IME {0} を次の場所にインストールしました:\n{1}\n\n{2}",
        (Ja, UninstallDone) => "Programming IME を削除しました。\n\n{0}",
        (Ja, RestartNeeded) => {
            "使用中のファイルがあります。Windows は再起動時にそれらを削除または置換します。サインアウトまたは再起動して完了してください。"
        }
        (Ja, Failed) => "処理を完了できませんでした:\n{0}",
        (Ja, ElevationDeclined) => {
            "セットアップには管理者権限が必要ですが、許可されませんでした。何も変更していません。"
        }
        (Ja, Notes) => "メモ:\n{0}",
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
            &["0.2.0", r"C:\Program Files\Programming IME", "ok"],
        );
        assert_eq!(
            s,
            "Programming IME 0.2.0 was installed into:\nC:\\Program Files\\Programming IME\n\nok"
        );
    }
}

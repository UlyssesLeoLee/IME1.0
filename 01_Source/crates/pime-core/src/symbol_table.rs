//! The symbol table: the symbols the panel offers (the "符号表" of the input methods).
//!
//! The table is **data**, not behaviour: it is the same for every platform and it is read by the text service when
//! the user opens the row of the panel. The names are the ones a screen reader would read; the text is what goes
//! into the document.

/// One symbol of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// The text that is written into the document.
    pub text: &'static str,
    /// The name of the symbol (what a screen reader reads).
    pub name: &'static str,
}

/// The symbols of the table, in the order they are offered.
pub const SYMBOLS: &[Symbol] = &[
    // the arrows
    Symbol { text: "←", name: "向左箭头" },
    Symbol { text: "→", name: "向右箭头" },
    Symbol { text: "↑", name: "向上箭头" },
    Symbol { text: "↓", name: "向下箭头" },
    Symbol { text: "↔", name: "左右箭头" },
    Symbol { text: "⇄", name: "双向箭头" },
    // the mathematics
    Symbol { text: "±", name: "正负号" },
    Symbol { text: "×", name: "乘号" },
    Symbol { text: "÷", name: "除号" },
    Symbol { text: "≠", name: "不等于" },
    Symbol { text: "≤", name: "小于等于" },
    Symbol { text: "≥", name: "大于等于" },
    Symbol { text: "≈", name: "约等于" },
    Symbol { text: "∞", name: "无穷大" },
    Symbol { text: "√", name: "平方根" },
    Symbol { text: "∑", name: "求和" },
    Symbol { text: "∏", name: "连乘" },
    Symbol { text: "∫", name: "积分" },
    Symbol { text: "∂", name: "偏导数" },
    Symbol { text: "∇", name: "梯度" },
    Symbol { text: "∈", name: "属于" },
    Symbol { text: "∉", name: "不属于" },
    Symbol { text: "⊂", name: "真包含于" },
    Symbol { text: "⊆", name: "包含于" },
    Symbol { text: "∪", name: "并集" },
    Symbol { text: "∩", name: "交集" },
    Symbol { text: "∅", name: "空集" },
    Symbol { text: "→", name: "推出" },
    Symbol { text: "⇒", name: "蕴含" },
    // the logic and the sets
    Symbol { text: "¬", name: "非" },
    Symbol { text: "∧", name: "合取" },
    Symbol { text: "∨", name: "析取" },
    Symbol { text: "∀", name: "对所有" },
    Symbol { text: "∃", name: "存在" },
    // the punctuation and the pieces
    Symbol { text: "…", name: "省略号" },
    Symbol { text: "—", name: "破折号" },
    Symbol { text: "–", name: "连接号" },
    Symbol { text: "·", name: "间隔号" },
    Symbol { text: "•", name: "项目符号" },
    Symbol { text: "§", name: "节符号" },
    Symbol { text: "¶", name: "段落符号" },
    Symbol { text: "†", name: "剑标" },
    Symbol { text: "‡", name: "双剑标" },
    Symbol { text: "№", name: "编号符号" },
    Symbol { text: "℃", name: "摄氏度" },
    Symbol { text: "©", name: "版权" },
    Symbol { text: "®", name: "注册商标" },
    Symbol { text: "™", name: "商标" },
    Symbol { text: "€", name: "欧元" },
    Symbol { text: "£", name: "英镑" },
    Symbol { text: "¥", name: "日元" },
    Symbol { text: "×", name: "乘号" },
    // the Greek letters that the code uses
    Symbol { text: "α", name: "阿尔法" },
    Symbol { text: "β", name: "贝塔" },
    Symbol { text: "γ", name: "伽马" },
    Symbol { text: "δ", name: "德尔塔" },
    Symbol { text: "λ", name: "拉姆达" },
    Symbol { text: "μ", name: "缪" },
    Symbol { text: "π", name: "派" },
    Symbol { text: "σ", name: "西格马" },
    Symbol { text: "φ", name: "斐" },
    Symbol { text: "ω", name: "欧米伽" },
];

/// The symbols of the table, as the pairs the panel shows (the text, the name).
pub fn symbol_pairs() -> Vec<(&'static str, &'static str)> {
    SYMBOLS.iter().map(|symbol| (symbol.text, symbol.name)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is not empty and every symbol has a name (a screen reader reads it).
    #[test]
    fn every_symbol_has_a_text_and_a_name() {
        assert!(SYMBOLS.len() > 20, "the table is not a short list");
        for symbol in SYMBOLS {
            assert!(!symbol.text.is_empty(), "{symbol:?}");
            assert!(!symbol.name.is_empty(), "{symbol:?}");
            assert_eq!(symbol.text.chars().count(), 1, "{symbol:?}: one character");
        }
    }

    /// The pairs are the table, in order.
    #[test]
    fn the_pairs_are_the_table() {
        let pairs = symbol_pairs();
        assert_eq!(pairs.len(), SYMBOLS.len());
        assert_eq!(pairs[0], ("←", "向左箭头"));
    }
}

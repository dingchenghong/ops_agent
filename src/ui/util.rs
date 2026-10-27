//! 终端文本布局工具：按显示宽度（CJK 友好）换行，并支持光标定位

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Debug)]
pub struct WrapLine {
    pub text: String,
    /// 在原始字符串中的字节起点
    pub start: usize,
    /// 在原始字符串中的字节终点（不含换行符）
    pub end: usize,
}

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// 按显示宽度换行（优先在空格处折行，尊重显式换行符）
pub fn wrap_layout(text: &str, width: usize) -> Vec<WrapLine> {
    let width = width.max(1);
    let mut out: Vec<WrapLine> = Vec::new();
    let mut start = 0usize;
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;

    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            out.push(WrapLine { text: text[start..idx].to_string(), start, end: idx });
            start = idx + ch.len_utf8();
            cur_w = 0;
            last_space = None;
            continue;
        }
        let w = char_width(ch);
        if cur_w + w > width && idx > start {
            match last_space.filter(|sp| *sp >= start) {
                Some(sp) => {
                    out.push(WrapLine { text: text[start..sp].to_string(), start, end: sp });
                    start = sp + 1;
                    cur_w = display_width(&text[start..idx]);
                }
                None => {
                    out.push(WrapLine { text: text[start..idx].to_string(), start, end: idx });
                    start = idx;
                    cur_w = 0;
                }
            }
            last_space = None;
        }
        cur_w += w;
        if ch == ' ' {
            last_space = Some(idx);
        }
    }
    out.push(WrapLine { text: text[start..].to_string(), start, end: text.len() });
    out
}

pub fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    wrap_layout(text, width).into_iter().map(|l| l.text).collect()
}

/// 光标（字节偏移）落在第几行、第几列（显示宽度）
pub fn cursor_in(lines: &[WrapLine], cursor: usize) -> (usize, usize) {
    for (row, line) in lines.iter().enumerate() {
        if cursor >= line.start && cursor <= line.end {
            let rel = cursor.saturating_sub(line.start).min(line.text.len());
            let mut rel = rel;
            while rel > 0 && !line.text.is_char_boundary(rel) {
                rel -= 1;
            }
            return (row, display_width(&line.text[..rel]));
        }
    }
    let row = lines.len().saturating_sub(1);
    let col = lines.get(row).map(|l| display_width(&l.text)).unwrap_or(0);
    (row, col)
}

/// 截断到指定显示宽度，超出加省略号
pub fn clip(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = char_width(ch);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// 宽度不可靠的字符（Emoji 变体选择符、常被渲染成 2 格的符号）会让整行右移，
/// 从而把它旁边的边框格子顶掉。这里统一替换成固定宽度的安全字符。
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            // 变体选择符 / 零宽连接符 / 组合键帽
            '\u{FE0E}' | '\u{FE0F}' | '\u{200D}' | '\u{20E3}' => {}
            '⚠' | '⚡' | '❗' | '❕' | '⏸' | '⏳' => out.push('!'),
            '✅' | '✔' | '☑' => out.push_str("[v]"),
            '❌' | '✖' | '❎' => out.push_str("[x]"),
            '❓' | '❔' => out.push('?'),
            '⭐' | '🌟' | '✨' | '⚙' => out.push('*'),
            '➡' => out.push_str("->"),
            c if risky_width(c) => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

/// 单字符消毒：返回要写入的安全字符（可能为空）
pub fn sanitize_char(c: char) -> String {
    sanitize(&c.to_string())
}

/// 这些码点在 unicode-width 里算 1 格，但很多终端（尤其带 Emoji 字体的）会画成 2 格
fn risky_width(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x23F8..=0x23FA
        | 0x25FD..=0x25FE | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F
        | 0x2693 | 0x2699 | 0x26A0..=0x26A1 | 0x26AA..=0x26AB | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3
        | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A..=0x270B | 0x2728
        | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797
        | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_emoji_variation_selector() {
        assert_eq!(sanitize("⚠️ 提示"), "! 提示");
        assert_eq!(sanitize("完成 ✅"), "完成 [v]");
        assert_eq!(sanitize("失败 ❌"), "失败 [x]");
        assert_eq!(sanitize("工具 ⚙ 运行"), "工具 * 运行");
    }

    #[test]
    fn keeps_normal_text_and_cjk() {
        assert_eq!(sanitize("df -h 磁盘"), "df -h 磁盘");
        assert_eq!(sanitize("◆ ● ▌ ⇔"), "◆ ● ▌ ⇔");
    }
}

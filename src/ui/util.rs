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

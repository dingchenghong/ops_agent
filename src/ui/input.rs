//! 多行文本输入框（字符级光标 + 选区 + 历史记录）

#[derive(Default, Clone)]
pub struct TextInput {
    pub value: String,
    /// 光标位置（字符索引）
    pub cursor: usize,
    /// 选区锚点（字符索引）；None 或与 cursor 相同表示无选区
    pub anchor: Option<usize>,
    history: Vec<String>,
    history_idx: Option<usize>,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    /// 字符索引 → 字节索引
    pub fn byte_at(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }

    /// 当前选区（字符索引，保证 start <= end）
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(if a < self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        })
    }

    pub fn selection_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        Some(self.value[self.byte_at(s)..self.byte_at(e)].to_string())
    }

    pub fn select_all(&mut self) {
        if self.value.is_empty() {
            return;
        }
        self.anchor = Some(0);
        self.cursor = self.char_len();
        self.history_idx = None;
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// 删除选区；返回是否真的删了内容
    pub fn delete_selection(&mut self) -> bool {
        let Some((s, e)) = self.selection() else {
            return false;
        };
        let (from, to) = (self.byte_at(s), self.byte_at(e));
        self.value.replace_range(from..to, "");
        self.cursor = s;
        self.anchor = None;
        true
    }

    pub fn insert_char(&mut self, c: char) {
        // 过滤掉终端宽度不可靠的字符（emoji 等），否则整行会错位
        let safe = super::util::sanitize_char(c);
        if safe.is_empty() {
            return;
        }
        self.delete_selection();
        let at = self.byte_at(self.cursor);
        self.value.insert_str(at, &safe);
        self.cursor += safe.chars().count();
        self.anchor = None;
        self.history_idx = None;
    }

    /// 一次插入一段文本（粘贴用），换行会原样保留
    pub fn insert_str(&mut self, s: &str) {
        let s = super::util::sanitize(s);
        if s.is_empty() {
            return;
        }
        self.delete_selection();
        let at = self.byte_at(self.cursor);
        self.value.insert_str(at, &s);
        self.cursor += s.chars().count();
        self.anchor = None;
        self.history_idx = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let (from, to) = (self.byte_at(self.cursor - 1), self.byte_at(self.cursor));
        self.value.replace_range(from..to, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor >= self.char_len() {
            return;
        }
        let (from, to) = (self.byte_at(self.cursor), self.byte_at(self.cursor + 1));
        self.value.replace_range(from..to, "");
    }

    pub fn left(&mut self) {
        if let Some((s, _)) = self.selection() {
            self.cursor = s;
            self.anchor = None;
            return;
        }
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if let Some((_, e)) = self.selection() {
            self.cursor = e;
            self.anchor = None;
            return;
        }
        if self.cursor < self.char_len() {
            self.cursor += 1;
        }
    }

    fn line_start_char(&self) -> usize {
        let at = self.byte_at(self.cursor);
        let start = self.value[..at].rfind('\n').map(|p| p + 1).unwrap_or(0);
        self.value[..start].chars().count()
    }

    fn line_end_char(&self) -> usize {
        let at = self.byte_at(self.cursor);
        let tail = &self.value[at..];
        let add = match tail.find('\n') {
            Some(p) => tail[..p].chars().count(),
            None => tail.chars().count(),
        };
        self.cursor + add
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start_char();
        self.anchor = None;
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end_char();
        self.anchor = None;
    }

    // ---- Shift + 方向键：扩展选区 ----

    fn mark_anchor(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
    }

    pub fn left_extend(&mut self) {
        self.mark_anchor();
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn right_extend(&mut self) {
        self.mark_anchor();
        if self.cursor < self.char_len() {
            self.cursor += 1;
        }
    }

    pub fn home_extend(&mut self) {
        self.mark_anchor();
        self.cursor = self.line_start_char();
    }

    pub fn end_extend(&mut self) {
        self.mark_anchor();
        self.cursor = self.line_end_char();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.anchor = None;
        self.history_idx = None;
    }

    pub fn delete_word(&mut self) {
        if self.delete_selection() {
            return;
        }
        // 先吃掉尾部空白，再删掉一个单词
        while self.cursor > 0 {
            let prev = self.value.chars().nth(self.cursor - 1).unwrap_or(' ');
            if prev.is_whitespace() {
                self.backspace();
            } else {
                break;
            }
        }
        while self.cursor > 0 {
            let prev = self.value.chars().nth(self.cursor - 1).unwrap_or(' ');
            if prev.is_whitespace() {
                break;
            }
            self.backspace();
        }
    }

    /// 取出内容并清空（同时写入历史）
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.value);
        self.cursor = 0;
        self.anchor = None;
        self.history_idx = None;
        if !text.trim().is_empty() {
            self.history.push(text.trim().to_string());
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        text
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            None => self.history.len() - 1,
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        self.value = self.history[idx].clone();
        self.cursor = self.char_len();
        self.anchor = None;
    }

    pub fn history_next(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_idx = None;
                self.clear();
            }
            Some(i) => {
                self.history_idx = Some(i + 1);
                self.value = self.history[i + 1].clone();
                self.cursor = self.char_len();
                self.anchor = None;
            }
        }
    }

    /// 光标对应的字节偏移
    pub fn byte_cursor(&self) -> usize {
        self.byte_at(self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_and_delete() {
        let mut input = TextInput::new();
        input.insert_str("hello world");
        input.select_all();
        assert_eq!(input.selection(), Some((0, 11)));
        assert_eq!(input.selection_text().as_deref(), Some("hello world"));
        assert!(input.delete_selection());
        assert!(input.value.is_empty());
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn shift_arrows_build_selection() {
        let mut input = TextInput::new();
        input.insert_str("abc");
        input.home();
        input.right_extend();
        input.right_extend();
        assert_eq!(input.selection_text().as_deref(), Some("ab"));
        // 普通方向键会折叠选区，光标落到选区起点
        input.left();
        assert_eq!(input.selection(), None);
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn typing_replaces_selection() {
        let mut input = TextInput::new();
        input.insert_str("abcdef");
        input.select_all();
        input.insert_char('X');
        assert_eq!(input.value, "X");
    }

    #[test]
    fn backspace_deletes_selection() {
        let mut input = TextInput::new();
        input.insert_str("abcdef");
        input.select_all();
        input.backspace();
        assert!(input.value.is_empty());
    }
}

//! 多行文本输入框（字符级光标 + 历史记录）

#[derive(Default, Clone)]
pub struct TextInput {
    pub value: String,
    /// 光标位置（字符索引）
    pub cursor: usize,
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

    fn byte_of(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }

    pub fn insert_char(&mut self, c: char) {
        let at = self.byte_of(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
        self.history_idx = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let from = self.byte_of(self.cursor - 1);
        let to = self.byte_of(self.cursor);
        self.value.replace_range(from..to, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.value.chars().count() {
            return;
        }
        let from = self.byte_of(self.cursor);
        let to = self.byte_of(self.cursor + 1);
        self.value.replace_range(from..to, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.value.chars().count() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        // 移动到当前行首
        let at = self.byte_of(self.cursor);
        let line_start = self.value[..at].rfind('\n').map(|p| p + 1).unwrap_or(0);
        self.cursor = self.value[..line_start].chars().count();
    }

    pub fn end(&mut self) {
        let at = self.byte_of(self.cursor);
        let tail = &self.value[at..];
        let add = match tail.find('\n') {
            Some(p) => tail[..p].chars().count(),
            None => tail.chars().count(),
        };
        self.cursor += add;
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.history_idx = None;
    }

    pub fn delete_word(&mut self) {
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
        self.cursor = self.value.chars().count();
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
                self.cursor = self.value.chars().count();
            }
        }
    }

    /// 光标对应的字节偏移
    pub fn byte_cursor(&self) -> usize {
        self.byte_of(self.cursor)
    }
}

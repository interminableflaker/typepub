pub struct Backend {
    text: String,
    typed: String,
    cursor: Len,
    cursor_prev: Len,
    errors: Vec<Len>,
    deleted_errors: Vec<Len>,
}

impl Backend {
    pub fn new(text: String) -> Self {
        Self {
            text,
            typed: String::new(),
            cursor: Len::new(0, 0),
            cursor_prev: Len::new(0, 0),
            errors: Vec::new(),
            deleted_errors: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> Len {
        self.cursor
    }

    pub fn last_cursor_position(&self) -> Len {
        self.cursor_prev
    }

    pub fn errors(&self) -> &[Len] {
        &self.errors
    }

    pub fn backspaced_errors(&self) -> &[Len] {
        &self.deleted_errors
    }

    pub fn clear_per_update_data(&mut self) {
        self.deleted_errors.truncate(0);
    }

    pub fn push(&mut self, c: char) {
        let Some(goal) = self.text[self.cursor.bytes..].chars().next() else {
            return;
        };
        self.typed.push(c);
        if !chars_are_equal_including_unicode_alternatives(goal, c) {
            self.errors.push(self.cursor);
        }
        self.cursor_prev = self.cursor;
        self.cursor.bytes += goal.len_utf8();
        self.cursor.chars += 1;
    }

    pub fn pop(&mut self) {
        let Some(typed) = self.typed.chars().last() else {
            return;
        };
        let text = self.text[..self.cursor.bytes].chars().last().unwrap();
        self.delete_backwards_impl(Len::new(text.len_utf8(), 1), Len::new(typed.len_utf8(), 1));
    }

    pub fn delete_word_backwards(&mut self) {
        let mut found_nonwhitespace = false;
        let [typed, text] = self
            .typed
            .chars()
            .rev()
            .take_while(move |c| {
                let is_ws = c.is_whitespace();
                found_nonwhitespace |= !is_ws;
                !(found_nonwhitespace && is_ws)
            })
            .zip(self.text[..self.cursor.bytes].chars().rev())
            .map(|(a, b)| [Len::new(a.len_utf8(), 1), Len::new(b.len_utf8(), 1)])
            .fold([Len::default(); 2], |acc, x| [0, 1].map(|i| acc[i] + x[i]));
        self.delete_backwards_impl(text, typed);
    }

    fn delete_backwards_impl(&mut self, len: Len, typed: Len) {
        self.typed.truncate(self.typed.len() - typed.bytes);
        self.cursor_prev = self.cursor;
        self.cursor -= len;

        // TODO: binary search this
        if let Some(first_deleted_error) = self
            .errors
            .iter()
            .position(|&i| i.chars >= self.cursor.chars)
        {
            self.deleted_errors
                .extend(self.errors.drain(first_deleted_error..));
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct Len {
    pub bytes: usize,
    pub chars: usize,
}

impl Len {
    pub fn new(bytes: usize, chars: usize) -> Self {
        Self { bytes, chars }
    }
}

impl std::ops::Add<Self> for Len {
    type Output = Self;

    fn add(self, Self { bytes, chars }: Self) -> Self::Output {
        Self {
            bytes: self.bytes + bytes,
            chars: self.chars + chars,
        }
    }
}

impl std::ops::AddAssign<Self> for Len {
    fn add_assign(&mut self, rhs: Self) {
        self.bytes += rhs.bytes;
        self.chars += rhs.chars;
    }
}

impl std::ops::Sub<Self> for Len {
    type Output = Self;

    fn sub(self, Self { bytes, chars }: Self) -> Self::Output {
        Self {
            bytes: self.bytes - bytes,
            chars: self.chars - chars,
        }
    }
}

impl std::ops::SubAssign<Self> for Len {
    fn sub_assign(&mut self, rhs: Self) {
        self.bytes -= rhs.bytes;
        self.chars -= rhs.chars;
    }
}

// TODO: probably make this configurable
fn chars_are_equal_including_unicode_alternatives(expected: char, got: char) -> bool {
    match got {
        '\'' if ['‘', '’'].contains(&expected) => true,
        '\"' if ['“', '”'].contains(&expected) => true,
        ' ' if [' '].contains(&expected) => true,
        _ => expected == got,
    }
}

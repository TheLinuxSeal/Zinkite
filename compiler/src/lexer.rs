//! The lexer (GRAMMAR §1): source text to tokens.

use crate::diag::Diagnostic;
use crate::source::Span;
use crate::token::{Cond, Kw, P, PUNCTS, Prim, StrPiece, Tok, Token};

/// Lexes a whole file. The last token is always `Eof`.
pub fn lex(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lx = Lexer::new(src);
    lx.skip_shebang();
    let mut toks = Vec::new();
    loop {
        let t = lx.next_token();
        let eof = t.tok == Tok::Eof;
        toks.push(t);
        if eof {
            break;
        }
    }
    (toks, lx.diags)
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    /// Nothing but whitespace since the start of the line.
    line_start: bool,
    /// The previous token was `return`, so a `#` that starts a line is a status write.
    prev_return: bool,
    /// Inside how many `${...}` interpolations.
    interp_depth: u32,
    diags: Vec<Diagnostic>,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Lexer<'a> {
        Lexer {
            src,
            b: src.as_bytes(),
            pos: 0,
            line_start: true,
            prev_return: false,
            interp_depth: 0,
            diags: Vec::new(),
        }
    }

    fn error(&mut self, start: usize, end: usize, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(Span::new(start, end), msg));
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.b.get(self.pos + ahead).copied()
    }

    fn skip_shebang(&mut self) {
        if self.src.starts_with("#!") {
            while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                self.pos += 1;
            }
        }
    }

    /// Skips whitespace and comments. Inside an interpolation it stops at a line
    /// break, which the string lexer reports.
    fn skip_trivia(&mut self) {
        while self.pos < self.b.len() {
            let c = self.b[self.pos];
            match c {
                b'\n' => {
                    if self.interp_depth > 0 {
                        return;
                    }
                    self.pos += 1;
                    self.line_start = true;
                }
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'/' if self.peek(1) == Some(b'/') => {
                    while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.peek(1) == Some(b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        if self.pos >= self.b.len() {
                            self.error(start, start + 2, "a `/*` comment isn't closed");
                            break;
                        }
                        if self.b[self.pos] == b'*' && self.peek(1) == Some(b'/') {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                    self.line_start = false;
                }
                _ => return,
            }
        }
    }

    fn next_token(&mut self) -> Token {
        self.skip_trivia();
        let start = self.pos;
        if self.pos >= self.b.len() || (self.interp_depth > 0 && self.b[self.pos] == b'\n') {
            return Token {
                tok: Tok::Eof,
                span: Span::new(start, start),
            };
        }
        let c = self.b[self.pos];
        let at_line_start = self.line_start;
        self.line_start = false;
        let tok = if c == b'#'
            && at_line_start
            && self.interp_depth == 0
            && !self.prev_return
            && self.peek(1).is_some_and(|d| d.is_ascii_alphabetic())
        {
            self.directive(start)
        } else if is_ident_start(c) {
            self.ident_or_keyword()
        } else if c.is_ascii_digit() {
            self.number(start)
        } else if c == b'\'' {
            self.char_lit(start)
        } else if c == b'"' {
            self.string_lit(start)
        } else {
            match self.punct() {
                Some(p) => Tok::P(p),
                None => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                    let msg = if ch.is_alphabetic() {
                        format!(
                            "`{}`: identifiers use ASCII letters only (Unicode letters are undecided, GRAMMAR §8)",
                            ch
                        )
                    } else {
                        format!("unexpected character `{}`", ch)
                    };
                    self.error(start, self.pos, msg);
                    return self.next_token();
                }
            }
        };
        self.prev_return = tok == Tok::Kw(Kw::Return);
        Token {
            tok,
            span: Span::new(start, self.pos),
        }
    }

    fn punct(&mut self) -> Option<P> {
        let rest = &self.b[self.pos..];
        for (text, p) in PUNCTS {
            if rest.starts_with(text.as_bytes()) {
                self.pos += text.len();
                return Some(*p);
            }
        }
        None
    }

    fn ident_or_keyword(&mut self) -> Tok {
        let start = self.pos;
        while self.pos < self.b.len() && is_ident_continue(self.b[self.pos]) {
            self.pos += 1;
        }
        let word = &self.src[start..self.pos];
        if word == "_" {
            Tok::Underscore
        } else if let Some(k) = Kw::from_word(word) {
            Tok::Kw(k)
        } else {
            Tok::Ident(word.to_string())
        }
    }

    /// Reads digits of a base, allowing `_` between them. Returns the digits without `_`.
    fn digits(&mut self, is_digit: fn(u8) -> bool) -> String {
        let start = self.pos;
        let mut out = String::new();
        while self.pos < self.b.len() && (is_digit(self.b[self.pos]) || self.b[self.pos] == b'_') {
            if self.b[self.pos] != b'_' {
                out.push(self.b[self.pos] as char);
            }
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        if text.starts_with('_') || text.ends_with('_') {
            self.error(
                start,
                self.pos,
                "`_` can only separate digits, not start or end a number",
            );
        }
        out
    }

    fn number(&mut self, start: usize) -> Tok {
        let radix = match (self.b[self.pos], self.peek(1)) {
            (b'0', Some(b'x')) => 16,
            (b'0', Some(b'b')) => 2,
            _ => 10,
        };
        if radix != 10 {
            self.pos += 2;
            let digits = if radix == 16 {
                self.digits(|c| c.is_ascii_hexdigit())
            } else {
                self.digits(|c| c == b'0' || c == b'1')
            };
            if digits.is_empty() {
                self.error(
                    start,
                    self.pos,
                    "a number needs digits after its `0x` or `0b`",
                );
            }
            let suffix = self.suffix(start, false);
            let value = self.int_value(start, &digits, radix);
            return Tok::Int { value, suffix };
        }

        let mut text = self.digits(|c| c.is_ascii_digit());
        let mut is_float = false;
        if self.peek(0) == Some(b'.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            text.push('.');
            text.push_str(&self.digits(|c| c.is_ascii_digit()));
            is_float = true;
        }
        if matches!(self.peek(0), Some(b'e' | b'E')) {
            let sign = matches!(self.peek(1), Some(b'+' | b'-'));
            let digit_at = if sign { 2 } else { 1 };
            if self.peek(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                text.push('e');
                if sign {
                    text.push(self.b[self.pos + 1] as char);
                }
                self.pos += digit_at;
                text.push_str(&self.digits(|c| c.is_ascii_digit()));
                is_float = true;
            }
        }
        let suffix = self.suffix(start, true);
        if is_float || suffix.is_some_and(Prim::is_float) {
            if suffix.is_some_and(Prim::is_int) {
                self.error(
                    start,
                    self.pos,
                    "an integer suffix can't follow a number with a fraction or an exponent",
                );
            }
            return Tok::Float { text, suffix };
        }
        let value = self.int_value(start, &text, 10);
        Tok::Int { value, suffix }
    }

    fn int_value(&mut self, start: usize, digits: &str, radix: u32) -> u128 {
        match u128::from_str_radix(digits, radix) {
            Ok(v) => v,
            Err(_) if !digits.is_empty() => {
                self.error(
                    start,
                    self.pos,
                    "this number is too large for any integer type",
                );
                0
            }
            _ => 0,
        }
    }

    /// A type suffix glued to a number: `255u8`, `1.5f32`.
    fn suffix(&mut self, number_start: usize, decimal: bool) -> Option<Prim> {
        if !self.peek(0).is_some_and(is_ident_start) {
            return None;
        }
        let start = self.pos;
        while self.pos < self.b.len() && is_ident_continue(self.b[self.pos]) {
            self.pos += 1;
        }
        let word = &self.src[start..self.pos];
        match Prim::from_suffix(word) {
            Some(p) if p.is_float() && !decimal => {
                self.error(
                    start,
                    self.pos,
                    "a float suffix can't follow a hex or binary number",
                );
                None
            }
            Some(p) => Some(p),
            None => {
                let msg = format!(
                    "`{}` isn't a number suffix: use one of i8, i16, i32, i64, u8, u16, u32, u64, f32, f64",
                    word
                );
                self.error(number_start, self.pos, msg);
                None
            }
        }
    }

    /// Reads one escape after a `\`; returns the code point.
    fn escape(&mut self, in_string: bool) -> Option<char> {
        let start = self.pos;
        self.pos += 1; // the backslash
        let Some(c) = self.peek(0) else {
            self.error(start, self.pos, "an escape needs a character after `\\`");
            return None;
        };
        self.pos += 1;
        Some(match c {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'0' => '\0',
            b'\\' => '\\',
            b'\'' => '\'',
            b'"' => '"',
            b'$' if in_string => '$',
            b'$' => '$',
            b'u' => {
                if self.peek(0) != Some(b'{') {
                    self.error(start, self.pos, "write a Unicode escape as `\\u{1F600}`");
                    return None;
                }
                self.pos += 1;
                let digits_start = self.pos;
                while self.peek(0).is_some_and(|d| d.is_ascii_hexdigit()) {
                    self.pos += 1;
                }
                let digits = &self.src[digits_start..self.pos];
                if self.peek(0) != Some(b'}') || digits.is_empty() || digits.len() > 6 {
                    self.error(
                        start,
                        self.pos,
                        "write a Unicode escape as `\\u{` and 1 to 6 hex digits, then `}`",
                    );
                    if self.peek(0) == Some(b'}') {
                        self.pos += 1;
                    }
                    return None;
                }
                self.pos += 1;
                let value = u32::from_str_radix(digits, 16).unwrap();
                match char::from_u32(value) {
                    Some(ch) => ch,
                    None => {
                        self.error(start, self.pos, "a Unicode escape must be a code point up to 10FFFF and not a surrogate");
                        return None;
                    }
                }
            }
            _ => {
                let ch = self.src[self.pos - 1..].chars().next().unwrap();
                self.pos += ch.len_utf8() - 1;
                self.error(start, self.pos, format!("`\\{}` isn't an escape", ch));
                return None;
            }
        })
    }

    fn char_lit(&mut self, start: usize) -> Tok {
        self.pos += 1; // the opening quote
        let value = match self.peek(0) {
            None | Some(b'\n') | Some(b'\'') => {
                self.error(start, self.pos, "a character literal needs one character");
                if self.peek(0) == Some(b'\'') {
                    self.pos += 1;
                }
                return Tok::Char(0);
            }
            Some(b'\\') => self.escape(false).map_or(0, |c| c as u32),
            Some(_) => {
                let ch = self.src[self.pos..].chars().next().unwrap();
                self.pos += ch.len_utf8();
                ch as u32
            }
        };
        if self.peek(0) == Some(b'\'') {
            self.pos += 1;
        } else {
            while self.pos < self.b.len() && !matches!(self.b[self.pos], b'\'' | b'\n') {
                self.pos += 1;
            }
            if self.peek(0) == Some(b'\'') {
                self.pos += 1;
            }
            self.error(
                start,
                self.pos,
                "a character literal holds one character; a string uses double quotes",
            );
        }
        Tok::Char(value)
    }

    fn string_lit(&mut self, start: usize) -> Tok {
        self.pos += 1; // the opening quote
        let mut pieces = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek(0) {
                None | Some(b'\n') => {
                    self.error(
                        start,
                        self.pos,
                        "this string isn't closed (strings can't span lines)",
                    );
                    break;
                }
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    if let Some(c) = self.escape(true) {
                        text.push(c);
                    }
                }
                Some(b'$') if self.peek(1) == Some(b'{') => {
                    if !text.is_empty() {
                        pieces.push(StrPiece::Text(std::mem::take(&mut text)));
                    }
                    let interp_start = self.pos;
                    self.pos += 2;
                    let tokens = self.interpolation(interp_start);
                    pieces.push(StrPiece::Interp(tokens, Span::new(interp_start, self.pos)));
                }
                Some(_) => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                    text.push(ch);
                }
            }
        }
        if !text.is_empty() || pieces.is_empty() {
            pieces.push(StrPiece::Text(text));
        }
        Tok::Str(pieces)
    }

    /// The tokens of `${ ... }`, up to the matching `}`, which it consumes.
    fn interpolation(&mut self, start: usize) -> Vec<Token> {
        self.interp_depth += 1;
        let mut tokens = Vec::new();
        let mut depth = 0u32;
        loop {
            let t = self.next_token();
            match t.tok {
                Tok::Eof => {
                    self.error(
                        start,
                        self.pos,
                        "this `${` has no matching `}` before the line ends",
                    );
                    break;
                }
                Tok::P(P::LBrace) => depth += 1,
                Tok::P(P::RBrace) if depth == 0 => break,
                Tok::P(P::RBrace) => depth -= 1,
                _ => {}
            }
            tokens.push(t);
        }
        if tokens.is_empty() {
            self.error(
                start,
                self.pos,
                "an interpolation `${}` needs an expression",
            );
        }
        tokens.push(Token {
            tok: Tok::Eof,
            span: Span::new(self.pos, self.pos),
        });
        self.interp_depth -= 1;
        tokens
    }

    /// A directive line: `#name value`, `#if cond`, `#else`, `#endif`.
    fn directive(&mut self, start: usize) -> Tok {
        self.pos += 1; // `#`
        let word_start = self.pos;
        while self.pos < self.b.len() && is_ident_continue(self.b[self.pos]) {
            self.pos += 1;
        }
        let word = self.src[word_start..self.pos].to_string();
        let line_end = self.src[self.pos..]
            .find('\n')
            .map_or(self.b.len(), |i| self.pos + i);
        let mut rest = &self.src[self.pos..line_end];
        if let Some(i) = rest.find("//") {
            rest = &rest[..i];
        }
        let rest = rest.trim().to_string();
        let rest_start = self.pos;
        self.pos = line_end;
        match word.as_str() {
            "if" => {
                let cond = self.condition(&rest, rest_start);
                Tok::HashIf(cond)
            }
            "else" | "endif" => {
                if !rest.is_empty() {
                    self.error(
                        start,
                        line_end,
                        format!("`#{}` takes nothing after it", word),
                    );
                }
                if word == "else" {
                    Tok::HashElse
                } else {
                    Tok::HashEnd
                }
            }
            "elif" => {
                self.error(
                    start,
                    line_end,
                    "`#elif` is undecided (GRAMMAR §8); nest an `#if` inside the `#else`",
                );
                Tok::HashElse
            }
            _ => {
                if rest.is_empty() || rest.contains(char::is_whitespace) {
                    self.error(
                        start,
                        line_end,
                        format!("`#{}` takes one value: a word or an integer", word),
                    );
                }
                Tok::Setting {
                    name: word,
                    value: rest,
                }
            }
        }
    }

    /// Parses an `#if` condition (GRAMMAR §1.6).
    fn condition(&mut self, text: &str, offset: usize) -> Cond {
        let mut toks = Vec::new();
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b' ' | b'\t' => i += 1,
                b'(' | b')' | b'!' => {
                    toks.push((b[i] as char).to_string());
                    i += 1;
                }
                b'&' | b'|' if b.get(i + 1) == Some(&b[i]) => {
                    toks.push(text[i..i + 2].to_string());
                    i += 2;
                }
                c if is_ident_start(c) => {
                    let s = i;
                    while i < b.len() && is_ident_continue(b[i]) {
                        i += 1;
                    }
                    toks.push(text[s..i].to_string());
                }
                _ => {
                    self.error(
                        offset,
                        offset + text.len(),
                        "an `#if` condition uses names, `!`, `&&`, `||` and parentheses",
                    );
                    return Cond::Name(String::new(), None);
                }
            }
        }
        let mut p = CondParser {
            toks,
            pos: 0,
            ok: true,
        };
        let cond = p.or();
        if !p.ok || p.pos != p.toks.len() {
            self.error(
                offset,
                offset + text.len(),
                "this `#if` condition doesn't parse",
            );
        }
        cond
    }
}

struct CondParser {
    toks: Vec<String>,
    pos: usize,
    ok: bool,
}

impl CondParser {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(String::as_str)
    }

    fn or(&mut self) -> Cond {
        let mut left = self.and();
        while self.peek() == Some("||") {
            self.pos += 1;
            left = Cond::Or(Box::new(left), Box::new(self.and()));
        }
        left
    }

    fn and(&mut self) -> Cond {
        let mut left = self.not();
        while self.peek() == Some("&&") {
            self.pos += 1;
            left = Cond::And(Box::new(left), Box::new(self.not()));
        }
        left
    }

    fn not(&mut self) -> Cond {
        match self.peek() {
            Some("!") => {
                self.pos += 1;
                Cond::Not(Box::new(self.not()))
            }
            Some("(") => {
                self.pos += 1;
                let c = self.or();
                if self.peek() == Some(")") {
                    self.pos += 1;
                } else {
                    self.ok = false;
                }
                c
            }
            Some(name) if name.as_bytes()[0].is_ascii_alphabetic() || name.starts_with('_') => {
                let name = name.to_string();
                self.pos += 1;
                if self.peek() == Some("(") {
                    self.pos += 1;
                    let arg = self.peek().map(str::to_string);
                    self.pos += 1;
                    if self.peek() == Some(")") {
                        self.pos += 1;
                    } else {
                        self.ok = false;
                    }
                    Cond::Name(name, arg)
                } else {
                    Cond::Name(name, None)
                }
            }
            _ => {
                self.ok = false;
                Cond::Name(String::new(), None)
            }
        }
    }
}

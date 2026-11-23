//! Tokens (GRAMMAR §1).

use crate::source::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Kw(Kw),
    /// A lone `_`.
    Underscore,
    /// An integer literal's value, and its suffix if it has one.
    Int {
        value: u128,
        suffix: Option<Prim>,
    },
    /// A float literal's digits (without `_`), and its suffix if it has one.
    Float {
        text: String,
        suffix: Option<Prim>,
    },
    /// A character literal's code point.
    Char(u32),
    /// A string literal, split into text and `${...}` interpolations.
    Str(Vec<StrPiece>),
    P(P),
    /// `#name value` at the start of a line (GRAMMAR §1.6).
    Setting {
        name: String,
        value: String,
    },
    HashIf(Cond),
    HashElse,
    HashEnd,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrPiece {
    Text(String),
    /// The tokens of an interpolated expression, ending in `Eof`, and the span of `${...}`.
    Interp(Vec<Token>, Span),
}

/// The condition of an `#if` line.
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// `debug`, or `os(linux)`.
    Name(String, Option<String>),
    Not(Box<Cond>),
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
}

/// The number types, which are also the literal suffixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
}

impl Prim {
    pub fn name(self) -> &'static str {
        match self {
            Prim::Bool => "bool",
            Prim::I8 => "i8",
            Prim::I16 => "i16",
            Prim::I32 => "i32",
            Prim::I64 => "i64",
            Prim::I128 => "i128",
            Prim::U8 => "u8",
            Prim::U16 => "u16",
            Prim::U32 => "u32",
            Prim::U64 => "u64",
            Prim::U128 => "u128",
            Prim::F32 => "f32",
            Prim::F64 => "f64",
        }
    }

    pub fn is_int(self) -> bool {
        !matches!(self, Prim::Bool | Prim::F32 | Prim::F64)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Prim::F32 | Prim::F64)
    }

    pub fn from_suffix(s: &str) -> Option<Prim> {
        Some(match s {
            "i8" => Prim::I8,
            "i16" => Prim::I16,
            "i32" => Prim::I32,
            "i64" => Prim::I64,
            "i128" => Prim::I128,
            "u8" => Prim::U8,
            "u16" => Prim::U16,
            "u32" => Prim::U32,
            "u64" => Prim::U64,
            "u128" => Prim::U128,
            "f32" => Prim::F32,
            "f64" => Prim::F64,
            _ => return None,
        })
    }
}

macro_rules! keywords {
    ($($variant:ident => $text:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Kw { $($variant,)* }

        impl Kw {
            pub fn from_word(s: &str) -> Option<Kw> {
                match s { $($text => Some(Kw::$variant),)* _ => None }
            }

            pub fn as_str(self) -> &'static str {
                match self { $(Kw::$variant => $text,)* }
            }
        }
    };
}

keywords! {
    // Types
    Bool => "bool", I8 => "i8", I16 => "i16", I32 => "i32", I64 => "i64", I128 => "i128",
    U8 => "u8", U16 => "u16", U32 => "u32", U64 => "u64", U128 => "u128",
    F32 => "f32", F64 => "f64",
    String => "string", Void => "void", Func => "func",
    // Declarations
    Package => "package", Import => "import", As => "as", Class => "class",
    Interface => "interface", Enum => "enum", Implements => "implements",
    Static => "static", Public => "public", Private => "private", Mut => "mut",
    Extern => "extern",
    // Statements
    If => "if", Else => "else", While => "while", Do => "do", For => "for",
    Break => "break", Continue => "continue", Return => "return", Switch => "switch",
    Case => "case", Default => "default", Yield => "yield", Var => "var", Defer => "defer",
    // Expressions
    New => "new", This => "this", Null => "null", True => "true", False => "false",
    Instanceof => "instanceof",
    // Reserved, not used
    Extends => "extends", Protected => "protected", Abstract => "abstract", Final => "final",
    Struct => "struct", Super => "super", Try => "try", Catch => "catch", Finally => "finally",
    Throw => "throw", Long => "long", Short => "short", Byte => "byte", Double => "double",
    Char => "char", Delete => "delete",
}

impl Kw {
    /// The number type a keyword names, if it names one.
    pub fn prim(self) -> Option<Prim> {
        Some(match self {
            Kw::Bool => Prim::Bool,
            Kw::I8 => Prim::I8,
            Kw::I16 => Prim::I16,
            Kw::I32 => Prim::I32,
            Kw::I64 => Prim::I64,
            Kw::U8 => Prim::U8,
            Kw::U16 => Prim::U16,
            Kw::U32 => Prim::U32,
            Kw::U64 => Prim::U64,
            Kw::I128 => Prim::I128,
            Kw::U128 => Prim::U128,
            Kw::F32 => Prim::F32,
            Kw::F64 => Prim::F64,
            _ => return None,
        })
    }

    /// Why a reserved keyword can't be used, for reserved keywords the language doesn't use.
    pub fn reserved_reason(self) -> Option<&'static str> {
        Some(match self {
            Kw::Extends | Kw::Super | Kw::Abstract | Kw::Protected => {
                "Zinkite has no inheritance; use an interface (DESIGN §6.1)"
            }
            Kw::Try | Kw::Catch | Kw::Finally | Kw::Throw => {
                "nothing is thrown in Zinkite: failures are status codes (DESIGN §7)"
            }
            Kw::Long => "the type is spelled `i64`",
            Kw::Short => "the type is spelled `i16`",
            Kw::Byte => "the type is spelled `u8`",
            Kw::Double => "the type is spelled `f64`",
            Kw::Char => "a code point is a `u32` (DESIGN §9.1)",
            Kw::Struct => "declare a `class`",
            Kw::Final => "`final` is undecided (GRAMMAR §8)",
            Kw::Delete => "there are no destructors; use `defer` (DESIGN §6.4)",
            _ => return None,
        })
    }
}

/// Operators and punctuation (GRAMMAR §1.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum P {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semi,
    Comma,
    Dot,
    Colon,
    Question,
    At,
    Hash,
    Dollar,
    FatArrow,
    Amp,
    Assign,
    DotAssign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
    ShlAssign,
    ShrAssign,
    WrapAddAssign,
    WrapSubAssign,
    WrapMulAssign,
    SatAddAssign,
    SatSubAssign,
    SatMulAssign,
    /// `+#=`, `-#=`, `*#=`: compound assignments that check for overflow.
    CheckAddAssign,
    CheckSubAssign,
    CheckMulAssign,
    OrOr,
    AndAnd,
    Pipe,
    Caret,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    Shl,
    Shr,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    WrapAdd,
    WrapSub,
    WrapMul,
    SatAdd,
    SatSub,
    SatMul,
    /// `+#`, `-#`, `*#`: arithmetic that fails with a status on overflow (DESIGN §9.2).
    CheckAdd,
    CheckSub,
    CheckMul,
    /// `+>`, `+<`, `+^`: insert at the end, at the front, at the cursor.
    InsertEnd,
    InsertFront,
    InsertCursor,
    /// `->`, `-<`, `-^`, `^-`: remove from the end, the front, before and after the cursor.
    RemoveEnd,
    RemoveFront,
    RemoveBefore,
    RemoveAfter,
    Bang,
    Tilde,
    PlusPlus,
    MinusMinus,
}

/// Every operator's spelling, longest first, for longest-match lexing.
pub const PUNCTS: &[(&str, P)] = &[
    ("<<=", P::ShlAssign),
    (">>=", P::ShrAssign),
    ("+%=", P::WrapAddAssign),
    ("-%=", P::WrapSubAssign),
    ("*%=", P::WrapMulAssign),
    ("+#=", P::CheckAddAssign),
    ("-#=", P::CheckSubAssign),
    ("*#=", P::CheckMulAssign),
    ("+|=", P::SatAddAssign),
    ("-|=", P::SatSubAssign),
    ("*|=", P::SatMulAssign),
    ("=>", P::FatArrow),
    (".=", P::DotAssign),
    ("+=", P::PlusAssign),
    ("-=", P::MinusAssign),
    ("*=", P::StarAssign),
    ("/=", P::SlashAssign),
    ("%=", P::PercentAssign),
    ("&=", P::AmpAssign),
    ("|=", P::PipeAssign),
    ("^=", P::CaretAssign),
    ("||", P::OrOr),
    ("&&", P::AndAnd),
    ("==", P::EqEq),
    ("!=", P::NotEq),
    ("<=", P::Le),
    (">=", P::Ge),
    ("<<", P::Shl),
    (">>", P::Shr),
    ("+%", P::WrapAdd),
    ("-%", P::WrapSub),
    ("*%", P::WrapMul),
    ("+#", P::CheckAdd),
    ("-#", P::CheckSub),
    ("*#", P::CheckMul),
    ("+|", P::SatAdd),
    ("-|", P::SatSub),
    ("*|", P::SatMul),
    ("+>", P::InsertEnd),
    ("+<", P::InsertFront),
    ("+^", P::InsertCursor),
    ("->", P::RemoveEnd),
    ("-<", P::RemoveFront),
    ("-^", P::RemoveBefore),
    ("^-", P::RemoveAfter),
    ("++", P::PlusPlus),
    ("--", P::MinusMinus),
    ("(", P::LParen),
    (")", P::RParen),
    ("{", P::LBrace),
    ("}", P::RBrace),
    ("[", P::LBracket),
    ("]", P::RBracket),
    (";", P::Semi),
    (",", P::Comma),
    (".", P::Dot),
    (":", P::Colon),
    ("?", P::Question),
    ("@", P::At),
    ("#", P::Hash),
    ("$", P::Dollar),
    ("&", P::Amp),
    ("=", P::Assign),
    ("|", P::Pipe),
    ("^", P::Caret),
    ("<", P::Lt),
    (">", P::Gt),
    ("+", P::Plus),
    ("-", P::Minus),
    ("*", P::Star),
    ("/", P::Slash),
    ("%", P::Percent),
    ("!", P::Bang),
    ("~", P::Tilde),
];

impl P {
    pub fn as_str(self) -> &'static str {
        PUNCTS
            .iter()
            .find(|(_, p)| *p == self)
            .map(|(s, _)| *s)
            .unwrap_or("?")
    }
}

impl Tok {
    /// A short description for error messages: "`;`", "`while`", "an identifier".
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("`{}`", name),
            Tok::Kw(k) => format!("`{}`", k.as_str()),
            Tok::Underscore => "`_`".into(),
            Tok::Int { .. } => "an integer".into(),
            Tok::Float { .. } => "a number".into(),
            Tok::Char(_) => "a character literal".into(),
            Tok::Str(_) => "a string".into(),
            Tok::P(p) => format!("`{}`", p.as_str()),
            Tok::Setting { name, .. } => format!("`#{}`", name),
            Tok::HashIf(_) => "`#if`".into(),
            Tok::HashElse => "`#else`".into(),
            Tok::HashEnd => "`#endif`".into(),
            Tok::Eof => "the end of the file".into(),
        }
    }
}

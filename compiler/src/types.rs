//! Semantic types: what the type checker works with (DESIGN §9, §4, §10).

use crate::token::Prim;

pub type ClassId = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Prim(Prim),
    Void,
    /// The type of `null`.
    Null,
    /// An integer literal that hasn't been given a type yet (DESIGN §9.1).
    /// `value` is known for literals and constant arithmetic on them.
    IntLit {
        value: Option<i128>,
        is_char: bool,
    },
    /// A float literal that hasn't been given a type yet.
    FloatLit,
    /// A class, root, interface or enum, with type arguments.
    Class(ClassId, Vec<Ty>),
    /// A type parameter in scope.
    Param(String),
    Array(Box<Ty>, ArrayKind),
    /// `T[:]`; `string` is `Slice(u8)`.
    Slice(Box<Ty>),
    /// `T[:]&`, a writable view.
    View(Box<Ty>),
    /// `T&`, and links.
    Ref(Box<Ty>),
    /// `T&?` or a nullable function.
    Nullable(Box<Ty>),
    Func(Vec<Ty>, Box<Ty>),
    /// Multiple results (DESIGN §9.5).
    Tuple(Vec<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    Pool(Box<Ty>),
    Handle(Box<Ty>),
    /// What a for-each over a `hashmap` gives: `key` and `value`.
    MapEntry(Box<Ty>, Box<Ty>),
    /// A library iterator yielding the type: `lines()`, `codePoints()`.
    Iter(Box<Ty>),
    /// A class or number type named as a value, before `.`: `Kind.OBJECT`, `File.open`.
    TypeName(Box<Ty>),
    /// A static-only namespace: `status`, `math`.
    Namespace(&'static str),
    /// An opaque C struct reached through a pointer, as `FILE *` (DESIGN §12.6): a
    /// value that can be compared, set to `null` and passed back to C, never looked into.
    CHandle(String),
    /// From a library the checker doesn't model yet, or after an error. Never reported.
    Unknown,
}

/// An array's slots and cap, from its type (DESIGN §4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayKind {
    pub start: char,
    pub cursor: char,
    pub end: char,
    pub cap: CapKind,
    /// A capped array's capacity, when it is a number; 0 otherwise.
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapKind {
    /// On a data stack, or sized at construction when frozen.
    None,
    /// Inline, with a capacity.
    Capped,
    /// `*`
    Varint,
    /// `$`
    Chunked,
}

impl ArrayKind {
    pub fn frozen() -> ArrayKind {
        ArrayKind {
            start: '.',
            cursor: '.',
            end: '.',
            cap: CapKind::None,
            size: 0,
        }
    }

    pub fn grow() -> ArrayKind {
        ArrayKind {
            start: '.',
            cursor: '.',
            end: '>',
            cap: CapKind::None,
            size: 0,
        }
    }

    fn inserts(c: char) -> bool {
        c == '>' || c == '~'
    }

    fn removes(c: char) -> bool {
        c == '<' || c == '~'
    }

    pub fn push_end(&self) -> bool {
        Self::inserts(self.end)
    }
    pub fn pop_end(&self) -> bool {
        Self::removes(self.end)
    }
    pub fn push_front(&self) -> bool {
        Self::inserts(self.start)
    }
    pub fn pop_front(&self) -> bool {
        Self::removes(self.start)
    }
    pub fn cursor_insert(&self) -> bool {
        Self::inserts(self.cursor)
    }
    pub fn cursor_remove(&self) -> bool {
        Self::removes(self.cursor)
    }
    pub fn has_cursor(&self) -> bool {
        self.cursor != '.'
    }
    pub fn is_frozen(&self) -> bool {
        self.start == '.' && self.cursor == '.' && self.end == '.'
    }
    /// Can grow or shrink, so it needs its own region (a "growable" field).
    pub fn is_editable(&self) -> bool {
        !self.is_frozen() && self.cap != CapKind::Capped
    }

    /// Converts to a slice (DESIGN §4.10): stored in order in one block, or capped, since
    /// a capped ring or gap buffer is small enough to be rearranged into one block first.
    pub fn sliceable(&self) -> bool {
        self.contiguous() || self.cap == CapKind::Capped
    }

    /// A capped array whose elements can move (REFERENCES §5.4): a gap buffer, whose
    /// edits move them, or a ring that changes at both ends and so can wrap, after which
    /// its next slice rotates it. References and slices into one end at its next insert,
    /// and a slice of one ends references to its elements.
    pub fn moves_elements(&self) -> bool {
        self.cap == CapKind::Capped
            && (self.has_cursor()
                || (self.start != '.'
                    && self.end != '.'
                    && (Self::inserts(self.start) || Self::inserts(self.end))))
    }

    /// Stored in order in one block.
    pub fn contiguous(&self) -> bool {
        !self.has_cursor()
            && self.cap != CapKind::Chunked
            && !(self.start != '.' && self.end == '.')
            && !(self.cap == CapKind::Capped && self.start != '.')
    }

    /// Whether an array of kind `self` offers every capability `param` asks for (DESIGN §4.3).
    pub fn satisfies(&self, param: &ArrayKind) -> bool {
        let need = |want: char, have: char| match want {
            '.' => true,
            '>' => Self::inserts(have),
            '<' => Self::removes(have),
            _ => Self::inserts(have) && Self::removes(have),
        };
        need(param.start, self.start)
            && need(param.cursor, self.cursor)
            && need(param.end, self.end)
    }

    pub fn slots(&self) -> String {
        if self.start == self.cursor && self.cursor == self.end {
            self.start.to_string()
        } else {
            format!("{}{}{}", self.start, self.cursor, self.end)
        }
    }
}

impl Ty {
    pub fn string() -> Ty {
        Ty::Slice(Box::new(Ty::Prim(Prim::U8)))
    }

    pub fn u8() -> Ty {
        Ty::Prim(Prim::U8)
    }

    pub fn i64() -> Ty {
        Ty::Prim(Prim::I64)
    }

    pub fn bool() -> Ty {
        Ty::Prim(Prim::Bool)
    }

    pub fn int_lit() -> Ty {
        Ty::IntLit {
            value: None,
            is_char: false,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Ty::Slice(e) if **e == Ty::Prim(Prim::U8))
    }

    pub fn prim(&self) -> Option<Prim> {
        match self {
            Ty::Prim(p) => Some(*p),
            _ => None,
        }
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Ty::IntLit { .. }) || self.prim().is_some_and(Prim::is_int)
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::FloatLit) || self.prim().is_some_and(Prim::is_float)
    }

    pub fn is_number(&self) -> bool {
        self.is_int() || self.is_float()
    }

    /// Strips one reference, for reading through it.
    pub fn unref(&self) -> &Ty {
        match self {
            Ty::Ref(t) => t,
            t => t,
        }
    }

    /// A literal's type once nothing else decides it (DESIGN §9.1).
    pub fn default_lit(&self) -> Ty {
        match self {
            Ty::IntLit { is_char: true, .. } => Ty::Prim(Prim::U32),
            Ty::IntLit { .. } => Ty::Prim(Prim::I64),
            Ty::FloatLit => Ty::Prim(Prim::F64),
            t => t.clone(),
        }
    }

    /// The element type of an array, slice or view.
    pub fn elem(&self) -> Option<&Ty> {
        match self.unref() {
            Ty::Array(e, _) | Ty::Slice(e) | Ty::View(e) => Some(e),
            _ => None,
        }
    }

    /// Replaces type parameters by name.
    pub fn subst(&self, map: &[(String, Ty)]) -> Ty {
        if map.is_empty() {
            return self.clone();
        }
        let s = |t: &Ty| Box::new(t.subst(map));
        match self {
            Ty::Param(n) => map
                .iter()
                .find(|(k, _)| k == n)
                .map_or(self.clone(), |(_, t)| t.clone()),
            Ty::Class(id, args) => Ty::Class(*id, args.iter().map(|a| a.subst(map)).collect()),
            Ty::Array(e, k) => Ty::Array(s(e), *k),
            Ty::Slice(e) => Ty::Slice(s(e)),
            Ty::View(e) => Ty::View(s(e)),
            Ty::Ref(e) => Ty::Ref(s(e)),
            Ty::Nullable(e) => Ty::Nullable(s(e)),
            Ty::Func(ps, r) => Ty::Func(ps.iter().map(|p| p.subst(map)).collect(), s(r)),
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| t.subst(map)).collect()),
            Ty::Map(k, v) => Ty::Map(s(k), s(v)),
            Ty::Set(e) => Ty::Set(s(e)),
            Ty::Pool(e) => Ty::Pool(s(e)),
            Ty::Handle(e) => Ty::Handle(s(e)),
            Ty::MapEntry(k, v) => Ty::MapEntry(s(k), s(v)),
            Ty::Iter(e) => Ty::Iter(s(e)),
            Ty::TypeName(e) => Ty::TypeName(s(e)),
            _ => self.clone(),
        }
    }

    pub fn mentions_param(&self) -> bool {
        match self {
            Ty::Param(_) => true,
            Ty::Class(_, args) => args.iter().any(Ty::mentions_param),
            Ty::Array(e, _)
            | Ty::Slice(e)
            | Ty::View(e)
            | Ty::Ref(e)
            | Ty::Nullable(e)
            | Ty::Set(e)
            | Ty::Pool(e)
            | Ty::Handle(e)
            | Ty::Iter(e)
            | Ty::TypeName(e) => e.mentions_param(),
            Ty::Func(ps, r) => ps.iter().any(Ty::mentions_param) || r.mentions_param(),
            Ty::Tuple(ts) => ts.iter().any(Ty::mentions_param),
            Ty::Map(k, v) | Ty::MapEntry(k, v) => k.mentions_param() || v.mentions_param(),
            _ => false,
        }
    }
}

pub fn bits(p: Prim) -> u32 {
    match p {
        Prim::Bool => 1,
        Prim::I8 | Prim::U8 => 8,
        Prim::I16 | Prim::U16 => 16,
        Prim::I32 | Prim::U32 | Prim::F32 => 32,
        Prim::I64 | Prim::U64 | Prim::F64 => 64,
        Prim::I128 | Prim::U128 => 128,
    }
}

pub fn signed(p: Prim) -> bool {
    matches!(p, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::I128)
}

/// Whether every value of `from` is a value of `to`: the implicit conversions (DESIGN §9.1).
pub fn lossless(from: Prim, to: Prim) -> bool {
    if from == to {
        return true;
    }
    match (from.is_int(), to.is_int()) {
        (true, true) => {
            if signed(from) == signed(to) {
                bits(to) >= bits(from)
            } else {
                !signed(from) && bits(to) > bits(from)
            }
        }
        (true, false) => match to {
            Prim::F32 => bits(from) <= 16,
            Prim::F64 => bits(from) <= 32,
            _ => false,
        },
        (false, false) => from == Prim::F32 && to == Prim::F64,
        (false, true) => false,
    }
}

/// Whether an integer literal's value fits a type. Values above `i128.MAX` aren't held
/// as an `i128`; see `fits_u128`.
pub fn fits(value: i128, p: Prim) -> bool {
    let (lo, hi): (i128, i128) = match p {
        Prim::I8 => (i8::MIN as i128, i8::MAX as i128),
        Prim::I16 => (i16::MIN as i128, i16::MAX as i128),
        Prim::I32 => (i32::MIN as i128, i32::MAX as i128),
        Prim::I64 => (i64::MIN as i128, i64::MAX as i128),
        Prim::U8 => (0, u8::MAX as i128),
        Prim::U16 => (0, u16::MAX as i128),
        Prim::U32 => (0, u32::MAX as i128),
        Prim::U64 => (0, u64::MAX as i128),
        Prim::I128 => (i128::MIN, i128::MAX),
        Prim::U128 => (0, i128::MAX),
        Prim::F32 | Prim::F64 => return true,
        Prim::Bool => return false,
    };
    value >= lo && value <= hi
}

/// Whether a literal as written, up to `u128.MAX`, fits a type.
pub fn fits_u128(value: u128, p: Prim) -> bool {
    match i128::try_from(value) {
        Ok(v) => fits(v, p),
        Err(_) => p == Prim::U128 || p.is_float(),
    }
}

/// The built-in generic bounds (DESIGN §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundKind {
    Sint,
    Uint,
    Int,
    Float,
    Number,
    Ordered,
    Hashable,
}

impl BoundKind {
    pub fn from_name(name: &str) -> Option<BoundKind> {
        Some(match name {
            "sint" => BoundKind::Sint,
            "uint" => BoundKind::Uint,
            "int" => BoundKind::Int,
            "float" => BoundKind::Float,
            "number" => BoundKind::Number,
            "ordered" => BoundKind::Ordered,
            "hashable" => BoundKind::Hashable,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BoundKind::Sint => "sint",
            BoundKind::Uint => "uint",
            BoundKind::Int => "int",
            BoundKind::Float => "float",
            BoundKind::Number => "number",
            BoundKind::Ordered => "ordered",
            BoundKind::Hashable => "hashable",
        }
    }

    /// Whether a bound gives every operation of `other`: `int` implies `number`, `ordered`, `hashable`.
    pub fn implies(self, other: BoundKind) -> bool {
        use BoundKind::*;
        self == other
            || match self {
                Sint | Uint => matches!(other, Int | Number | Ordered | Hashable),
                Int => matches!(other, Number | Ordered | Hashable),
                Float => matches!(other, Number | Ordered),
                Number => other == Ordered,
                Ordered | Hashable => false,
            }
    }

    /// Whether a number type satisfies the bound.
    pub fn admits_prim(self, p: Prim) -> bool {
        use BoundKind::*;
        match self {
            Sint => p.is_int() && signed(p),
            Uint => p.is_int() && !signed(p),
            Int => p.is_int(),
            Float => p.is_float(),
            Number => p.is_int() || p.is_float(),
            Ordered => true,
            Hashable => p != Prim::F32 && p != Prim::F64,
        }
    }
}

/// A type parameter's bound: a built-in one, or an interface.
#[derive(Clone, Debug, PartialEq)]
pub enum Bound {
    Builtin(BoundKind),
    Interface(Ty),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TParam {
    pub name: String,
    pub bounds: Vec<Bound>,
}

impl TParam {
    pub fn has(&self, k: BoundKind) -> bool {
        self.bounds
            .iter()
            .any(|b| matches!(b, Bound::Builtin(b) if b.implies(k)))
    }
}

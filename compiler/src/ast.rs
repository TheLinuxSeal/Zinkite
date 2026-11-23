//! The syntax tree the parser builds (GRAMMAR §2–§6).

use crate::source::Span;
use crate::token::{Cond, Prim};

#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// A name that may also be `_`, which discards.
#[derive(Clone, Debug, PartialEq)]
pub enum Name {
    Ident(Ident),
    Discard(Span),
}

/// The status destinations after a target (DESIGN §7.3): `@a` takes the status of the
/// call itself, `@@b` the outermost failure anywhere inside it, and `@@@t` its trail.
/// Either of the last two also catches a failure nothing read inside the call.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Capture {
    pub one: Option<Name>,
    pub deep: Option<Name>,
    pub trail: Option<Name>,
}

impl Capture {
    /// Whether it catches failures nothing read inside the call, too (DESIGN §7.6).
    pub fn catches_inside(&self) -> bool {
        self.deep.is_some() || self.trail.is_some()
    }

    /// The `u8` destinations: `@a` and `@@b`.
    pub fn statuses(&self) -> impl Iterator<Item = &Name> {
        self.one.iter().chain(self.deep.iter())
    }

    pub fn names(&self) -> impl Iterator<Item = &Name> {
        self.one.iter().chain(self.deep.iter()).chain(self.trail.iter())
    }
}

/// `a.b.c`
#[derive(Clone, Debug, PartialEq)]
pub struct QualName {
    pub parts: Vec<Ident>,
    pub span: Span,
}

impl QualName {
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

// ---------------------------------------------------------------- files

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub settings: Vec<SettingItem>,
    pub package: Option<QualName>,
    pub imports: Vec<Import>,
    /// `extern import stdio.h;` and `extern link m;` (DESIGN §12.6).
    pub externs: Vec<ExternItem>,
    pub items: Vec<TopItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExternItem {
    pub kind: ExternKind,
    /// The header's path or the library's name, as written: `SDL3/SDL.h`, `m`.
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternKind {
    Import,
    Link,
}

/// An `#if` block, holding items of one kind.
#[derive(Clone, Debug, PartialEq)]
pub struct CondBlock<T> {
    pub cond: Cond,
    pub then: Vec<T>,
    pub els: Option<Vec<T>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingItem {
    Setting {
        name: String,
        value: String,
        span: Span,
    },
    If(CondBlock<SettingItem>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub path: QualName,
    pub kind: ImportKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImportKind {
    /// `import a.b.C;`
    Single,
    /// `import a.b.*;`
    Wildcard,
    /// `import a.b.C as D;`
    Alias(Ident),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TopItem {
    Type(TypeDecl),
    /// Script files only.
    Field(FieldDecl),
    /// Script files only.
    Method(MethodDecl),
    If(CondBlock<TopItem>),
}

// ---------------------------------------------------------------- declarations

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modifier {
    Public,
    Private,
    Static,
    Mut,
    /// A C function, with its C name when that differs: `extern("glClearColor")`.
    Extern(Option<String>),
    /// Compiled once more for each wider vector unit, the widest the CPU has chosen when
    /// it runs (DESIGN §12.8). A keyword only here, so `simd` is still a name elsewhere.
    Simd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDecl {
    pub modifiers: Vec<Modifier>,
    pub kind: TypeDeclKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDeclKind {
    Class(ClassDecl),
    /// A root (DESIGN §6.6): a class whose growable fields each get their own data stack.
    Root(ClassDecl),
    Interface(InterfaceDecl),
    Enum(EnumDecl),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassDecl {
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub implements: Vec<Type>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceDecl {
    pub name: Ident,
    pub type_params: Vec<TypeParam>,
    pub members: Vec<InterfaceMember>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InterfaceMember {
    Method(MethodDecl),
    If(CondBlock<InterfaceMember>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub constants: Vec<Ident>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeParam {
    pub name: Ident,
    /// `implements A & B`
    pub bounds: Vec<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Member {
    Field(FieldDecl),
    Method(MethodDecl),
    Ctor(CtorDecl),
    If(CondBlock<Member>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub modifiers: Vec<Modifier>,
    pub ty: Type,
    pub vars: Vec<VarDeclarator>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VarDeclarator {
    pub name: Ident,
    pub init: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDecl {
    pub modifiers: Vec<Modifier>,
    pub type_params: Vec<TypeParam>,
    pub ret: ReturnType,
    pub name: Ident,
    pub params: Vec<Param>,
    /// `None` in an interface.
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CtorDecl {
    pub modifiers: Vec<Modifier>,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub ty: Type,
    pub name: Name,
    pub span: Span,
}

// ---------------------------------------------------------------- types

#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    Prim(Prim),
    /// `string`, the same type as `u8[:]`.
    String,
    /// A class, interface, enum, type parameter or built-in generic: `Node<K>`, `zink.io.File`.
    Named {
        path: QualName,
        args: Vec<Type>,
    },
    /// `func(A, B => R)`; no parameters for `func(void => ...)`.
    Func {
        params: Vec<Type>,
        result: Box<ReturnType>,
    },
    /// `T[sce cap]`
    Array {
        elem: Box<Type>,
        slots: Slots,
    },
    /// `T[:]`
    Slice(Box<Type>),
    /// `T&`, or a link `T&<i32>`, `T&<u32 in arr>`.
    Ref {
        inner: Box<Type>,
        link: Option<Link>,
    },
    /// `T&?` or a nullable function.
    Nullable(Box<Type>),
}

/// An array's slot characters and cap. `chars` has one or three of `.`, `>`, `<`, `~`.
#[derive(Clone, Debug, PartialEq)]
pub struct Slots {
    pub chars: Vec<char>,
    pub cap: Option<Cap>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cap {
    Int(u64),
    Const(QualName),
    /// `*`: a varint length field.
    Varint,
    /// `$`: a chunked index.
    Chunked,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// `i8`, `i16`, `i32` for a self-relative link; `u8`, `u16`, `u32` for a pool link.
    pub width: Prim,
    /// The array a pool link is relative to.
    pub pool: Option<QualName>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReturnType {
    Void,
    Type(Type),
    Tuple(Vec<Type>),
}

// ---------------------------------------------------------------- statements

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    Block(Block),
    Local(LocalDecl),
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// `while (T x : call) [@why] body` (DESIGN §9.4).
    Repeat {
        target: LoopTarget,
        source: Expr,
        capture: Option<Capture>,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
    },
    For {
        init: Option<ForInit>,
        cond: Option<Expr>,
        update: Vec<Expr>,
        body: Box<Stmt>,
    },
    ForEach {
        ty: LocalType,
        name: Name,
        iter: Expr,
        body: Box<Stmt>,
    },
    Switch(Switch),
    Break(Option<Ident>),
    Continue(Option<Ident>),
    Return {
        status: Option<StatusWrite>,
        value: Option<Expr>,
    },
    Destructure {
        targets: Vec<Target>,
        capture: Option<Capture>,
        value: Expr,
    },
    Defer(Box<Stmt>),
    Yield(Expr),
    Labeled {
        label: Ident,
        stmt: Box<Stmt>,
    },
    Expr(Expr),
    If_(CondBlock<Stmt>),
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LocalType {
    Type(Type),
    /// `var`
    Var,
    /// `var&`
    VarRef,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalDecl {
    pub ty: LocalType,
    pub decls: Vec<LocalDeclarator>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalDeclarator {
    pub name: Ident,
    /// `@err`, `@@err`, `@@@trail`
    pub capture: Option<Capture>,
    pub init: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ForInit {
    Decl(LocalDecl),
    Exprs(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoopTarget {
    Single(LocalType, Name),
    Tuple(Vec<LoopItem>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoopItem {
    Decl(LocalType, Name),
    Discard(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Declares a new local: `string a`.
    Decl(LocalType, Name),
    /// An existing variable, or `_`.
    Existing(Name),
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatusWrite {
    Int(u128),
    Name(QualName),
    Expr(Expr),
    /// `return ## e;`: `e`'s value and its status, as `@` reads it (DESIGN §7.2).
    Pass,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Switch {
    pub scrutinee: Box<Expr>,
    pub rules: Vec<SwitchRule>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchRule {
    /// `None` for `default`.
    pub labels: Option<Vec<CaseLabel>>,
    pub body: SwitchBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SwitchBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaseLabel {
    Int {
        negative: bool,
        value: u128,
        span: Span,
    },
    Char(u32, Span),
    Str(String, Span),
    Ident(Ident),
}

// ---------------------------------------------------------------- expressions

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int {
        value: u128,
        suffix: Option<Prim>,
    },
    Float {
        text: String,
        suffix: Option<Prim>,
    },
    Char(u32),
    Str(Vec<StrPart>),
    Bool(bool),
    Null,
    Ident(String),
    /// `_`, only as an assignment target.
    Discard,
    This,
    /// `this.new(...)`
    ThisNew(Vec<Arg>),
    /// `i64.parse`, `u64.MAX`
    PrimMember {
        prim: Prim,
        name: Ident,
    },
    /// `new C(...)`, or `new(...)` with the class inferred.
    New {
        class: Option<Type>,
        args: Vec<Arg>,
    },
    ArrayLit(Vec<Expr>),
    Switch(Switch),
    Paren(Box<Expr>),
    /// Only after `return`, on the right of a destructuring, and as a lambda's body.
    Tuple(Vec<Expr>),
    Lambda {
        params: Vec<Param>,
        body: Box<LambdaBody>,
    },
    /// `x = e`, `x .= e`, `x += e`, and `x @err = e`.
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        capture: Option<Capture>,
        value: Box<Expr>,
    },
    /// `@err = e`, `@@err = e`, `@@@trail = e`
    StatusAssign {
        capture: Capture,
        value: Box<Expr>,
    },
    Cond {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    InstanceOf {
        expr: Box<Expr>,
        ty: Type,
        bind: Option<Ident>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    /// `@e`, or `@@e` (`deep`), which also catches failures nothing read inside it
    StatusRead { expr: Box<Expr>, deep: bool },
    Cast {
        ty: CastTarget,
        mode: CastMode,
        expr: Box<Expr>,
    },
    Postfix {
        op: PostfixOp,
        expr: Box<Expr>,
    },
    Member {
        expr: Box<Expr>,
        name: Ident,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    Index {
        expr: Box<Expr>,
        mode: IndexMode,
        index: Box<Expr>,
    },
    Slice {
        expr: Box<Expr>,
        lo: Option<Bound>,
        hi: Option<Bound>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Text(String),
    Interp(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LambdaBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// Written `&x`: accepted with a warning (DESIGN §5.1).
    pub marked: bool,
    pub expr: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    /// `^k`
    pub from_end: bool,
    pub expr: Box<Expr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexMode {
    Checked,
    /// `[%i]`
    Wrap,
    /// `[|i]`
    Clamp,
    /// `[^k]`
    FromEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CastTarget {
    Prim(Prim),
    /// A type parameter in scope: `(T) x`.
    Param(Ident),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastMode {
    Checked,
    /// `(u8%) x`
    Wrap,
    /// `(u8|) x`
    Saturate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Rebind,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    CheckAdd,
    CheckSub,
    CheckMul,
    SatAdd,
    SatSub,
    SatMul,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    BitOr,
    BitXor,
    BitAnd,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    InsertEnd,
    InsertFront,
    InsertCursor,
    Shl,
    Shr,
    Add,
    Sub,
    CheckAdd,
    CheckSub,
    SatAdd,
    SatSub,
    Mul,
    Div,
    Rem,
    CheckMul,
    SatMul,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Neg,
    Not,
    BitNot,
    PreInc,
    PreDec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostfixOp {
    Inc,
    Dec,
    /// `->`
    RemoveEnd,
    /// `-<`
    RemoveFront,
    /// `-^`
    RemoveBefore,
    /// `^-`
    RemoveAfter,
}

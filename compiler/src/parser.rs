//! The parser (GRAMMAR §2–§6): tokens to a syntax tree.
//!
//! Recursive descent, with precedence climbing for binary operators. The
//! disambiguation rules of GRAMMAR §6 are implemented where they apply:
//! speculative parsing for declarations and `while` headers (rules 4, 12),
//! looking past the matching `)` for lambdas and destructuring (rule 1), the
//! type parameters in scope for casts (rule 2), and splitting `>>` when type
//! arguments close (rule 3).

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::source::Span;
use crate::token::{Kw, P, Prim, StrPiece, Tok, Token};

/// Parses a whole file.
pub fn parse_file(tokens: Vec<Token>) -> (File, Vec<Diagnostic>) {
    let mut p = Parser::new(tokens, Vec::new());
    let file = p.file();
    (file, p.diags)
}

/// Parses a single expression (for tests and tools).
pub fn parse_expr(tokens: Vec<Token>) -> (Option<Expr>, Vec<Diagnostic>) {
    let mut p = Parser::new(tokens, Vec::new());
    let e = p.expr().ok();
    if e.is_some() && *p.tok() != Tok::Eof {
        let span = p.span();
        let found = p.tok().describe();
        p.error(
            span,
            format!("expected the end of the expression, found {}", found),
        );
    }
    (e, p.diags)
}

/// Parses a sequence of statements (for tests and tools).
pub fn parse_stmts(tokens: Vec<Token>) -> (Vec<Stmt>, Vec<Diagnostic>) {
    let mut p = Parser::new(tokens, Vec::new());
    let mut stmts = Vec::new();
    while *p.tok() != Tok::Eof {
        let before = p.pos;
        match p.stmt() {
            Ok(s) => stmts.push(s),
            Err(Fail) => p.sync_stmt(before),
        }
    }
    (stmts, p.diags)
}

/// Parsing failed at a point that has already been reported, or, while
/// speculating, that isn't worth reporting.
#[derive(Debug)]
struct Fail;
type PResult<T> = Result<T, Fail>;

static GT: Tok = Tok::P(P::Gt);
static EOF: Tok = Tok::Eof;

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// The current token is `>>` and its first `>` has been consumed by closing type arguments.
    split: bool,
    /// The end of the last token consumed.
    last_end: u32,
    diags: Vec<Diagnostic>,
    /// Type parameters in scope, so `(T) x` is a cast (GRAMMAR §6, rule 2).
    tparams: Vec<String>,
}

#[derive(Clone, Copy)]
struct Mark {
    pos: usize,
    split: bool,
    last_end: u32,
    diags: usize,
}

fn binop(t: &Tok) -> Option<(BinOp, u8)> {
    let Tok::P(p) = t else { return None };
    Some(match p {
        P::OrOr => (BinOp::Or, 4),
        P::AndAnd => (BinOp::And, 5),
        P::Pipe => (BinOp::BitOr, 6),
        P::Caret => (BinOp::BitXor, 7),
        P::Amp => (BinOp::BitAnd, 8),
        P::EqEq => (BinOp::Eq, 9),
        P::NotEq => (BinOp::Ne, 9),
        P::Lt => (BinOp::Lt, 10),
        P::Gt => (BinOp::Gt, 10),
        P::Le => (BinOp::Le, 10),
        P::Ge => (BinOp::Ge, 10),
        P::InsertEnd => (BinOp::InsertEnd, 11),
        P::InsertFront => (BinOp::InsertFront, 11),
        P::InsertCursor => (BinOp::InsertCursor, 11),
        P::Shl => (BinOp::Shl, 12),
        P::Shr => (BinOp::Shr, 12),
        P::Plus => (BinOp::Add, 13),
        P::Minus => (BinOp::Sub, 13),
        P::WrapAdd => (BinOp::WrapAdd, 13),
        P::WrapSub => (BinOp::WrapSub, 13),
        P::SatAdd => (BinOp::SatAdd, 13),
        P::SatSub => (BinOp::SatSub, 13),
        P::Star => (BinOp::Mul, 14),
        P::Slash => (BinOp::Div, 14),
        P::Percent => (BinOp::Rem, 14),
        P::WrapMul => (BinOp::WrapMul, 14),
        P::SatMul => (BinOp::SatMul, 14),
        _ => return None,
    })
}

/// The precedence level of a binary operator (GRAMMAR §6).
pub fn binop_level(op: BinOp) -> u8 {
    use BinOp::*;
    match op {
        Or => 4,
        And => 5,
        BitOr => 6,
        BitXor => 7,
        BitAnd => 8,
        Eq | Ne => 9,
        Lt | Gt | Le | Ge => 10,
        InsertEnd | InsertFront | InsertCursor => 11,
        Shl | Shr => 12,
        Add | Sub | WrapAdd | WrapSub | SatAdd | SatSub => 13,
        Mul | Div | Rem | WrapMul | SatMul => 14,
    }
}

fn assign_op(t: &Tok) -> Option<AssignOp> {
    let Tok::P(p) = t else { return None };
    Some(match p {
        P::Assign => AssignOp::Assign,
        P::DotAssign => AssignOp::Rebind,
        P::PlusAssign => AssignOp::Add,
        P::MinusAssign => AssignOp::Sub,
        P::StarAssign => AssignOp::Mul,
        P::SlashAssign => AssignOp::Div,
        P::PercentAssign => AssignOp::Rem,
        P::AmpAssign => AssignOp::And,
        P::PipeAssign => AssignOp::Or,
        P::CaretAssign => AssignOp::Xor,
        P::ShlAssign => AssignOp::Shl,
        P::ShrAssign => AssignOp::Shr,
        P::WrapAddAssign => AssignOp::WrapAdd,
        P::WrapSubAssign => AssignOp::WrapSub,
        P::WrapMulAssign => AssignOp::WrapMul,
        P::SatAddAssign => AssignOp::SatAdd,
        P::SatSubAssign => AssignOp::SatSub,
        P::SatMulAssign => AssignOp::SatMul,
        _ => return None,
    })
}

/// Whether an expression may be a slice bound without parentheses: one at the
/// shift level or tighter (GRAMMAR §6, rule 13).
fn is_shift_level(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Binary { op, .. } => binop_level(*op) >= 12,
        ExprKind::Cond { .. }
        | ExprKind::Assign { .. }
        | ExprKind::StatusAssign { .. }
        | ExprKind::Lambda { .. }
        | ExprKind::InstanceOf { .. } => false,
        _ => true,
    }
}

/// Can this token start an operand? Used for hints after a postfix `^-`.
fn starts_operand(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_)
            | Tok::Int { .. }
            | Tok::Float { .. }
            | Tok::Char(_)
            | Tok::Str(_)
            | Tok::P(P::LParen)
    )
}

impl Parser {
    fn new(toks: Vec<Token>, tparams: Vec<String>) -> Parser {
        Parser {
            toks,
            pos: 0,
            split: false,
            last_end: 0,
            diags: Vec::new(),
            tparams,
        }
    }

    // ------------------------------------------------------------ token access

    fn tok(&self) -> &Tok {
        let t = &self.toks[self.pos].tok;
        if self.split && *t == Tok::P(P::Shr) {
            &GT
        } else {
            t
        }
    }

    fn tok_at(&self, n: usize) -> &Tok {
        if n == 0 {
            return self.tok();
        }
        self.toks.get(self.pos + n).map_or(&EOF, |t| &t.tok)
    }

    fn span(&self) -> Span {
        let s = self.toks[self.pos].span;
        if self.split {
            Span {
                start: s.start + 1,
                end: s.end,
            }
        } else {
            s
        }
    }

    /// From `start` to the end of the last token consumed.
    fn since(&self, start: Span) -> Span {
        Span {
            start: start.start,
            end: self.last_end.max(start.start),
        }
    }

    fn bump(&mut self) -> Token {
        let mut t = self.toks[self.pos].clone();
        if self.split {
            t.tok = Tok::P(P::Gt);
            t.span.start += 1;
        }
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        self.split = false;
        self.last_end = t.span.end;
        t
    }

    fn at(&self, p: P) -> bool {
        *self.tok() == Tok::P(p)
    }

    fn at_kw(&self, k: Kw) -> bool {
        *self.tok() == Tok::Kw(k)
    }

    fn eat(&mut self, p: P) -> bool {
        if self.at(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Kw) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg));
    }

    fn error_help(&mut self, span: Span, msg: impl Into<String>, help: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(span, msg).with_help(help));
    }

    /// Reports "expected X, found Y".
    fn expected(&mut self, what: &str) -> Fail {
        let span = self.span();
        let found = self.tok().describe();
        if let Tok::Kw(k) = self.tok()
            && let Some(reason) = k.reserved_reason()
        {
            let msg = format!("`{}` is reserved: {}", k.as_str(), reason);
            self.error(span, msg);
            return Fail;
        }
        self.error(span, format!("expected {}, found {}", what, found));
        Fail
    }

    fn expect(&mut self, p: P, what: &str) -> PResult<Span> {
        if self.at(p) {
            Ok(self.bump().span)
        } else {
            Err(self.expected(what))
        }
    }

    fn mark(&self) -> Mark {
        Mark {
            pos: self.pos,
            split: self.split,
            last_end: self.last_end,
            diags: self.diags.len(),
        }
    }

    fn reset(&mut self, m: Mark) {
        self.pos = m.pos;
        self.split = m.split;
        self.last_end = m.last_end;
        self.diags.truncate(m.diags);
    }

    /// Closes type arguments with `>`, taking half of a `>>` if needed (rule 3).
    fn eat_close_angle(&mut self) -> bool {
        match self.tok() {
            Tok::P(P::Gt) => {
                self.bump();
                true
            }
            Tok::P(P::Shr) => {
                self.split = true;
                self.last_end = self.toks[self.pos].span.start + 1;
                true
            }
            _ => false,
        }
    }

    /// The index of the `)` matching the `(` at the current token.
    fn matching_paren(&self) -> Option<usize> {
        let mut depth = 0usize;
        for (i, t) in self.toks.iter().enumerate().skip(self.pos) {
            match t.tok {
                Tok::P(P::LParen) => depth += 1,
                Tok::P(P::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                Tok::P(P::LBrace | P::RBrace | P::Semi) if depth == 0 => return None,
                Tok::Eof => return None,
                _ => {}
            }
        }
        None
    }

    fn ident(&mut self, what: &str) -> PResult<Ident> {
        if let Tok::Ident(name) = self.tok() {
            let name = name.clone();
            let span = self.bump().span;
            Ok(Ident { name, span })
        } else {
            Err(self.expected(what))
        }
    }

    fn name(&mut self, what: &str) -> PResult<Name> {
        if *self.tok() == Tok::Underscore {
            Ok(Name::Discard(self.bump().span))
        } else {
            Ok(Name::Ident(self.ident(what)?))
        }
    }

    fn qual_name(&mut self, what: &str) -> PResult<QualName> {
        let first = self.ident(what)?;
        let start = first.span;
        let mut parts = vec![first];
        while self.at(P::Dot) && matches!(self.tok_at(1), Tok::Ident(_)) {
            self.bump();
            parts.push(self.ident("a name")?);
        }
        Ok(QualName {
            parts,
            span: self.since(start),
        })
    }

    // ------------------------------------------------------------ recovery

    /// Skips past the end of a broken statement: after its `;`, or after a block
    /// it opened. Stops before a `}` that closes an enclosing block.
    fn sync_stmt(&mut self, before: usize) {
        let mut depth = 0usize;
        loop {
            match self.tok() {
                Tok::Eof => return,
                Tok::P(P::LBrace) => depth += 1,
                Tok::P(P::RBrace) => {
                    if depth == 0 {
                        if self.pos == before {
                            self.bump();
                        }
                        return;
                    }
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                }
                Tok::P(P::Semi) if depth == 0 => {
                    self.bump();
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ------------------------------------------------------------ files

    fn file(&mut self) -> File {
        let settings = self.settings();
        let package = if self.at_kw(Kw::Package) {
            self.bump();
            let name = self.qual_name("a package name").ok();
            if self.expect(P::Semi, "`;` after the package name").is_err() {
                self.sync_stmt(self.pos);
            }
            name
        } else {
            None
        };
        let mut imports = Vec::new();
        let mut externs = Vec::new();
        loop {
            let before = self.pos;
            if self.at_kw(Kw::Import) {
                match self.import() {
                    Ok(i) => imports.push(i),
                    Err(Fail) => self.sync_stmt(before),
                }
            } else if self.at_kw(Kw::Extern)
                && (self.tok_at(1) == &Tok::Kw(Kw::Import)
                    || matches!(self.tok_at(1), Tok::Ident(n) if n == "link"))
            {
                match self.extern_item() {
                    Ok(i) => externs.push(i),
                    Err(Fail) => self.sync_stmt(before),
                }
            } else {
                break;
            }
        }
        let mut items = Vec::new();
        while *self.tok() != Tok::Eof {
            let before = self.pos;
            match self.top_item() {
                Ok(i) => items.push(i),
                Err(Fail) => self.sync_stmt(before),
            }
        }
        File {
            settings,
            package,
            imports,
            externs,
            items,
        }
    }

    fn settings(&mut self) -> Vec<SettingItem> {
        let mut out = Vec::new();
        loop {
            match self.tok() {
                Tok::Setting { .. } => out.push(self.setting_item()),
                Tok::HashIf(_) if matches!(self.tok_at(1), Tok::Setting { .. }) => {
                    out.push(self.setting_item())
                }
                _ => return out,
            }
        }
    }

    fn setting_item(&mut self) -> SettingItem {
        if let Tok::Setting { name, value } = self.tok().clone() {
            let span = self.bump().span;
            return SettingItem::Setting { name, value, span };
        }
        match self.cond_block(&mut |p| Ok(p.setting_item())) {
            Ok(b) => SettingItem::If(b),
            Err(Fail) => SettingItem::Setting {
                name: String::new(),
                value: String::new(),
                span: self.span(),
            },
        }
    }

    /// `extern import SDL3/SDL.h;` or `extern link SDL3;`: the path or name is spelled
    /// out from its tokens, up to the `;`.
    fn extern_item(&mut self) -> PResult<ExternItem> {
        let start = self.bump().span; // `extern`
        let kind = if self.eat_kw(Kw::Import) {
            ExternKind::Import
        } else {
            self.bump(); // `link`
            ExternKind::Link
        };
        let mut text = String::new();
        while !matches!(self.tok(), Tok::P(P::Semi) | Tok::Eof) {
            let t = self.bump();
            match &t.tok {
                Tok::Ident(n) => text.push_str(n),
                Tok::Kw(k) => text.push_str(k.as_str()),
                Tok::P(p) => text.push_str(p.as_str()),
                Tok::Int { value, .. } => text.push_str(&value.to_string()),
                _ => {
                    let what = if kind == ExternKind::Import {
                        "a header's path, such as `stdio.h` or `SDL3/SDL.h`"
                    } else {
                        "a library's name, such as `m` or `SDL3`"
                    };
                    self.error(t.span, format!("expected {}", what));
                    return Err(Fail);
                }
            }
        }
        if text.is_empty() {
            let span = self.span();
            self.error(span, "expected a header or library name before `;`");
            return Err(Fail);
        }
        self.expect(P::Semi, "`;` after the name")?;
        Ok(ExternItem {
            kind,
            text,
            span: self.since(start),
        })
    }

    fn import(&mut self) -> PResult<Import> {
        let start = self.bump().span; // `import`
        let path = self.qual_name("the name to import")?;
        let kind = if self.at(P::Dot) && self.tok_at(1) == &Tok::P(P::Star) {
            self.bump();
            self.bump();
            ImportKind::Wildcard
        } else if self.eat_kw(Kw::As) {
            ImportKind::Alias(self.ident("a name after `as`")?)
        } else {
            ImportKind::Single
        };
        self.expect(P::Semi, "`;` after the import")?;
        Ok(Import {
            path,
            kind,
            span: self.since(start),
        })
    }

    /// `#if cond ... [#else ...] #endif` around items of one kind.
    fn cond_block<T>(
        &mut self,
        item: &mut dyn FnMut(&mut Parser) -> PResult<T>,
    ) -> PResult<CondBlock<T>> {
        let start = self.span();
        let Tok::HashIf(cond) = self.bump().tok else {
            unreachable!()
        };
        let then = self.cond_items(item);
        let els = if *self.tok() == Tok::HashElse {
            self.bump();
            Some(self.cond_items(item))
        } else {
            None
        };
        if *self.tok() == Tok::HashEnd {
            self.bump();
        } else {
            self.error(start, "this `#if` has no `#endif`");
            return Err(Fail);
        }
        Ok(CondBlock {
            cond,
            then,
            els,
            span: self.since(start),
        })
    }

    fn cond_items<T>(&mut self, item: &mut dyn FnMut(&mut Parser) -> PResult<T>) -> Vec<T> {
        let mut out = Vec::new();
        loop {
            match self.tok() {
                Tok::HashElse | Tok::HashEnd | Tok::Eof | Tok::P(P::RBrace) => return out,
                _ => {}
            }
            let before = self.pos;
            match item(self) {
                Ok(x) => out.push(x),
                Err(Fail) => self.sync_stmt(before),
            }
        }
    }

    // ------------------------------------------------------------ declarations

    fn modifiers(&mut self) -> Vec<Modifier> {
        let mut mods = Vec::new();
        loop {
            let m = match self.tok() {
                Tok::Kw(Kw::Public) => Modifier::Public,
                Tok::Kw(Kw::Private) => Modifier::Private,
                Tok::Kw(Kw::Static) => Modifier::Static,
                Tok::Kw(Kw::Mut) => Modifier::Mut,
                Tok::Kw(Kw::Extern) => {
                    self.bump();
                    let mut cname = None;
                    if self.at(P::LParen) {
                        self.bump();
                        if let Tok::Str(pieces) = self.tok().clone()
                            && let [crate::token::StrPiece::Text(t)] = &pieces[..]
                        {
                            cname = Some(t.clone());
                            self.bump();
                        } else {
                            let span = self.span();
                            self.error(span, "expected the C function's name as a string: `extern(\"glClearColor\")`");
                        }
                        let _ = self.expect(P::RParen, "`)` after the C name");
                    }
                    mods.push(Modifier::Extern(cname));
                    continue;
                }
                Tok::Kw(k @ (Kw::Protected | Kw::Abstract | Kw::Final)) => {
                    let k = *k;
                    let span = self.bump().span;
                    let msg = format!(
                        "`{}` is reserved: {}",
                        k.as_str(),
                        k.reserved_reason().unwrap()
                    );
                    self.error(span, msg);
                    continue;
                }
                _ => return mods,
            };
            self.bump();
            mods.push(m);
        }
    }

    fn is_root_decl(&self) -> bool {
        matches!(self.tok(), Tok::Ident(n) if n == "root")
            && matches!(self.tok_at(1), Tok::Ident(_))
            && matches!(
                self.tok_at(2),
                Tok::P(P::LBrace | P::Lt) | Tok::Kw(Kw::Implements)
            )
    }

    fn top_item(&mut self) -> PResult<TopItem> {
        if let Tok::HashIf(_) = self.tok() {
            return Ok(TopItem::If(self.cond_block(&mut |p| p.top_item())?));
        }
        if matches!(self.tok(), Tok::HashElse | Tok::HashEnd) {
            let span = self.bump().span;
            self.error(span, "this has no matching `#if`");
            return Err(Fail);
        }
        let start = self.span();
        let mods = self.modifiers();
        if matches!(self.tok(), Tok::Kw(Kw::Class | Kw::Interface | Kw::Enum))
            || self.is_root_decl()
        {
            return Ok(TopItem::Type(self.type_decl(mods, start)?));
        }
        match self.method_or_field(mods, start, true)? {
            FieldOrMethod::Field(f) => Ok(TopItem::Field(f)),
            FieldOrMethod::Method(m) => Ok(TopItem::Method(m)),
        }
    }

    fn type_decl(&mut self, modifiers: Vec<Modifier>, start: Span) -> PResult<TypeDecl> {
        let tparams_len = self.tparams.len();
        let result = self.type_decl_inner(modifiers, start);
        self.tparams.truncate(tparams_len);
        result
    }

    fn type_decl_inner(&mut self, modifiers: Vec<Modifier>, start: Span) -> PResult<TypeDecl> {
        let keyword = self.bump();
        let kind = match keyword.tok {
            Tok::Kw(Kw::Interface) => {
                let name = self.ident("an interface name")?;
                let type_params = self.type_params()?;
                self.expect(P::LBrace, "`{` to start the interface")?;
                let mut members = Vec::new();
                while !self.at(P::RBrace) && *self.tok() != Tok::Eof {
                    let before = self.pos;
                    match self.interface_member() {
                        Ok(m) => members.push(m),
                        Err(Fail) => self.sync_stmt(before),
                    }
                }
                self.expect(P::RBrace, "`}` to end the interface")?;
                TypeDeclKind::Interface(InterfaceDecl {
                    name,
                    type_params,
                    members,
                })
            }
            Tok::Kw(Kw::Enum) => {
                let name = self.ident("an enum name")?;
                self.expect(P::LBrace, "`{` to start the enum")?;
                let mut constants = Vec::new();
                while !self.at(P::RBrace) {
                    constants.push(self.ident("an enum constant")?);
                    if !self.eat(P::Comma) {
                        break;
                    }
                }
                if self.at(P::Semi) || self.at(P::LParen) {
                    let span = self.span();
                    self.error(
                        span,
                        "enums with fields, methods or payloads are undecided (GRAMMAR §8)",
                    );
                    return Err(Fail);
                }
                self.expect(P::RBrace, "`}` to end the enum")?;
                TypeDeclKind::Enum(EnumDecl { name, constants })
            }
            _ => {
                let is_root = keyword.tok != Tok::Kw(Kw::Class);
                let name = self.ident(if is_root {
                    "a root name"
                } else {
                    "a class name"
                })?;
                let type_params = self.type_params()?;
                if self.at_kw(Kw::Extends) {
                    let span = self.span();
                    self.error_help(
                        span,
                        "Zinkite has no inheritance",
                        "implement an interface instead (DESIGN §6.1)",
                    );
                    return Err(Fail);
                }
                let mut implements = Vec::new();
                if self.eat_kw(Kw::Implements) {
                    loop {
                        implements.push(self.class_type()?);
                        if !self.eat(P::Comma) {
                            break;
                        }
                    }
                }
                let members = self.class_body()?;
                let decl = ClassDecl {
                    name,
                    type_params,
                    implements,
                    members,
                };
                if is_root {
                    TypeDeclKind::Root(decl)
                } else {
                    TypeDeclKind::Class(decl)
                }
            }
        };
        Ok(TypeDecl {
            modifiers,
            kind,
            span: self.since(start),
        })
    }

    /// `<T implements A & B, U>`, whose names come into scope (rule 2).
    fn type_params(&mut self) -> PResult<Vec<TypeParam>> {
        let mut out = Vec::new();
        if !self.eat(P::Lt) {
            return Ok(out);
        }
        loop {
            let name = self.ident("a type parameter name")?;
            let mut bounds = Vec::new();
            if self.eat_kw(Kw::Implements) {
                loop {
                    bounds.push(self.class_type()?);
                    if !self.eat(P::Amp) {
                        break;
                    }
                }
            }
            self.tparams.push(name.name.clone());
            out.push(TypeParam { name, bounds });
            if self.eat(P::Comma) {
                continue;
            }
            if self.eat_close_angle() {
                return Ok(out);
            }
            return Err(self.expected("`,` or `>` in the type parameters"));
        }
    }

    fn class_body(&mut self) -> PResult<Vec<Member>> {
        self.expect(P::LBrace, "`{` to start the body")?;
        let mut members = Vec::new();
        loop {
            match self.tok() {
                Tok::P(P::RBrace) | Tok::Eof => break,
                Tok::P(P::Semi) => {
                    self.bump();
                    continue;
                }
                _ => {}
            }
            let before = self.pos;
            match self.member() {
                Ok(m) => members.push(m),
                Err(Fail) => self.sync_stmt(before),
            }
        }
        self.expect(P::RBrace, "`}` to end the body")?;
        Ok(members)
    }

    fn member(&mut self) -> PResult<Member> {
        if let Tok::HashIf(_) = self.tok() {
            return Ok(Member::If(self.cond_block(&mut |p| p.member())?));
        }
        let start = self.span();
        if self.at_kw(Kw::Static) && self.tok_at(1) == &Tok::P(P::LBrace) {
            self.error(
                start,
                "static initializer blocks are undecided (GRAMMAR §8)",
            );
            return Err(Fail);
        }
        let modifiers = self.modifiers();
        if matches!(self.tok(), Tok::Kw(Kw::Class | Kw::Interface | Kw::Enum))
            || self.is_root_decl()
        {
            let span = self.span();
            self.error(span, "nested type declarations are undecided (GRAMMAR §8)");
            return Err(Fail);
        }
        if self.at_kw(Kw::New) && self.tok_at(1) == &Tok::P(P::LParen) {
            self.bump();
            let params = self.params()?;
            let body = self.block()?;
            return Ok(Member::Ctor(CtorDecl {
                modifiers,
                params,
                body,
                span: self.since(start),
            }));
        }
        match self.method_or_field(modifiers, start, true)? {
            FieldOrMethod::Field(f) => Ok(Member::Field(f)),
            FieldOrMethod::Method(m) => Ok(Member::Method(m)),
        }
    }

    fn interface_member(&mut self) -> PResult<InterfaceMember> {
        if let Tok::HashIf(_) = self.tok() {
            return Ok(InterfaceMember::If(
                self.cond_block(&mut |p| p.interface_member())?,
            ));
        }
        let start = self.span();
        let modifiers = self.modifiers();
        match self.method_or_field(modifiers, start, false)? {
            FieldOrMethod::Method(m) => Ok(InterfaceMember::Method(m)),
            FieldOrMethod::Field(f) => {
                self.error(
                    f.span,
                    "interfaces declare methods only, not fields (DESIGN §6.3)",
                );
                Err(Fail)
            }
        }
    }

    /// After modifiers: `[<T>] ret name (params) body` or `type name [= e], ...;`
    /// (rule 9). `with_body` is false in an interface.
    fn method_or_field(
        &mut self,
        modifiers: Vec<Modifier>,
        start: Span,
        with_body: bool,
    ) -> PResult<FieldOrMethod> {
        let tparams_len = self.tparams.len();
        let result = self.method_or_field_inner(modifiers, start, with_body);
        self.tparams.truncate(tparams_len);
        result
    }

    fn method_or_field_inner(
        &mut self,
        modifiers: Vec<Modifier>,
        start: Span,
        with_body: bool,
    ) -> PResult<FieldOrMethod> {
        let type_params = self.type_params()?;
        let ret = self.return_type()?;
        let name = self.ident("a name for the field or method")?;
        if self.at(P::LParen) {
            let params = self.params()?;
            let is_extern = modifiers.iter().any(|m| matches!(m, Modifier::Extern(_)));
            let body = if is_extern {
                // A C function: declared here, defined in C (DESIGN §12.6).
                self.expect(P::Semi, "`;` after the C function's parameters")?;
                None
            } else if with_body {
                Some(self.block()?)
            } else {
                if self.at(P::LBrace) {
                    let span = self.span();
                    self.error(span, "interface methods have no bodies: default methods are undecided (GRAMMAR §8)");
                    return Err(Fail);
                }
                self.expect(P::Semi, "`;` after the interface method")?;
                None
            };
            return Ok(FieldOrMethod::Method(MethodDecl {
                modifiers,
                type_params,
                ret,
                name,
                params,
                body,
                span: self.since(start),
            }));
        }
        if !type_params.is_empty() {
            let span = name.span;
            self.error(span, "a field can't have type parameters");
        }
        let ty = match ret {
            ReturnType::Type(t) => t,
            ReturnType::Void | ReturnType::Tuple(_) => {
                self.error(
                    name.span,
                    "a field needs a type: `void` and tuple types are only for results",
                );
                return Err(Fail);
            }
        };
        let mut vars = Vec::new();
        let mut name = name;
        loop {
            let init = if self.eat(P::Assign) {
                Some(self.expr()?)
            } else {
                None
            };
            vars.push(VarDeclarator { name, init });
            if !self.eat(P::Comma) {
                break;
            }
            name = self.ident("a field name")?;
        }
        self.expect(P::Semi, "`;` after the field")?;
        Ok(FieldOrMethod::Field(FieldDecl {
            modifiers,
            ty,
            vars,
            span: self.since(start),
        }))
    }

    fn return_type(&mut self) -> PResult<ReturnType> {
        if self.eat_kw(Kw::Void) {
            return Ok(ReturnType::Void);
        }
        if self.at(P::LParen) {
            return self.tuple_type();
        }
        Ok(ReturnType::Type(self.ty()?))
    }

    /// `(A, B, ...)`, results only (DESIGN §9.5).
    fn tuple_type(&mut self) -> PResult<ReturnType> {
        self.expect(P::LParen, "`(`")?;
        let mut types = vec![self.ty()?];
        while self.eat(P::Comma) {
            types.push(self.ty()?);
        }
        self.expect(P::RParen, "`)` to end the tuple type")?;
        if types.len() < 2 {
            let span = self.span();
            self.error(span, "a tuple type has at least two types");
        }
        Ok(ReturnType::Tuple(types))
    }

    fn params(&mut self) -> PResult<Vec<Param>> {
        self.expect(P::LParen, "`(` to start the parameters")?;
        let mut params = Vec::new();
        if !self.at(P::RParen) {
            loop {
                let start = self.span();
                let ty = self.ty()?;
                let name = self.name("a parameter name")?;
                params.push(Param {
                    ty,
                    name,
                    span: self.since(start),
                });
                if !self.eat(P::Comma) {
                    break;
                }
            }
        }
        self.expect(P::RParen, "`,` or `)` in the parameters")?;
        Ok(params)
    }

    // ------------------------------------------------------------ types

    /// `qualified_name [type_args]`, for bounds, `implements` and `instanceof`.
    fn class_type(&mut self) -> PResult<Type> {
        let start = self.span();
        let path = self.qual_name("a type name")?;
        let args = self.type_args()?;
        Ok(Type {
            kind: TypeKind::Named { path, args },
            span: self.since(start),
        })
    }

    fn type_args(&mut self) -> PResult<Vec<Type>> {
        let mut args = Vec::new();
        if !self.at(P::Lt) {
            return Ok(args);
        }
        self.bump();
        loop {
            args.push(self.ty()?);
            if self.eat(P::Comma) {
                continue;
            }
            if self.eat_close_angle() {
                return Ok(args);
            }
            return Err(self.expected("`,` or `>` in the type arguments"));
        }
    }

    fn ty(&mut self) -> PResult<Type> {
        let start = self.span();
        let mut t = self.base_type()?;
        loop {
            match self.tok() {
                Tok::P(P::LBracket) => {
                    t = self.array_suffix(t, start)?;
                }
                Tok::P(P::Amp) => {
                    self.bump();
                    let link = self.link()?;
                    t = Type {
                        kind: TypeKind::Ref {
                            inner: Box::new(t),
                            link,
                        },
                        span: self.since(start),
                    };
                }
                Tok::P(P::Question)
                    if matches!(t.kind, TypeKind::Ref { .. } | TypeKind::Func { .. }) =>
                {
                    self.bump();
                    t = Type {
                        kind: TypeKind::Nullable(Box::new(t)),
                        span: self.since(start),
                    };
                }
                _ => return Ok(t),
            }
        }
    }

    fn base_type(&mut self) -> PResult<Type> {
        let start = self.span();
        let kind = match self.tok() {
            Tok::Kw(k) if k.prim().is_some() => {
                let p = k.prim().unwrap();
                self.bump();
                TypeKind::Prim(p)
            }
            Tok::Kw(Kw::String) => {
                self.bump();
                TypeKind::String
            }
            Tok::Kw(Kw::Func) => return self.func_type(),
            Tok::Ident(_) => {
                let path = self.qual_name("a type")?;
                let args = self.type_args()?;
                TypeKind::Named { path, args }
            }
            _ => return Err(self.expected("a type")),
        };
        Ok(Type {
            kind,
            span: self.since(start),
        })
    }

    /// `func(A, B => R)`, with `void` for none on either side.
    fn func_type(&mut self) -> PResult<Type> {
        let start = self.bump().span; // `func`
        self.expect(P::LParen, "`(` after `func`")?;
        let mut params = Vec::new();
        if !self.eat_kw(Kw::Void) {
            loop {
                params.push(self.ty()?);
                if !self.eat(P::Comma) {
                    break;
                }
            }
        }
        if !self.at(P::FatArrow) {
            let span = self.span();
            self.error_help(
                span,
                "a function type writes both sides",
                "write `func(string => void)`, with `void` for no parameters or no result (DESIGN §9.3)",
            );
            return Err(Fail);
        }
        self.bump();
        let result = if self.eat_kw(Kw::Void) {
            ReturnType::Void
        } else if self.at(P::LParen) {
            self.tuple_type()?
        } else {
            ReturnType::Type(self.ty()?)
        };
        self.expect(P::RParen, "`)` to end the function type")?;
        Ok(Type {
            kind: TypeKind::Func {
                params,
                result: Box::new(result),
            },
            span: self.since(start),
        })
    }

    /// After `&`: an optional link width, `<i32>` or `<u32 in arr>`.
    fn link(&mut self) -> PResult<Option<Link>> {
        if !self.at(P::Lt) {
            return Ok(None);
        }
        let width = match self.tok_at(1) {
            Tok::Kw(k) => match k.prim() {
                Some(p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32)) => {
                    p
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        self.bump();
        self.bump();
        let pool = if matches!(width, Prim::U8 | Prim::U16 | Prim::U32) {
            match self.tok() {
                Tok::Ident(n) if n == "in" => {
                    self.bump();
                }
                _ => return Err(self.expected("`in` and the array a pool link is relative to")),
            }
            Some(self.qual_name("the array a pool link is relative to")?)
        } else {
            None
        };
        if !self.eat_close_angle() {
            return Err(self.expected("`>` to end the link"));
        }
        Ok(Some(Link { width, pool }))
    }

    /// `[slots cap]`, `[slots $]` or `[:]` after a type.
    fn array_suffix(&mut self, elem: Type, start: Span) -> PResult<Type> {
        if self.tok_at(1) == &Tok::P(P::Colon) && self.tok_at(2) == &Tok::P(P::RBracket) {
            self.bump();
            self.bump();
            self.bump();
            return Ok(Type {
                kind: TypeKind::Slice(Box::new(elem)),
                span: self.since(start),
            });
        }
        let open = self.bump().span;
        let mut chars = Vec::new();
        loop {
            let c: &[char] = match self.tok() {
                Tok::P(P::Dot) => &['.'],
                Tok::P(P::Gt) => &['>'],
                Tok::P(P::Lt) => &['<'],
                Tok::P(P::Tilde) => &['~'],
                Tok::P(P::Shr) => &['>', '>'],
                Tok::P(P::Shl) => &['<', '<'],
                _ => break,
            };
            chars.extend_from_slice(c);
            self.bump();
        }
        if chars.is_empty() {
            if matches!(self.tok(), Tok::Int { .. }) {
                let span = self.span();
                self.error_help(
                    span,
                    "an array type needs slot characters",
                    "write `T[.k]` for an array of exactly k elements (DESIGN §4.2)",
                );
            } else {
                self.expected("slot characters (`.`, `>`, `<`, `~`) or `:` in an array type");
            }
            return Err(Fail);
        }
        let cap = match self.tok().clone() {
            Tok::Int { value, suffix } => {
                let span = self.bump().span;
                if suffix.is_some() {
                    self.error(span, "an array cap takes no suffix");
                }
                Some(Cap::Int(value as u64))
            }
            Tok::Ident(_) => Some(Cap::Const(self.qual_name("a cap")?)),
            Tok::P(P::Star) => {
                self.bump();
                Some(Cap::Varint)
            }
            Tok::P(P::Dollar) => {
                self.bump();
                Some(Cap::Chunked)
            }
            _ => None,
        };
        if !self.at(P::RBracket) {
            let msg = if cap.is_some() {
                "`]` to end the array type (a cap and `$` can't be combined)"
            } else {
                "`]` to end the array type"
            };
            return Err(self.expected(msg));
        }
        self.bump();
        if chars.len() != 1 && chars.len() != 3 {
            let span = Span {
                start: open.start,
                end: self.last_end,
            };
            self.error_help(
                span,
                format!(
                    "an array type has one or three slot characters, not {}",
                    chars.len()
                ),
                "one character stands for all three: `T[~]` is `T[~~~]` (DESIGN §4.1)",
            );
        }
        Ok(Type {
            kind: TypeKind::Array {
                elem: Box::new(elem),
                slots: Slots { chars, cap },
            },
            span: self.since(start),
        })
    }

    /// `var`, `var&` or a type.
    fn local_type(&mut self) -> PResult<LocalType> {
        if self.eat_kw(Kw::Var) {
            if self.eat(P::Amp) {
                return Ok(LocalType::VarRef);
            }
            return Ok(LocalType::Var);
        }
        Ok(LocalType::Type(self.ty()?))
    }

    /// A local type followed by a name: the start of a declaration (rule 4).
    /// Consumes nothing if there isn't one.
    fn try_decl_head(&mut self) -> Option<(LocalType, Name)> {
        let m = self.mark();
        let ok = (|| -> PResult<(LocalType, Name)> {
            let ty = self.local_type()?;
            if !matches!(self.tok(), Tok::Ident(_) | Tok::Underscore) {
                return Err(Fail);
            }
            let name = self.name("a name")?;
            Ok((ty, name))
        })();
        match ok {
            Ok(x) => Some(x),
            Err(Fail) => {
                self.reset(m);
                None
            }
        }
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self) -> PResult<Block> {
        let start = self.expect(P::LBrace, "`{` to start a block")?;
        let mut stmts = Vec::new();
        loop {
            match self.tok() {
                Tok::P(P::RBrace) | Tok::Eof => break,
                _ => {}
            }
            let before = self.pos;
            match self.stmt() {
                Ok(s) => stmts.push(s),
                Err(Fail) => self.sync_stmt(before),
            }
        }
        self.expect(P::RBrace, "`}` to end the block")?;
        Ok(Block {
            stmts,
            span: self.since(start),
        })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kind = match self.tok() {
            Tok::P(P::LBrace) => StmtKind::Block(self.block()?),
            Tok::P(P::Semi) => {
                self.bump();
                StmtKind::Empty
            }
            Tok::Kw(Kw::If) => self.if_stmt()?,
            Tok::Kw(Kw::While) => self.while_stmt()?,
            Tok::Kw(Kw::Do) => {
                self.bump();
                let body = Box::new(self.stmt()?);
                if !self.eat_kw(Kw::While) {
                    return Err(self.expected("`while` after the body of `do`"));
                }
                self.expect(P::LParen, "`(` after `while`")?;
                let cond = self.expr()?;
                self.expect(P::RParen, "`)` after the condition")?;
                self.expect(P::Semi, "`;` after `do ... while (...)`")?;
                StmtKind::DoWhile { body, cond }
            }
            Tok::Kw(Kw::For) => self.for_stmt()?,
            Tok::Kw(Kw::Switch) => StmtKind::Switch(self.switch()?),
            Tok::Kw(Kw::Break | Kw::Continue) => {
                let is_break = self.at_kw(Kw::Break);
                self.bump();
                let label = if matches!(self.tok(), Tok::Ident(_)) {
                    Some(self.ident("a label")?)
                } else {
                    None
                };
                self.expect(P::Semi, "`;`")?;
                if is_break {
                    StmtKind::Break(label)
                } else {
                    StmtKind::Continue(label)
                }
            }
            Tok::Kw(Kw::Return) => self.return_stmt()?,
            Tok::Kw(Kw::Defer) => {
                self.bump();
                let inner_start = self.span();
                let inner = if self.at(P::LBrace) {
                    StmtKind::Block(self.block()?)
                } else {
                    let e = self.expr()?;
                    self.expect(P::Semi, "`;` after the deferred statement")?;
                    StmtKind::Expr(e)
                };
                StmtKind::Defer(Box::new(Stmt {
                    kind: inner,
                    span: self.since(inner_start),
                }))
            }
            Tok::Kw(Kw::Yield) => {
                self.bump();
                let e = self.expr()?;
                self.expect(P::Semi, "`;` after `yield`")?;
                StmtKind::Yield(e)
            }
            Tok::HashIf(_) => StmtKind::If_(self.cond_block(&mut |p| p.stmt())?),
            Tok::HashElse | Tok::HashEnd => {
                let span = self.bump().span;
                self.error(span, "this has no matching `#if`");
                return Err(Fail);
            }
            Tok::Kw(k) if k.reserved_reason().is_some() => return Err(self.expected("a statement")),
            Tok::Ident(_) if self.tok_at(1) == &Tok::P(P::Colon) => {
                let label = self.ident("a label")?;
                self.bump();
                let stmt = Box::new(self.stmt()?);
                StmtKind::Labeled { label, stmt }
            }
            Tok::P(P::LParen) if self.is_destructure() => self.destructure()?,
            _ => {
                if let Some((ty, name)) = self.try_decl_head() {
                    let decl = self.local_decl_rest(ty, name)?;
                    self.expect(P::Semi, "`;` after the declaration")?;
                    StmtKind::Local(decl)
                } else {
                    let e = self.expr()?;
                    self.expect(P::Semi, "`;` after the statement")?;
                    StmtKind::Expr(e)
                }
            }
        };
        Ok(Stmt {
            kind,
            span: self.since(start),
        })
    }

    /// The declarators after a declaration's type and first name.
    fn local_decl_rest(&mut self, ty: LocalType, first: Name) -> PResult<LocalDecl> {
        let mut decls = Vec::new();
        let mut name = match first {
            Name::Ident(i) => i,
            Name::Discard(span) => {
                self.error(
                    span,
                    "`_` can't be declared; it only discards (DESIGN §7.3)",
                );
                return Err(Fail);
            }
        };
        loop {
            let capture = if self.at(P::At) {
                self.bump();
                Some(self.name("a status variable after `@`")?)
            } else {
                None
            };
            let init = if self.eat(P::Assign) {
                Some(self.expr()?)
            } else {
                None
            };
            decls.push(LocalDeclarator {
                name,
                capture,
                init,
            });
            if !self.eat(P::Comma) {
                break;
            }
            name = self.ident("a variable name")?;
        }
        Ok(LocalDecl { ty, decls })
    }

    fn if_stmt(&mut self) -> PResult<StmtKind> {
        self.bump();
        self.expect(P::LParen, "`(` after `if`")?;
        let cond = self.expr()?;
        self.expect(P::RParen, "`)` after the condition")?;
        let then = Box::new(self.stmt()?);
        let els = if self.eat_kw(Kw::Else) {
            Some(Box::new(self.stmt()?))
        } else {
            None
        };
        Ok(StmtKind::If { cond, then, els })
    }

    /// `while (cond) body`, or the repeat loop `while (T x : call) [@why] body` (rule 12).
    fn while_stmt(&mut self) -> PResult<StmtKind> {
        self.bump();
        self.expect(P::LParen, "`(` after `while`")?;
        if let Some(target) = self.try_loop_target() {
            let source = self.expr()?;
            self.expect(P::RParen, "`)` after the repeat loop's call")?;
            let capture = if self.at(P::At)
                && matches!(self.tok_at(1), Tok::Ident(_) | Tok::Underscore)
                && self.tok_at(2) == &Tok::P(P::LBrace)
            {
                self.bump();
                Some(self.name("a status variable")?)
            } else {
                None
            };
            let body = Box::new(self.stmt()?);
            return Ok(StmtKind::Repeat {
                target,
                source,
                capture,
                body,
            });
        }
        let cond = self.expr()?;
        self.expect(P::RParen, "`)` after the condition")?;
        let body = Box::new(self.stmt()?);
        Ok(StmtKind::While { cond, body })
    }

    /// `T x :` or `(T a, U b) :` at the start of a `while` header; consumes the `:`.
    fn try_loop_target(&mut self) -> Option<LoopTarget> {
        let m = self.mark();
        let result = (|| -> PResult<LoopTarget> {
            let target = if self.at(P::LParen) {
                self.bump();
                let mut items = Vec::new();
                loop {
                    if *self.tok() == Tok::Underscore {
                        items.push(LoopItem::Discard(self.bump().span));
                    } else {
                        let (ty, name) = self.try_decl_head().ok_or(Fail)?;
                        items.push(LoopItem::Decl(ty, name));
                    }
                    if !self.eat(P::Comma) {
                        break;
                    }
                }
                if !self.eat(P::RParen) || items.len() < 2 {
                    return Err(Fail);
                }
                LoopTarget::Tuple(items)
            } else {
                let (ty, name) = self.try_decl_head().ok_or(Fail)?;
                LoopTarget::Single(ty, name)
            };
            if !self.eat(P::Colon) {
                return Err(Fail);
            }
            Ok(target)
        })();
        match result {
            Ok(t) => Some(t),
            Err(Fail) => {
                self.reset(m);
                None
            }
        }
    }

    fn for_stmt(&mut self) -> PResult<StmtKind> {
        self.bump();
        self.expect(P::LParen, "`(` after `for`")?;
        // for-each: `for (T x : e)`
        let m = self.mark();
        if let Some((ty, name)) = self.try_decl_head() {
            if self.eat(P::Colon) {
                let iter = self.expr()?;
                self.expect(P::RParen, "`)` after the for-each's array or iterator")?;
                let body = Box::new(self.stmt()?);
                return Ok(StmtKind::ForEach {
                    ty,
                    name,
                    iter,
                    body,
                });
            }
            self.reset(m);
        }
        // classic: `for (init; cond; update)`
        let init = if self.at(P::Semi) {
            None
        } else if let Some((ty, name)) = self.try_decl_head() {
            Some(ForInit::Decl(self.local_decl_rest(ty, name)?))
        } else {
            Some(ForInit::Exprs(self.expr_list()?))
        };
        self.expect(P::Semi, "`;` after the loop's start")?;
        let cond = if self.at(P::Semi) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect(P::Semi, "`;` after the loop's condition")?;
        let update = if self.at(P::RParen) {
            Vec::new()
        } else {
            self.expr_list()?
        };
        self.expect(P::RParen, "`)` after the loop's step")?;
        let body = Box::new(self.stmt()?);
        Ok(StmtKind::For {
            init,
            cond,
            update,
            body,
        })
    }

    fn expr_list(&mut self) -> PResult<Vec<Expr>> {
        let mut out = vec![self.expr()?];
        while self.eat(P::Comma) {
            out.push(self.expr()?);
        }
        Ok(out)
    }

    fn return_stmt(&mut self) -> PResult<StmtKind> {
        self.bump();
        let status = if self.at(P::Hash) {
            self.bump();
            Some(match self.tok().clone() {
                Tok::Int { value, .. } => {
                    self.bump();
                    StatusWrite::Int(value)
                }
                Tok::Ident(_) => StatusWrite::Name(self.qual_name("a status")?),
                Tok::P(P::LParen) => {
                    self.bump();
                    let e = self.expr()?;
                    self.expect(P::RParen, "`)` after the status")?;
                    StatusWrite::Expr(e)
                }
                _ => {
                    return Err(
                        self.expected("a status after `#`: a number, a named code or `(...)`")
                    );
                }
            })
        } else {
            None
        };
        let value = if self.at(P::Semi) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect(P::Semi, "`;` after `return`")?;
        Ok(StmtKind::Return { status, value })
    }

    /// At `(`: whether `=` or `@` follows the matching `)` (rule 1).
    fn is_destructure(&self) -> bool {
        match self.matching_paren() {
            Some(close) => matches!(
                self.toks.get(close + 1).map(|t| &t.tok),
                Some(Tok::P(P::Assign | P::At))
            ),
            None => false,
        }
    }

    fn destructure(&mut self) -> PResult<StmtKind> {
        self.bump(); // `(`
        let mut targets = Vec::new();
        loop {
            if *self.tok() == Tok::Underscore {
                targets.push(Target::Existing(Name::Discard(self.bump().span)));
            } else if let Some((ty, name)) = self.try_decl_head() {
                targets.push(Target::Decl(ty, name));
            } else {
                targets.push(Target::Existing(Name::Ident(
                    self.ident("a variable to assign")?,
                )));
            }
            if !self.eat(P::Comma) {
                break;
            }
        }
        self.expect(P::RParen, "`)` after the targets")?;
        let capture = if self.eat(P::At) {
            Some(self.name("a status variable after `@`")?)
        } else {
            None
        };
        self.expect(P::Assign, "`=` in the destructuring")?;
        let value = self.expr()?;
        self.expect(P::Semi, "`;` after the destructuring")?;
        Ok(StmtKind::Destructure {
            targets,
            capture,
            value,
        })
    }

    fn switch(&mut self) -> PResult<Switch> {
        let start = self.bump().span; // `switch`
        self.expect(P::LParen, "`(` after `switch`")?;
        let scrutinee = Box::new(self.expr()?);
        self.expect(P::RParen, "`)` after the switch value")?;
        self.expect(P::LBrace, "`{` to start the cases")?;
        let mut rules = Vec::new();
        while !self.at(P::RBrace) && *self.tok() != Tok::Eof {
            let rule_start = self.span();
            let labels = if self.eat_kw(Kw::Default) {
                None
            } else if self.eat_kw(Kw::Case) {
                let mut labels = vec![self.case_label()?];
                while self.eat(P::Comma) {
                    labels.push(self.case_label()?);
                }
                Some(labels)
            } else {
                return Err(self.expected("`case` or `default`"));
            };
            if !self.at(P::FatArrow) {
                let span = self.span();
                self.error_help(
                    span,
                    "expected `=>` after the case",
                    "switch cases use `=>` and never fall through (DESIGN §9.4)",
                );
                return Err(Fail);
            }
            self.bump();
            let body = if self.at(P::LBrace) {
                SwitchBody::Block(self.block()?)
            } else {
                let e = self.expr()?;
                self.expect(P::Semi, "`;` after the case")?;
                SwitchBody::Expr(e)
            };
            rules.push(SwitchRule {
                labels,
                body,
                span: self.since(rule_start),
            });
        }
        self.expect(P::RBrace, "`}` to end the switch")?;
        Ok(Switch {
            scrutinee,
            rules,
            span: self.since(start),
        })
    }

    fn case_label(&mut self) -> PResult<CaseLabel> {
        let start = self.span();
        let negative = self.eat(P::Minus);
        match self.tok().clone() {
            Tok::Int { value, .. } => {
                self.bump();
                Ok(CaseLabel::Int {
                    negative,
                    value,
                    span: self.since(start),
                })
            }
            _ if negative => Err(self.expected("a number after `-`")),
            Tok::Char(c) => Ok(CaseLabel::Char(c, self.bump().span)),
            Tok::Str(pieces) => {
                let span = self.bump().span;
                let mut text = String::new();
                for piece in pieces {
                    match piece {
                        StrPiece::Text(t) => text.push_str(&t),
                        StrPiece::Interp(_, s) => {
                            self.error(s, "a case label is a constant: no interpolation");
                            return Err(Fail);
                        }
                    }
                }
                Ok(CaseLabel::Str(text, span))
            }
            Tok::Ident(_) => Ok(CaseLabel::Ident(self.ident("a case label")?)),
            _ => Err(self.expected("a case label: a number, character, string or enum constant")),
        }
    }

    // ------------------------------------------------------------ expressions

    fn expr(&mut self) -> PResult<Expr> {
        self.assignment()
    }

    fn at_status_dest(&self) -> bool {
        self.at(P::At)
            && matches!(self.tok_at(1), Tok::Ident(_) | Tok::Underscore)
            && self.tok_at(2) == &Tok::P(P::Assign)
    }

    fn assignment(&mut self) -> PResult<Expr> {
        let start = self.span();
        if self.at_status_dest() {
            // `@err = e` (rule 7)
            self.bump();
            let capture = self.name("a status variable")?;
            self.bump(); // `=`
            let value = Box::new(self.assignment()?);
            return Ok(Expr {
                kind: ExprKind::StatusAssign { capture, value },
                span: self.since(start),
            });
        }
        let lhs = self.conditional()?;
        let capture = if self.at_status_dest() {
            self.bump();
            Some(self.name("a status variable")?)
        } else {
            None
        };
        let Some(op) = assign_op(self.tok()) else {
            return Ok(lhs);
        };
        self.bump();
        let value = Box::new(self.assignment()?);
        Ok(Expr {
            kind: ExprKind::Assign {
                op,
                target: Box::new(lhs),
                capture,
                value,
            },
            span: self.since(start),
        })
    }

    fn conditional(&mut self) -> PResult<Expr> {
        let start = self.span();
        let cond = self.binary(4)?;
        if !self.eat(P::Question) {
            return Ok(cond);
        }
        if self.at(P::Dot) {
            let span = self.span();
            self.error(span, "null-safe access `?.` is undecided (GRAMMAR §8)");
            return Err(Fail);
        }
        let then = self.expr()?;
        self.expect(P::Colon, "`:` in the conditional")?;
        let els = self.conditional()?;
        Ok(Expr {
            kind: ExprKind::Cond {
                cond: Box::new(cond),
                then: Box::new(then),
                els: Box::new(els),
            },
            span: self.since(start),
        })
    }

    /// Binary operators from level `min` up (precedence climbing).
    fn binary(&mut self, min: u8) -> PResult<Expr> {
        let start = self.span();
        let mut lhs = self.unary()?;
        loop {
            if self.at_kw(Kw::Instanceof) {
                if min > 10 {
                    break;
                }
                self.bump();
                let ty = self.class_type()?;
                let bind = if matches!(self.tok(), Tok::Ident(_)) {
                    Some(self.ident("a name")?)
                } else {
                    None
                };
                lhs = Expr {
                    kind: ExprKind::InstanceOf {
                        expr: Box::new(lhs),
                        ty,
                        bind,
                    },
                    span: self.since(start),
                };
                continue;
            }
            let Some((op, level)) = binop(self.tok()) else {
                break;
            };
            if level < min {
                break;
            }
            self.bump();
            let rhs = self.binary(level + 1)?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: self.since(start),
            };
        }
        Ok(lhs)
    }

    /// At `(`: whether this is a cast (rule 2).
    fn is_cast(&self) -> bool {
        let target = match self.tok_at(1) {
            Tok::Kw(k) => k.prim().is_some(),
            Tok::Ident(n) => self.tparams.iter().any(|p| p == n),
            _ => false,
        };
        target
            && match self.tok_at(2) {
                Tok::P(P::RParen) => true,
                Tok::P(P::Percent | P::Pipe) => self.tok_at(3) == &Tok::P(P::RParen),
                _ => false,
            }
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let op = match self.tok() {
            Tok::P(P::Plus) => Some(UnaryOp::Plus),
            Tok::P(P::Minus) => Some(UnaryOp::Neg),
            Tok::P(P::Bang) => Some(UnaryOp::Not),
            Tok::P(P::Tilde) => Some(UnaryOp::BitNot),
            Tok::P(P::PlusPlus) => Some(UnaryOp::PreInc),
            Tok::P(P::MinusMinus) => Some(UnaryOp::PreDec),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let expr = Box::new(self.unary()?);
            return Ok(Expr {
                kind: ExprKind::Unary { op, expr },
                span: self.since(start),
            });
        }
        if self.at(P::At) {
            self.bump();
            let expr = Box::new(self.postfix()?);
            return Ok(Expr {
                kind: ExprKind::StatusRead(expr),
                span: self.since(start),
            });
        }
        if self.at(P::LParen) && self.is_cast() {
            self.bump();
            let ty = match self.bump().tok {
                Tok::Kw(k) => CastTarget::Prim(k.prim().unwrap()),
                Tok::Ident(n) => CastTarget::Param(Ident {
                    name: n,
                    span: Span {
                        start: start.start + 1,
                        end: self.last_end,
                    },
                }),
                _ => unreachable!(),
            };
            let mode = if self.eat(P::Percent) {
                CastMode::Wrap
            } else if self.eat(P::Pipe) {
                CastMode::Saturate
            } else {
                CastMode::Checked
            };
            self.bump(); // `)`
            let expr = Box::new(self.unary()?);
            return Ok(Expr {
                kind: ExprKind::Cast { ty, mode, expr },
                span: self.since(start),
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut e = self.primary()?;
        loop {
            match self.tok() {
                Tok::P(P::Dot) => {
                    self.bump();
                    if self.at_kw(Kw::New) {
                        let span = self.span();
                        self.error(
                            span,
                            "only `this.new(...)` calls a constructor this way (DESIGN §6.2)",
                        );
                        return Err(Fail);
                    }
                    let name = self.ident("a member name after `.`")?;
                    e = Expr {
                        kind: ExprKind::Member {
                            expr: Box::new(e),
                            name,
                        },
                        span: self.since(start),
                    };
                }
                Tok::P(P::LParen) => {
                    let args = self.args()?;
                    e = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(e),
                            args,
                        },
                        span: self.since(start),
                    };
                }
                Tok::P(P::LBracket) => e = self.index_or_slice(e, start)?,
                _ => break,
            }
        }
        loop {
            let op = match self.tok() {
                Tok::P(P::PlusPlus) => PostfixOp::Inc,
                Tok::P(P::MinusMinus) => PostfixOp::Dec,
                Tok::P(P::RemoveEnd) => PostfixOp::RemoveEnd,
                Tok::P(P::RemoveFront) => PostfixOp::RemoveFront,
                Tok::P(P::RemoveBefore) => PostfixOp::RemoveBefore,
                Tok::P(P::RemoveAfter) => PostfixOp::RemoveAfter,
                _ => break,
            };
            let op_span = self.bump().span;
            if op == PostfixOp::RemoveAfter && starts_operand(self.tok()) {
                self.error_help(
                    op_span,
                    "`^-` removes the element after the cursor",
                    "for XOR with a negated value, put a space between them: `a ^ -b` (GRAMMAR §1.5)",
                );
                return Err(Fail);
            }
            e = Expr {
                kind: ExprKind::Postfix {
                    op,
                    expr: Box::new(e),
                },
                span: self.since(start),
            };
        }
        Ok(e)
    }

    fn args(&mut self) -> PResult<Vec<Arg>> {
        self.expect(P::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(P::RParen) {
            loop {
                let marked = self.eat(P::Amp);
                args.push(Arg {
                    marked,
                    expr: self.expr()?,
                });
                if !self.eat(P::Comma) {
                    break;
                }
            }
        }
        self.expect(P::RParen, "`,` or `)` in the arguments")?;
        Ok(args)
    }

    /// After an expression, `[` starts an index or a slice (rules 5, 6, 13).
    fn index_or_slice(&mut self, e: Expr, start: Span) -> PResult<Expr> {
        self.bump(); // `[`
        let mode = match self.tok() {
            Tok::P(P::Percent) => Some(IndexMode::Wrap),
            Tok::P(P::Pipe) => Some(IndexMode::Clamp),
            _ => None,
        };
        if let Some(mode) = mode {
            self.bump();
            let index = Box::new(self.expr()?);
            self.close_index()?;
            return Ok(Expr {
                kind: ExprKind::Index {
                    expr: Box::new(e),
                    mode,
                    index,
                },
                span: self.since(start),
            });
        }
        let lo = if self.at(P::Colon) {
            None
        } else if self.eat(P::Caret) {
            let b = self.binary(12)?;
            if !self.at(P::Colon) {
                self.close_index()?;
                return Ok(Expr {
                    kind: ExprKind::Index {
                        expr: Box::new(e),
                        mode: IndexMode::FromEnd,
                        index: Box::new(b),
                    },
                    span: self.since(start),
                });
            }
            Some(Bound {
                from_end: true,
                expr: Box::new(b),
            })
        } else {
            let index = self.expr()?;
            if !self.at(P::Colon) {
                self.close_index()?;
                return Ok(Expr {
                    kind: ExprKind::Index {
                        expr: Box::new(e),
                        mode: IndexMode::Checked,
                        index: Box::new(index),
                    },
                    span: self.since(start),
                });
            }
            if !is_shift_level(&index) {
                self.error_help(
                    index.span,
                    "a slice bound that uses a comparison, a bitwise operator or a conditional needs parentheses",
                    "write `a[(c ? i : j):]` or `a[0:(n & 7)]` (GRAMMAR §6, rule 13)",
                );
            }
            Some(Bound {
                from_end: false,
                expr: Box::new(index),
            })
        };
        self.bump(); // `:`
        let hi = if self.at(P::RBracket) {
            None
        } else {
            let from_end = self.eat(P::Caret);
            if self.at(P::Pipe) {
                let span = self.span();
                self.error(span, "a clamped slice bound `[:|n]` is undecided");
                return Err(Fail);
            }
            Some(Bound {
                from_end,
                expr: Box::new(self.binary(12)?),
            })
        };
        if !self.at(P::RBracket) {
            let span = self.span();
            self.error_help(
                span,
                "expected `]` to end the slice",
                "a slice bound that uses a comparison, a bitwise operator or a conditional needs parentheses (GRAMMAR §6, rule 13)",
            );
            return Err(Fail);
        }
        self.bump();
        Ok(Expr {
            kind: ExprKind::Slice {
                expr: Box::new(e),
                lo,
                hi,
            },
            span: self.since(start),
        })
    }

    fn close_index(&mut self) -> PResult<()> {
        if self.at(P::Comma) {
            let span = self.span();
            self.error(
                span,
                "multi-dimensional indexing `m[i, j]` is undecided (GRAMMAR §8)",
            );
            return Err(Fail);
        }
        self.expect(P::RBracket, "`]` to end the index")?;
        Ok(())
    }

    fn primary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let kind = match self.tok().clone() {
            Tok::Int { value, suffix } => {
                self.bump();
                ExprKind::Int { value, suffix }
            }
            Tok::Float { text, suffix } => {
                self.bump();
                ExprKind::Float { text, suffix }
            }
            Tok::Char(c) => {
                self.bump();
                ExprKind::Char(c)
            }
            Tok::Str(pieces) => {
                self.bump();
                let mut parts = Vec::new();
                for piece in pieces {
                    match piece {
                        StrPiece::Text(t) => parts.push(StrPart::Text(t)),
                        StrPiece::Interp(toks, span) => {
                            parts.push(StrPart::Interp(self.interpolation(toks, span)?))
                        }
                    }
                }
                ExprKind::Str(parts)
            }
            Tok::Kw(Kw::True) => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::Kw(Kw::False) => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Kw(Kw::Null) => {
                self.bump();
                ExprKind::Null
            }
            Tok::Ident(name) => {
                self.bump();
                ExprKind::Ident(name)
            }
            Tok::Underscore => {
                self.bump();
                ExprKind::Discard
            }
            // `extern.errno()`: the C side's namespace.
            Tok::Kw(Kw::Extern) => {
                self.bump();
                ExprKind::Ident("extern".into())
            }
            Tok::Kw(Kw::This) => {
                self.bump();
                if self.at(P::Dot)
                    && self.tok_at(1) == &Tok::Kw(Kw::New)
                    && self.tok_at(2) == &Tok::P(P::LParen)
                {
                    self.bump();
                    self.bump();
                    ExprKind::ThisNew(self.args()?)
                } else {
                    ExprKind::This
                }
            }
            Tok::Kw(k) if k.prim().is_some() => {
                let prim = k.prim().unwrap();
                self.bump();
                if !self.at(P::Dot) {
                    let span = self.since(start);
                    self.error_help(
                        span,
                        format!("`{}` is a type, not a value", prim.name()),
                        format!("for a conversion write a cast: `({}) x`", prim.name()),
                    );
                    return Err(Fail);
                }
                self.bump();
                let name = self.ident("a member name")?;
                ExprKind::PrimMember { prim, name }
            }
            Tok::Kw(Kw::New) => {
                self.bump();
                let class = if self.at(P::LParen) {
                    None
                } else {
                    Some(self.class_type()?)
                };
                ExprKind::New {
                    class,
                    args: self.args()?,
                }
            }
            Tok::P(P::LBracket) => {
                self.bump();
                let mut elems = Vec::new();
                while !self.at(P::RBracket) {
                    elems.push(self.expr()?);
                    if self.at(P::Colon) {
                        let span = self.span();
                        self.error(span, "hash map literals with entries are undecided (GRAMMAR §8); start with `[]` and `put`");
                        return Err(Fail);
                    }
                    if !self.eat(P::Comma) {
                        break;
                    }
                }
                self.expect(P::RBracket, "`,` or `]` in the array literal")?;
                ExprKind::ArrayLit(elems)
            }
            Tok::Kw(Kw::Switch) => ExprKind::Switch(self.switch()?),
            Tok::P(P::LParen) => {
                if let Some(close) = self.matching_paren()
                    && self.toks.get(close + 1).map(|t| &t.tok) == Some(&Tok::P(P::FatArrow))
                {
                    return self.lambda();
                }
                self.bump();
                let first = self.expr()?;
                if self.eat(P::Comma) {
                    let mut elems = vec![first];
                    loop {
                        elems.push(self.expr()?);
                        if !self.eat(P::Comma) {
                            break;
                        }
                    }
                    self.expect(P::RParen, "`)` to end the tuple")?;
                    ExprKind::Tuple(elems)
                } else {
                    self.expect(P::RParen, "`)`")?;
                    ExprKind::Paren(Box::new(first))
                }
            }
            _ => return Err(self.expected("an expression")),
        };
        Ok(Expr {
            kind,
            span: self.since(start),
        })
    }

    /// Parses the tokens of a `${...}` as one expression.
    fn interpolation(&mut self, toks: Vec<Token>, span: Span) -> PResult<Expr> {
        let mut sub = Parser::new(toks, self.tparams.clone());
        sub.last_end = span.start;
        let e = sub.expr();
        if e.is_ok() && *sub.tok() != Tok::Eof {
            let s = sub.span();
            let found = sub.tok().describe();
            sub.error(
                s,
                format!("expected `}}` to end the interpolation, found {}", found),
            );
        }
        self.diags.append(&mut sub.diags);
        e
    }

    /// `(T a, U b) => body` (DESIGN §9.3).
    fn lambda(&mut self) -> PResult<Expr> {
        let start = self.bump().span; // `(`
        let mut params = Vec::new();
        if !self.at(P::RParen) {
            loop {
                let pstart = self.span();
                if self.at_kw(Kw::Var) {
                    let span = self.span();
                    self.error(span, "a lambda parameter's type is written out; `var` isn't allowed (DESIGN §9.3)");
                    return Err(Fail);
                }
                let ty = self.ty()?;
                if !matches!(self.tok(), Tok::Ident(_) | Tok::Underscore) {
                    let span = ty.span;
                    self.error_help(
                        span,
                        "a lambda parameter needs a type and a name",
                        "write `(i32 a, i32 b) => a + b` (DESIGN §9.3)",
                    );
                    return Err(Fail);
                }
                let name = self.name("a parameter name")?;
                params.push(Param {
                    ty,
                    name,
                    span: self.since(pstart),
                });
                if !self.eat(P::Comma) {
                    break;
                }
            }
        }
        self.expect(P::RParen, "`)` after the lambda's parameters")?;
        self.expect(P::FatArrow, "`=>`")?;
        let body = if self.at(P::LBrace) {
            LambdaBody::Block(self.block()?)
        } else {
            LambdaBody::Expr(self.expr()?)
        };
        Ok(Expr {
            kind: ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
            span: self.since(start),
        })
    }
}

enum FieldOrMethod {
    Field(FieldDecl),
    Method(MethodDecl),
}

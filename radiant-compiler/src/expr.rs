use crate::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Null,
    Bool(bool),
    String(String),
    Integer(i64),
    Float(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Negate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Elvis,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal {
        value: Literal,
        span: Span,
    },
    Identifier {
        name: String,
        span: Span,
    },
    Namespace {
        namespace: String,
        name: String,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expression: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Member {
        object: Box<Expr>,
        member: String,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        arguments: Vec<Expr>,
        span: Span,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Safe {
        expression: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Literal { span, .. }
            | Self::Identifier { span, .. }
            | Self::Namespace { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Member { span, .. }
            | Self::Call { span, .. }
            | Self::Index { span, .. }
            | Self::Safe { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Null,
    Bool(bool),
    Str(String),
    Int(i64),
    Float(f64),
    Op(&'static str),
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    Comma,
    Colon,
    End,
}

#[derive(Debug, Clone)]
struct Token {
    kind: Tok,
    span: Span,
}

pub(crate) fn parse_expression(
    source_name: &str,
    whole: &str,
    text: &str,
    base: usize,
) -> Result<Expr, Diagnostic> {
    let tokens = lex(source_name, whole, text, base)?;
    let mut parser = ExprParser {
        source_name,
        whole,
        tokens,
        at: 0,
    };
    let expr = parser.parse_bp(0)?;
    if parser.current().kind != Tok::End {
        return Err(parser.error(
            "E_EXPR_TRAILING",
            "unexpected token after expression",
            parser.current().span,
        ));
    }
    Ok(expr)
}

fn lex(source_name: &str, whole: &str, input: &str, base: usize) -> Result<Vec<Token>, Diagnostic> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let ch = input[i..].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        let start = i;
        if ch.is_ascii_alphabetic() || ch == '_' {
            i += ch.len_utf8();
            while i < input.len() {
                let c = input[i..].chars().next().unwrap_or_default();
                if c.is_alphanumeric() || c == '_' {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            let word = &input[start..i];
            let kind = match word {
                "null" => Tok::Null,
                "true" => Tok::Bool(true),
                "false" => Tok::Bool(false),
                _ => Tok::Ident(word.into()),
            };
            out.push(Token {
                kind,
                span: Span::new(base + start, base + i),
            });
            continue;
        }
        if ch.is_ascii_digit() {
            i += 1;
            while i < input.len() && input.as_bytes()[i].is_ascii_digit() {
                i += 1
            }
            let mut float = false;
            if i < input.len()
                && input.as_bytes()[i] == b'.'
                && i + 1 < input.len()
                && input.as_bytes()[i + 1].is_ascii_digit()
            {
                float = true;
                i += 1;
                while i < input.len() && input.as_bytes()[i].is_ascii_digit() {
                    i += 1
                }
            }
            let raw = &input[start..i];
            let kind = if float {
                Tok::Float(raw.parse().unwrap_or_default())
            } else {
                Tok::Int(raw.parse().unwrap_or_default())
            };
            out.push(Token {
                kind,
                span: Span::new(base + start, base + i),
            });
            continue;
        }
        if ch == '\'' || ch == '"' {
            let quote = ch;
            i += 1;
            let mut value = String::new();
            let mut closed = false;
            while i < input.len() {
                let c = input[i..].chars().next().unwrap_or_default();
                i += c.len_utf8();
                if c == quote {
                    closed = true;
                    break;
                }
                if c == '\\' && i < input.len() {
                    let e = input[i..].chars().next().unwrap_or_default();
                    i += e.len_utf8();
                    value.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    value.push(c)
                }
            }
            if !closed {
                return Err(make_diag(
                    source_name,
                    whole,
                    "E_EXPR_STRING",
                    "unterminated string literal",
                    Span::new(base + start, base + i),
                ));
            }
            out.push(Token {
                kind: Tok::Str(value),
                span: Span::new(base + start, base + i),
            });
            continue;
        }
        let rest = &input[i..];
        let (kind, n) = if rest.starts_with("??") {
            (Tok::Op("??"), 2)
        } else if rest.starts_with("?:") {
            (Tok::Op("?:"), 2)
        } else if rest.starts_with("&&") {
            (Tok::Op("&&"), 2)
        } else if rest.starts_with("||") {
            (Tok::Op("||"), 2)
        } else if rest.starts_with("==") {
            (Tok::Op("=="), 2)
        } else if rest.starts_with("!=") {
            (Tok::Op("!="), 2)
        } else if rest.starts_with("<=") {
            (Tok::Op("<="), 2)
        } else if rest.starts_with(">=") {
            (Tok::Op(">="), 2)
        } else {
            (
                match ch {
                    '!' => Tok::Op("!"),
                    '-' => Tok::Op("-"),
                    '+' => Tok::Op("+"),
                    '*' => Tok::Op("*"),
                    '/' => Tok::Op("/"),
                    '%' => Tok::Op("%"),
                    '<' => Tok::Op("<"),
                    '>' => Tok::Op(">"),
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    '[' => Tok::LBracket,
                    ']' => Tok::RBracket,
                    '.' => Tok::Dot,
                    ',' => Tok::Comma,
                    ':' => Tok::Colon,
                    _ => {
                        return Err(make_diag(
                            source_name,
                            whole,
                            "E_EXPR_TOKEN",
                            format!("unexpected character `{ch}`"),
                            Span::new(base + i, base + i + ch.len_utf8()),
                        ));
                    }
                },
                ch.len_utf8(),
            )
        };
        i += n;
        out.push(Token {
            kind,
            span: Span::new(base + start, base + i),
        });
    }
    out.push(Token {
        kind: Tok::End,
        span: Span::new(base + input.len(), base + input.len()),
    });
    Ok(out)
}

struct ExprParser<'a> {
    source_name: &'a str,
    whole: &'a str,
    tokens: Vec<Token>,
    at: usize,
}
impl ExprParser<'_> {
    fn current(&self) -> &Token {
        &self.tokens[self.at]
    }
    fn bump(&mut self) -> Token {
        let t = self.tokens[self.at].clone();
        self.at += 1;
        t
    }
    fn error(&self, code: &'static str, msg: impl Into<String>, span: Span) -> Diagnostic {
        make_diag(self.source_name, self.whole, code, msg, span)
    }
    fn parse_bp(&mut self, min: u8) -> Result<Expr, Diagnostic> {
        let token = self.bump();
        let mut lhs = match token.kind {
            Tok::Null => Expr::Literal {
                value: Literal::Null,
                span: token.span,
            },
            Tok::Bool(v) => Expr::Literal {
                value: Literal::Bool(v),
                span: token.span,
            },
            Tok::Str(v) => Expr::Literal {
                value: Literal::String(v),
                span: token.span,
            },
            Tok::Int(v) => Expr::Literal {
                value: Literal::Integer(v),
                span: token.span,
            },
            Tok::Float(v) => Expr::Literal {
                value: Literal::Float(v),
                span: token.span,
            },
            Tok::Ident(name) => {
                if self.current().kind == Tok::Colon {
                    self.bump();
                    let rhs = self.bump();
                    if let Tok::Ident(second) = rhs.kind {
                        Expr::Namespace {
                            namespace: name,
                            name: second,
                            span: Span::new(token.span.start, rhs.span.end),
                        }
                    } else {
                        return Err(self.error(
                            "E_EXPR_NAMESPACE",
                            "expected name after namespace colon",
                            rhs.span,
                        ));
                    }
                } else {
                    Expr::Identifier {
                        name,
                        span: token.span,
                    }
                }
            }
            Tok::Op("!") | Tok::Op("-") => {
                let op = if token.kind == Tok::Op("!") {
                    UnaryOp::Not
                } else {
                    UnaryOp::Negate
                };
                let rhs = self.parse_bp(13)?;
                let end = rhs.span().end;
                Expr::Unary {
                    op,
                    expression: Box::new(rhs),
                    span: Span::new(token.span.start, end),
                }
            }
            Tok::LParen => {
                let value = self.parse_bp(0)?;
                if self.current().kind != Tok::RParen {
                    return Err(self.error("E_EXPR_PAREN", "expected `)`", self.current().span));
                }
                self.bump();
                value
            }
            _ => return Err(self.error("E_EXPR_EXPECTED", "expected expression", token.span)),
        };
        loop {
            match self.current().kind.clone() {
                Tok::Dot if 15 >= min => {
                    self.bump();
                    let member = self.bump();
                    let Tok::Ident(name) = member.kind else {
                        return Err(self.error(
                            "E_EXPR_MEMBER",
                            "expected member name",
                            member.span,
                        ));
                    };
                    let start = lhs.span().start;
                    lhs = Expr::Member {
                        object: Box::new(lhs),
                        member: name,
                        span: Span::new(start, member.span.end),
                    }
                }
                Tok::LParen if 15 >= min => {
                    self.bump();
                    let mut args = Vec::new();
                    if self.current().kind != Tok::RParen {
                        loop {
                            args.push(self.parse_bp(0)?);
                            if self.current().kind == Tok::Comma {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                    }
                    if self.current().kind != Tok::RParen {
                        return Err(self.error("E_EXPR_CALL", "expected `)`", self.current().span));
                    }
                    let end = self.bump().span.end;
                    let start = lhs.span().start;
                    lhs = Expr::Call {
                        callee: Box::new(lhs),
                        arguments: args,
                        span: Span::new(start, end),
                    }
                }
                Tok::LBracket if 15 >= min => {
                    self.bump();
                    let index = self.parse_bp(0)?;
                    if self.current().kind != Tok::RBracket {
                        return Err(self.error(
                            "E_EXPR_INDEX",
                            "expected `]`",
                            self.current().span,
                        ));
                    }
                    let end = self.bump().span.end;
                    let start = lhs.span().start;
                    lhs = Expr::Index {
                        object: Box::new(lhs),
                        index: Box::new(index),
                        span: Span::new(start, end),
                    }
                }
                Tok::Op("??") if 14 >= min => {
                    let end = self.bump().span.end;
                    let start = lhs.span().start;
                    lhs = Expr::Safe {
                        expression: Box::new(lhs),
                        span: Span::new(start, end),
                    }
                }
                Tok::Op(op) => {
                    let Some((l, r, bop)) = infix(op) else { break };
                    if l < min {
                        break;
                    }
                    self.bump();
                    let rhs = self.parse_bp(r)?;
                    let span = Span::new(lhs.span().start, rhs.span().end);
                    lhs = Expr::Binary {
                        op: bop,
                        left: Box::new(lhs),
                        right: Box::new(rhs),
                        span,
                    }
                }
                _ => break,
            }
        }
        Ok(lhs)
    }
}
fn infix(op: &str) -> Option<(u8, u8, BinaryOp)> {
    Some(match op {
        "?:" => (1, 1, BinaryOp::Elvis),
        "||" => (2, 3, BinaryOp::Or),
        "&&" => (4, 5, BinaryOp::And),
        "==" => (6, 7, BinaryOp::Equal),
        "!=" => (6, 7, BinaryOp::NotEqual),
        "<" => (8, 9, BinaryOp::Less),
        "<=" => (8, 9, BinaryOp::LessEqual),
        ">" => (8, 9, BinaryOp::Greater),
        ">=" => (8, 9, BinaryOp::GreaterEqual),
        "+" => (10, 11, BinaryOp::Add),
        "-" => (10, 11, BinaryOp::Subtract),
        "*" => (12, 13, BinaryOp::Multiply),
        "/" => (12, 13, BinaryOp::Divide),
        "%" => (12, 13, BinaryOp::Remainder),
        _ => return None,
    })
}

pub(crate) fn make_diag(
    source_name: &str,
    source: &str,
    code: &'static str,
    message: impl Into<String>,
    span: Span,
) -> Diagnostic {
    let offset = span.start.min(source.len());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |n| n + 1);
    let column = source[line_start..offset].chars().count() + 1;
    Diagnostic {
        code,
        message: message.into(),
        source_name: source_name.into(),
        span,
        line,
        column,
    }
}

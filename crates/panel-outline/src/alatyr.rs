//! Lightweight structural extraction for the Alatyr language.
//!
//! Alatyr deliberately has no declaration keywords: declarations bind a name
//! to a value (`name := value`), and `fn`/`struct`/`enum`/`union`/`mod` are
//! value introducers on the right-hand side.  There is no tree-sitter grammar
//! in the workspace, so the outline uses a small comment- and string-aware
//! lexer plus scope tracking instead of trying to parse expressions.

use crate::symbols::{SymbolInfo, SymbolKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Quoted,
    At,
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    ColonEq,
    Colon,
    Eq,
    Comma,
    Semi,
    Other,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
    line: usize,
    column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Function,
    Aggregate,
    Other,
}

struct Declaration {
    names: Vec<usize>,
    display_name: Option<String>,
    kind: SymbolKind,
}

/// Extract declarations from an Alatyr source file.
pub(crate) fn extract_symbols(source: &str) -> Vec<SymbolInfo> {
    let tokens = lex(source);
    let mut symbols = Vec::new();
    let mut scopes = vec![ScopeKind::Module];
    let mut pending_introducer = None;
    let mut declaration_active = false;
    let mut rhs_parens = 0usize;
    let mut rhs_brackets = 0usize;
    let mut previous_line = None;

    for (index, token) in tokens.iter().enumerate() {
        // Everything between a binding head and its RHS belongs to that
        // declaration: function parameters, return types, and aggregate
        // fields must not look like module-level bindings. A newline ends a
        // flat RHS when it is not inside `(...)`/`[...]`; braces are handled
        // below so function/aggregate bodies remain opaque while `mod` bodies
        // become a new declaration scope.
        if declaration_active && scopes.last() == Some(&ScopeKind::Module) {
            let line_changed = previous_line.is_some_and(|line| token.line > line);
            let next_declaration = rhs_parens == 0
                && rhs_brackets == 0
                && parse_declaration(&tokens, source, index).is_some();
            if line_changed && next_declaration {
                declaration_active = false;
            }
        }

        if !declaration_active && scopes.last() == Some(&ScopeKind::Module) {
            if let Some(declaration) = parse_declaration(&tokens, source, index) {
                for &name_index in &declaration.names {
                    let name = &tokens[name_index];
                    let name_text = declaration
                        .display_name
                        .as_deref()
                        .unwrap_or(&source[name.start..name.end]);
                    if name_text.is_empty() {
                        continue;
                    }
                    symbols.push(SymbolInfo {
                        name: name_text.to_string(),
                        full_name: None,
                        kind: declaration.kind,
                        line: name.line,
                        column: name.column,
                        depth: module_depth(&scopes),
                    });
                }
                declaration_active = true;
                rhs_parens = 0;
                rhs_brackets = 0;
            }
        }

        if token.kind == TokenKind::Word {
            match &source[token.start..token.end] {
                "fn" => pending_introducer = Some(ScopeKind::Function),
                "struct" | "enum" | "union" => pending_introducer = Some(ScopeKind::Aggregate),
                "mod" => pending_introducer = Some(ScopeKind::Module),
                _ => {}
            }
        }

        if declaration_active {
            match token.kind {
                TokenKind::LParen => rhs_parens += 1,
                TokenKind::RParen => rhs_parens = rhs_parens.saturating_sub(1),
                TokenKind::LBracket => rhs_brackets += 1,
                TokenKind::RBracket => rhs_brackets = rhs_brackets.saturating_sub(1),
                _ => {}
            }
        }

        match token.kind {
            TokenKind::LBrace => {
                let scope = pending_introducer.take().unwrap_or(ScopeKind::Other);
                scopes.push(scope);
                if scope == ScopeKind::Module {
                    declaration_active = false;
                    rhs_parens = 0;
                    rhs_brackets = 0;
                }
            }
            TokenKind::RBrace => {
                if scopes.len() > 1 {
                    scopes.pop();
                }
                if declaration_active && scopes.last() == Some(&ScopeKind::Module) {
                    declaration_active = false;
                    rhs_parens = 0;
                    rhs_brackets = 0;
                }
                pending_introducer = None;
            }
            TokenKind::Semi
                if declaration_active
                    && scopes.last() == Some(&ScopeKind::Module)
                    && rhs_parens == 0
                    && rhs_brackets == 0 =>
            {
                declaration_active = false;
            }
            _ => {}
        }

        previous_line = Some(token.line);
    }

    symbols
}

fn module_depth(scopes: &[ScopeKind]) -> usize {
    scopes
        .iter()
        .filter(|scope| **scope == ScopeKind::Module)
        .count()
        .saturating_sub(1)
}

fn parse_declaration(tokens: &[Token], source: &str, start: usize) -> Option<Declaration> {
    let (mut index, has_test) = skip_modifiers(tokens, source, start);

    // Alatyr permits an anonymous `@test(...) fn { ... }` item.  It has no
    // binding name to navigate to, so use a stable display label when it is
    // encountered.  Named test bindings take the normal path below.
    if index < tokens.len()
        && tokens[index].kind == TokenKind::Word
        && word_is(tokens, source, index, "fn")
        && has_test
    {
        return Some(Declaration {
            names: vec![index],
            display_name: Some("test".to_string()),
            kind: SymbolKind::Function,
        });
    }

    let mut names = Vec::new();
    if index < tokens.len() && tokens[index].kind == TokenKind::Word {
        if !is_reserved_word(&source[tokens[index].start..tokens[index].end]) {
            names.push(index);
            index += 1;

            // The self-hosted compiler also accepts the historical generic
            // aggregate surface `Box(T) := struct/enum { ... }`.  It is
            // distinct from a projection binding because the name precedes
            // the parameter list.
            if tokens
                .get(index)
                .is_some_and(|token| token.kind == TokenKind::LParen)
            {
                index = skip_generic_parameters(tokens, index)?;
            }
        } else {
            return None;
        }
    } else if index < tokens.len() && tokens[index].kind == TokenKind::LParen {
        index += 1;
        loop {
            let name = tokens.get(index)?;
            if name.kind != TokenKind::Word || is_reserved_word(&source[name.start..name.end]) {
                return None;
            }
            names.push(index);
            index += 1;

            match tokens.get(index).map(|token| token.kind) {
                Some(TokenKind::Comma) => index += 1,
                Some(TokenKind::RParen) => {
                    index += 1;
                    break;
                }
                _ => return None,
            }
        }
    } else if index < tokens.len()
        && tokens[index].kind == TokenKind::Other
        && is_operator_name(&source[tokens[index].start..tokens[index].end])
    {
        names.push(index);
        index += 1;
    } else {
        return None;
    }

    let operator = tokens.get(index)?.kind;
    let rhs_start = match operator {
        TokenKind::ColonEq => Some(index + 1),
        TokenKind::Colon => find_typed_rhs(tokens, index + 1),
        _ => return None,
    };

    let kind = rhs_start
        .map(|rhs| classify_rhs(tokens, source, rhs))
        .unwrap_or(SymbolKind::Constant);

    Some(Declaration {
        names,
        display_name: None,
        kind,
    })
}

fn skip_generic_parameters(tokens: &[Token], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut parameter_count = 0usize;

    loop {
        let parameter = tokens.get(index)?;
        if parameter.kind != TokenKind::Word {
            return None;
        }
        parameter_count += 1;
        index += 1;

        match tokens.get(index).map(|token| token.kind) {
            Some(TokenKind::Comma) => index += 1,
            Some(TokenKind::RParen) if parameter_count > 0 => return Some(index + 1),
            _ => return None,
        }
    }
}

fn skip_modifiers(tokens: &[Token], source: &str, start: usize) -> (usize, bool) {
    let mut index = start;
    let mut has_test = false;

    loop {
        let Some(token) = tokens.get(index) else {
            break;
        };
        if token.kind == TokenKind::Word
            && matches!(&source[token.start..token.end], "pub" | "mut" | "comptime")
        {
            index += 1;
            continue;
        }
        if token.kind != TokenKind::At {
            break;
        }

        index += 1;
        let Some(name) = tokens.get(index) else {
            break;
        };
        if name.kind != TokenKind::Word {
            break;
        }
        if &source[name.start..name.end] == "test" {
            has_test = true;
        }
        index += 1;
        if tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::LParen)
        {
            index = skip_balanced(tokens, index);
        }
    }

    (index, has_test)
}

fn find_typed_rhs(tokens: &[Token], start: usize) -> Option<usize> {
    let mut index = start;
    let mut parens = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;

    while let Some(token) = tokens.get(index) {
        match token.kind {
            TokenKind::LParen => parens += 1,
            TokenKind::RParen => parens = parens.saturating_sub(1),
            TokenKind::LBracket => brackets += 1,
            TokenKind::RBracket => brackets = brackets.saturating_sub(1),
            TokenKind::LBrace => braces += 1,
            TokenKind::RBrace => {
                if braces == 0 {
                    return None;
                }
                braces -= 1;
            }
            TokenKind::Eq if parens == 0 && brackets == 0 && braces == 0 => return Some(index + 1),
            _ => {}
        }
        index += 1;
    }

    None
}

fn classify_rhs(tokens: &[Token], source: &str, start: usize) -> SymbolKind {
    let mut index = start;
    loop {
        let Some(token) = tokens.get(index) else {
            return SymbolKind::Constant;
        };
        if token.kind != TokenKind::At {
            break;
        }
        index += 2; // `@` plus the attribute name
        if tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::LParen)
        {
            index = skip_balanced(tokens, index);
        }
    }

    let Some(token) = tokens.get(index) else {
        return SymbolKind::Constant;
    };
    if token.kind != TokenKind::Word {
        return SymbolKind::Constant;
    }

    match &source[token.start..token.end] {
        "fn" => SymbolKind::Function,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        // SymbolKind has no separate union glyph yet; both are aggregate type
        // declarations and share the existing structure icon.
        "union" => SymbolKind::Struct,
        "mod" => SymbolKind::Module,
        "brand" => SymbolKind::TypeAlias,
        _ => SymbolKind::Constant,
    }
}

fn skip_balanced(tokens: &[Token], start: usize) -> usize {
    let Some(open) = tokens.get(start).map(|token| token.kind) else {
        return start;
    };
    let close = match open {
        TokenKind::LParen => TokenKind::RParen,
        TokenKind::LBracket => TokenKind::RBracket,
        TokenKind::LBrace => TokenKind::RBrace,
        _ => return start,
    };

    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            kind if kind == open => depth += 1,
            kind if kind == close => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    tokens.len()
}

fn word_is(tokens: &[Token], source: &str, index: usize, word: &str) -> bool {
    tokens.get(index).is_some_and(|token| {
        token.kind == TokenKind::Word && &source[token.start..token.end] == word
    })
}

fn is_operator_name(name: &str) -> bool {
    matches!(name, "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^")
}

fn is_reserved_word(word: &str) -> bool {
    matches!(
        word,
        "pub"
            | "mut"
            | "comptime"
            | "unchecked"
            | "when"
            | "and"
            | "or"
            | "not"
            | "if"
            | "else"
            | "match"
            | "loop"
            | "while"
            | "for"
            | "break"
            | "continue"
            | "return"
            | "defer"
            | "struct"
            | "enum"
            | "union"
            | "fn"
            | "type"
            | "mod"
            | "abi"
            | "dyn"
            | "async"
            | "await"
    )
}

fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    let mut line = 0usize;
    let mut column = 0usize;

    while index < bytes.len() {
        let start = index;
        let token_line = line;
        let token_column = column;
        let byte = bytes[index];

        match byte {
            b' ' | b'\t' => {
                index += 1;
                column += 1;
            }
            b'\n' => {
                index += 1;
                line += 1;
                column = 0;
            }
            b'\r' => {
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                line += 1;
                column = 0;
            }
            b'#' => {
                while index < bytes.len() && bytes[index] != b'\n' && bytes[index] != b'\r' {
                    index += 1;
                    column += 1;
                }
            }
            b'"' | b'\'' => {
                let quote = byte;
                index += 1;
                column += 1;
                while index < bytes.len() {
                    let current = bytes[index];
                    if current == b'\\' {
                        index += 1;
                        column += 1;
                        if index < bytes.len() {
                            index += 1;
                            column += 1;
                        }
                    } else if current == quote {
                        index += 1;
                        column += 1;
                        break;
                    } else if current == b'\n' {
                        index += 1;
                        line += 1;
                        column = 0;
                    } else if current == b'\r' {
                        index += 1;
                        if bytes.get(index) == Some(&b'\n') {
                            index += 1;
                        }
                        line += 1;
                        column = 0;
                    } else {
                        index += 1;
                        column += 1;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::Quoted,
                    start,
                    end: index,
                    line: token_line,
                    column: token_column,
                });
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                index += 1;
                column += 1;
                while let Some(next) = bytes.get(index) {
                    if next.is_ascii_alphanumeric() || *next == b'_' {
                        index += 1;
                        column += 1;
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::Word,
                    start,
                    end: index,
                    line: token_line,
                    column: token_column,
                });
            }
            b'0'..=b'9' => {
                index += 1;
                column += 1;
                while let Some(next) = bytes.get(index) {
                    if next.is_ascii_alphanumeric() || matches!(*next, b'_' | b'.') {
                        index += 1;
                        column += 1;
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::Other,
                    start,
                    end: index,
                    line: token_line,
                    column: token_column,
                });
            }
            b'@' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::At,
                start,
                token_line,
                token_column,
            ),
            b'{' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::LBrace,
                start,
                token_line,
                token_column,
            ),
            b'}' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::RBrace,
                start,
                token_line,
                token_column,
            ),
            b'(' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::LParen,
                start,
                token_line,
                token_column,
            ),
            b')' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::RParen,
                start,
                token_line,
                token_column,
            ),
            b'[' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::LBracket,
                start,
                token_line,
                token_column,
            ),
            b']' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::RBracket,
                start,
                token_line,
                token_column,
            ),
            b',' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::Comma,
                start,
                token_line,
                token_column,
            ),
            b';' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::Semi,
                start,
                token_line,
                token_column,
            ),
            b':' if bytes.get(index + 1) == Some(&b'=') => push_double(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::ColonEq,
                start,
                token_line,
                token_column,
            ),
            b':' => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::Colon,
                start,
                token_line,
                token_column,
            ),
            b'=' if !matches!(bytes.get(index + 1), Some(b'=') | Some(b'>')) => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::Eq,
                start,
                token_line,
                token_column,
            ),
            _ => push_single(
                &mut tokens,
                &mut index,
                &mut column,
                TokenKind::Other,
                start,
                token_line,
                token_column,
            ),
        }
    }

    tokens
}

fn push_single(
    tokens: &mut Vec<Token>,
    index: &mut usize,
    column: &mut usize,
    kind: TokenKind,
    start: usize,
    line: usize,
    token_column: usize,
) {
    *index += 1;
    *column += 1;
    tokens.push(Token {
        kind,
        start,
        end: *index,
        line,
        column: token_column,
    });
}

fn push_double(
    tokens: &mut Vec<Token>,
    index: &mut usize,
    column: &mut usize,
    kind: TokenKind,
    start: usize,
    line: usize,
    token_column: usize,
) {
    *index += 2;
    *column += 2;
    tokens.push(Token {
        kind,
        start,
        end: *index,
        line,
        column: token_column,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<(String, SymbolKind, usize, usize)> {
        extract_symbols(source)
            .iter()
            .map(|symbol| (symbol.name.clone(), symbol.kind, symbol.depth, symbol.line))
            .collect()
    }

    #[test]
    fn extracts_alatyr_declarations_and_inline_modules() {
        let source = r#"## A doc comment with fake := fn()
pub Point := struct { x : u64, y : u64 }
pub Color := enum { Red, Green }
pub add := fn(a : u64, b : u64) -> u64 { a + b }
pub Geometry := mod {
    pub circle := fn(r : u64) -> u64 { r }
    Hidden := struct { value : u64 }
}
value := "not := fn() { fake }"
"#;

        let found = names(source);
        assert_eq!(
            found,
            vec![
                ("Point".into(), SymbolKind::Struct, 0, 1),
                ("Color".into(), SymbolKind::Enum, 0, 2),
                ("add".into(), SymbolKind::Function, 0, 3),
                ("Geometry".into(), SymbolKind::Module, 0, 4),
                ("circle".into(), SymbolKind::Function, 1, 5),
                ("Hidden".into(), SymbolKind::Struct, 1, 6),
                ("value".into(), SymbolKind::Constant, 0, 8),
            ]
        );
    }

    #[test]
    fn ignores_function_locals_and_supports_attributes_and_typed_bindings() {
        let source = r#"@inline pub answer : u64 = 42
@test("works") tested := fn() -> bool {
    local := fn() { 1 }
    local_value : u64 = 1
    true
}
@extern pub printf :=
    @abi(c)
    fn(
        fmt : str
    )
Meters := brand(u64)
"#;

        let found = names(source);
        assert_eq!(
            found,
            vec![
                ("answer".into(), SymbolKind::Constant, 0, 0),
                ("tested".into(), SymbolKind::Function, 0, 1),
                ("printf".into(), SymbolKind::Function, 0, 6),
                ("Meters".into(), SymbolKind::TypeAlias, 0, 11),
            ]
        );
    }

    #[test]
    fn extracts_projection_and_operator_bindings() {
        let source = "(fmt, vec) := std::fmt\n                      + := fn(a : u64, b : u64) -> u64 { a + b }\n";
        let found = names(source);
        assert_eq!(
            found,
            vec![
                ("fmt".into(), SymbolKind::Constant, 0, 0),
                ("vec".into(), SymbolKind::Constant, 0, 0),
                ("+".into(), SymbolKind::Function, 0, 1),
            ]
        );
    }

    #[test]
    fn keeps_multiline_function_signatures_opaque() {
        let source =
            "multiline :=\n    fn(\n        value : u64\n    ) -> u64 { value }\nnext := 1\n";
        let found = names(source);
        assert_eq!(
            found,
            vec![
                ("multiline".into(), SymbolKind::Function, 0, 0),
                ("next".into(), SymbolKind::Constant, 0, 4),
            ]
        );
    }

    #[test]
    fn gives_anonymous_tests_a_navigable_label() {
        let source = "@test(\"smoke\") fn() { true }\nnext := 1\n";
        let found = names(source);
        assert_eq!(
            found,
            vec![
                ("test".into(), SymbolKind::Function, 0, 0),
                ("next".into(), SymbolKind::Constant, 0, 1),
            ]
        );
    }

    #[test]
    fn supports_generic_aggregate_bindings() {
        let source =
            "Box(T) := struct { value : T }\nOpt(T) := enum { Some(T), None }\nnext := 1\n";
        let found = names(source);
        assert_eq!(
            found,
            vec![
                ("Box".into(), SymbolKind::Struct, 0, 0),
                ("Opt".into(), SymbolKind::Enum, 0, 1),
                ("next".into(), SymbolKind::Constant, 0, 2),
            ]
        );
    }
}

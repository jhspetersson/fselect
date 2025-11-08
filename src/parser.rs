//! Handles the parsing of the query string

use std::path::PathBuf;
use std::str::FromStr;

use directories::UserDirs;
use crate::expr::Expr;
use crate::field::Field;
use crate::function::Function;
use crate::lexer::Lexeme;
use crate::lexer::Lexer;
use crate::operators::ArithmeticOp;
use crate::operators::LogicalOp;
use crate::operators::Op;
use crate::query::Query;
use crate::query::Root;
use crate::query::TraversalMode::{Bfs, Dfs};
use crate::query::{OutputFormat, RootOptions};
#[cfg(not(feature = "git"))]
use crate::util::error_message;

pub struct Parser<'a> {
    lexer: &'a mut Lexer,
    lexemes: Vec<Lexeme>,
    index: isize,
    roots_parsed: bool,
    where_parsed: bool,
    debug: bool,
}

impl <'a> Parser<'a> {
    pub fn new(lexer: &mut Lexer) -> Parser<'_> {
        Parser {
            lexer,
            lexemes: vec![],
            index: -1,
            roots_parsed: false,
            where_parsed: false,
            debug: false,
        }
    }

    pub fn parse(&mut self, debug: bool) -> Result<Query, String> {
        self.debug = debug;

        if let Some(Lexeme::Select) = self.next_lexeme() {
            // skip the "select" keyword
        } else {
            self.drop_lexeme();
        }

        let fields = self.parse_fields()?;
        let mut roots = self.parse_roots()?;
        let root_options = self.parse_root_options()?;
        self.roots_parsed = true;
        let expr = self.parse_where()?;
        self.where_parsed = true;
        let grouping_fields = self.parse_group_by()?;
        let (ordering_fields, ordering_asc) = self.parse_order_by(&fields)?;
        let mut limit = self.parse_limit()?;
        let output_format = self.parse_output_format()?;

        if roots.is_empty() {
            roots = self.parse_roots()?;
        }

        if roots.is_empty() {
            roots.push(Root::default(root_options));
        }

        if limit == 0
            && fields
                .iter()
                .all(|expr| expr.get_required_fields().is_empty())
        {
            limit = 1;
        }

        Ok(Query {
            fields,
            roots,
            expr,
            grouping_fields,
            ordering_fields,
            ordering_asc,
            limit,
            output_format,
        })
    }

    fn parse_fields(&mut self) -> Result<Vec<Expr>, String> {
        let mut fields = vec![];

        loop {
            let lexeme = self.next_lexeme();
            match lexeme {
                Some(Lexeme::Comma) => {
                    // skip
                }
                Some(Lexeme::String(ref s))
                | Some(Lexeme::RawString(ref s))
                | Some(Lexeme::ArithmeticOperator(ref s)) => {
                    if s == "*" {
                        #[cfg(unix)]
                        {
                            fields.push(Expr::field(Field::Mode));
                            #[cfg(feature = "users")]
                            fields.push(Expr::field(Field::User));
                            #[cfg(feature = "users")]
                            fields.push(Expr::field(Field::Group));
                        }

                        fields.push(Expr::field(Field::Size));
                        fields.push(Expr::field(Field::Modified));
                        fields.push(Expr::field(Field::Path));
                    } else {
                        if s.to_lowercase() == "group" {
                            if let Some(Lexeme::By) = self.next_lexeme() {
                                self.drop_lexeme();
                                self.drop_lexeme();
                                break;
                            } else {
                                self.drop_lexeme();
                            }
                        }

                        self.drop_lexeme();

                        if Self::is_root_option_keyword(s) {
                            break;
                        }

                        if let Ok(Some(field)) = self.parse_expr() {
                            fields.push(field);
                        }
                    }
                }
                Some(Lexeme::Open) | Some(Lexeme::CurlyOpen) => {
                    self.drop_lexeme();
                    if let Ok(Some(field)) = self.parse_expr() {
                        fields.push(field);
                    }
                }
                _ => {
                    self.drop_lexeme();
                    break;
                }
            }
        }

        if fields.is_empty() {
            return Err(String::from("Error parsing fields, no selector found"));
        }

        Ok(fields)
    }

    fn parse_roots(&mut self) -> Result<Vec<Root>, String> {
        enum RootParsingMode {
            Unknown,
            From,
            Root,
            Comma,
        }

        let mut roots: Vec<Root> = Vec::new();
        let mut mode = RootParsingMode::Unknown;

        if let Some(ref lexeme) = self.next_lexeme() {
            match lexeme {
                &Lexeme::From => {
                    mode = RootParsingMode::From;
                }
                _ => {
                    self.drop_lexeme();
                }
            }
        }

        if let RootParsingMode::From = mode {
            let mut path: String = String::from("");
            let mut root_options = RootOptions::new();

            loop {
                let lexeme = self.next_lexeme();
                match lexeme {
                    Some(ref lexeme) => match lexeme {
                        Lexeme::String(s) | Lexeme::RawString(s) => match mode {
                            RootParsingMode::From | RootParsingMode::Comma => {
                                path = s.to_string();
                                if path.starts_with("~") {
                                    if let Some(ud) = UserDirs::new() {
                                        let mut pb = PathBuf::from(path.clone());
                                        pb = pb.components().skip(1).collect();
                                        pb = ud.home_dir().to_path_buf().join(pb);
                                        path = pb.to_string_lossy().to_string();
                                    }
                                }
                                mode = RootParsingMode::Root;
                            }
                            RootParsingMode::Root => {
                                if s.to_lowercase() == "group" {
                                    if let Some(Lexeme::By) = self.next_lexeme() {
                                        self.drop_lexeme();
                                        self.drop_lexeme();

                                        if !path.is_empty() {
                                            roots.push(Root::new(path, root_options));
                                        }
                                        break;
                                    }
                                }

                                self.drop_lexeme();
                                match self.parse_root_options()? {
                                    Some(options) => root_options = options,
                                    None => {
                                        roots.push(Root::new(path, RootOptions::new()));
                                        break
                                    }
                                }
                            }
                            _ => {}
                        },
                        Lexeme::Comma => {
                            if !path.is_empty() {
                                roots.push(Root::new(path, root_options));

                                path = String::from("");
                                root_options = RootOptions::new();

                                mode = RootParsingMode::Comma;
                            } else {
                                self.drop_lexeme();
                                break;
                            }
                        }
                        _ => {
                            if !path.is_empty() {
                                roots.push(Root::new(path, root_options));
                            }

                            self.drop_lexeme();
                            break;
                        }
                    },
                    None => {
                        if !path.is_empty() {
                            roots.push(Root::new(path, root_options));
                        }
                        break;
                    }
                }
            }
        }

        Ok(roots)
    }

    fn parse_root_options(&mut self) -> Result<Option<RootOptions>, String> {
        #[derive(Debug, PartialEq)]
        enum RootParsingMode {
            Unknown,
            Options,
            MinDepth,
            Depth,
            Alias,
        }

        let mut mode = RootParsingMode::Unknown;

        let mut min_depth: u32 = 0;
        let mut max_depth: u32 = 0;
        let mut archives = false;
        let mut symlinks = false;
        let mut hardlinks = false;
        let mut gitignore = None;
        let mut hgignore = None;
        let mut dockerignore = None;
        let mut traversal = Bfs;
        let mut regexp = false;
        let mut alias: Option<String> = None;

        loop {
            let lexem = self.next_lexeme();
            match lexem {
                Some(ref lexem) => match lexem {
                    Lexeme::String(s) | Lexeme::RawString(s) => match mode {
                        RootParsingMode::Unknown | RootParsingMode::Options => {
                            let s = s.to_ascii_lowercase();
                            if s == "mindepth" {
                                mode = RootParsingMode::MinDepth;
                            } else if s == "maxdepth" || s == "depth" {
                                mode = RootParsingMode::Depth;
                            } else if s.starts_with("arc") {
                                archives = true;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("sym") {
                                symlinks = true;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("hard") {
                                hardlinks = true;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("git") {
                                #[cfg(feature = "git")]
                                {
                                    gitignore = Some(true);
                                    mode = RootParsingMode::Options;
                                }
                                #[cfg(not(feature = "git"))]
                                {
                                    error_message("parser", "git support is not enabled");
                                    self.drop_lexem();
                                    break;
                                }
                            } else if s.starts_with("hg") {
                                hgignore = Some(true);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("dock") {
                                dockerignore = Some(true);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("nogit") {
                                gitignore = Some(false);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("nohg") {
                                hgignore = Some(false);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("nodock") {
                                dockerignore = Some(false);
                                mode = RootParsingMode::Options;
                            } else if s == "bfs" {
                                traversal = Bfs;
                                mode = RootParsingMode::Options;
                            } else if s == "dfs" {
                                traversal = Dfs;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("regex") {
                                regexp = true;
                                mode = RootParsingMode::Options;
                            } else if s == "as" {
                                mode = RootParsingMode::Alias;
                            } else {
                                self.drop_lexeme();
                                break;
                            }
                        }
                        RootParsingMode::MinDepth => {
                            let d: Result<u32, _> = s.parse();
                            match d {
                                Ok(d) => {
                                    min_depth = d;
                                    mode = RootParsingMode::Options;
                                }
                                _ => {
                                    self.drop_lexeme();
                                    break;
                                }
                            }
                        }
                        RootParsingMode::Depth => {
                            let d: Result<u32, _> = s.parse();
                            match d {
                                Ok(d) => {
                                    max_depth = d;
                                    mode = RootParsingMode::Options;
                                }
                                _ => {
                                    self.drop_lexeme();
                                    break;
                                }
                            }
                        }
                        RootParsingMode::Alias => {
                            alias = Some(s.to_string());
                            mode = RootParsingMode::Options;
                        }
                    },
                    Lexeme::Operator(s) if s.eq("rx") => {
                        regexp = true;
                        mode = RootParsingMode::Options;
                    }
                    _ => {
                        self.drop_lexeme();
                        break;
                    }
                },
                None => {
                    if mode != RootParsingMode::Unknown && mode != RootParsingMode::Options {
                        return Err(String::from("Error parsing root options"));
                    }
                    break;
                }
            }
        }

        match mode {
            RootParsingMode::Unknown => Ok(None),
            _ => Ok(Some(RootOptions {
                min_depth,
                max_depth,
                archives,
                symlinks,
                hardlinks,
                gitignore,
                hgignore,
                dockerignore,
                traversal,
                regexp,
                alias,
            })),
        }
    }

    fn is_root_option_keyword(s: &str) -> bool {
        let s = s.to_ascii_lowercase();
        s == "depth"
            || s == "mindepth"
            || s == "maxdepth"
            || s.starts_with("arc")
            || s.starts_with("sym")
            || s.starts_with("hard")
            || s.starts_with("git")
            || s.starts_with("hg")
            || s.starts_with("dock")
            || s.starts_with("nogit")
            || s.starts_with("nohg")
            || s.starts_with("nodock")
            || s == "bfs"
            || s == "dfs"
            || s.starts_with("regex")
            || s == "as"
    }

    /*

    expr        := and (OR and)*
    and         := cond (AND cond)*
    cond        := add_sub (OP add_sub)*
    add_sub     := mul_div (PLUS mul_div)* | mul_div (MINUS mul_div)*
    mul_div     := paren (MUL paren)* | paren (DIV paren)*
    paren       := ( expr ) | func_scalar
    func_scalar := function paren | field | scalar

    */

    fn parse_where(&mut self) -> Result<Option<Expr>, String> {
        match self.next_lexeme() {
            Some(Lexeme::Where) => self.parse_expr(),
            _ => {
                self.drop_lexeme();
                Ok(None)
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_and()?;

        let mut right: Option<Expr> = None;
        loop {
            let lexeme = self.next_lexeme();
            match lexeme {
                Some(Lexeme::Or) => {
                    let expr = self.parse_and()?;
                    right = match right {
                        Some(right) => Some(Expr::logical_op(
                            right,
                            LogicalOp::Or,
                            expr.clone().unwrap(),
                        )),
                        None => expr,
                    };
                }
                _ => {
                    self.drop_lexeme();

                    return match right {
                        Some(right) => {
                            match left.as_ref().unwrap().weight <= right.weight {
                                true => Ok(Some(Expr::logical_op(
                                    left.unwrap(),
                                    LogicalOp::Or,
                                    right,
                                ))),
                                false => Ok(Some(Expr::logical_op(
                                    right,
                                    LogicalOp::Or,
                                    left.unwrap(),
                                ))),
                            }
                        }
                        None => Ok(left),
                    };
                }
            }
        }
    }

    fn parse_and(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_cond()?;

        let mut right: Option<Expr> = None;
        loop {
            let lexem = self.next_lexeme();
            match lexem {
                Some(Lexeme::And) => {
                    let expr = self.parse_cond()?;
                    right = match right {
                        Some(right) => Some(Expr::logical_op(right, LogicalOp::And, expr.unwrap())),
                        None => expr,
                    };
                }
                _ => {
                    self.drop_lexeme();

                    return match right {
                        Some(right) => {
                            Ok(Some(Expr::logical_op(left.unwrap(), LogicalOp::And, right)))
                        }
                        None => Ok(left),
                    };
                }
            }
        }
    }

    fn parse_cond(&mut self) -> Result<Option<Expr>, String> {
        let mut negate = false;

        loop {
            if let Some(Lexeme::Not) = self.next_lexeme() {
                negate = !negate;
            } else {
                self.drop_lexeme();
                break;
            }
        }

        let left = self.parse_add_sub()?;

        let mut not = false;

        let lexem = self.next_lexeme();
        match lexem {
            Some(Lexeme::Not) => {
                not = true;
            }
            _ => {
                self.drop_lexeme();
            }
        };

        let lexem = self.next_lexeme();
        let mut result = match lexem {
            Some(Lexeme::Operator(s)) if s.as_str() == "between" => {
                let left_between = self.parse_add_sub()?;

                let and_lexem = self.next_lexeme();
                if and_lexem.is_none() || and_lexem.unwrap() != Lexeme::And {
                    return Err(String::from("Error parsing BETWEEN operator"));
                }

                let right_between = self.parse_add_sub()?;

                let left_expr = Expr::op(
                    left.clone().unwrap(),
                    match not {
                        false => Op::Gte,
                        true => Op::Lte,
                    },
                    left_between.unwrap(),
                );
                let right_expr = Expr::op(
                    left.unwrap(),
                    match not {
                        false => Op::Lte,
                        true => Op::Gte,
                    },
                    right_between.unwrap(),
                );

                Ok(Some(Expr::logical_op(
                    left_expr,
                    match not {
                        false => LogicalOp::And,
                        true => LogicalOp::Or,
                    },
                    right_expr,
                )))
            }
            Some(Lexeme::Operator(s)) if s.as_str() == "in" => {
                let list = self.parse_list()?;
                let op = Op::from_with_not(s, not);
                Ok(Some(Expr::op(
                    left.unwrap(),
                    op.unwrap(),
                    list,
                )))
            }
            Some(Lexeme::Operator(s)) => {
                let right = self.parse_add_sub()?;
                let op = Op::from_with_not(s, not);
                Ok(Some(Expr::op(left.unwrap(), op.unwrap(), right.unwrap())))
            }
            _ => {
                self.drop_lexeme();
                Ok(left)
            }
        };

        if let Ok(Some(expr)) = result.clone() {
            if let Some(field) = expr.field {
                if expr.left.is_none()
                    && expr.right.is_none()
                    && field.is_boolean_field()
                    && self.roots_parsed
                    && !self.where_parsed
                {
                    result = Ok(Some(Expr::op(
                        Expr::field(field),
                        Op::Eq,
                        Expr::value(String::from("true")),
                    )));
                }
            } else if let Some(function) = expr.function {
                if expr.right.is_none()
                    && (expr.args.is_none() || expr.args.unwrap().is_empty())
                    && function.is_boolean_function()
                    && self.roots_parsed
                    && !self.where_parsed
                {
                    let func_expr = Expr::function_left(function, expr.left);
                    result = Ok(Some(Expr::op(
                        func_expr,
                        Op::Eq,
                        Expr::value(String::from("true")),
                    )));
                }
            }
        }

        if negate {
            if let Ok(Some(expr)) = result {
                return Ok(Some(Self::negate_expr_op(&expr)));
            }
        }

        result
    }

    fn parse_add_sub(&mut self) -> Result<Option<Expr>, String> {
        let mut left = self.parse_mul_div()?;

        let mut op = None;
        loop {
            let lexem = self.next_lexeme();
            if let Some(Lexeme::ArithmeticOperator(s)) = lexem {
                let new_op = ArithmeticOp::from(s);
                match new_op {
                    Some(ArithmeticOp::Add) | Some(ArithmeticOp::Subtract) => {
                        let expr = self.parse_mul_div()?;
                        if op.is_none() {
                            op = new_op.clone();
                        }

                        left = match left {
                            Some(left) => {
                                Some(Expr::arithmetic_op(left, new_op.unwrap(), expr.unwrap()))
                            }
                            None => expr,
                        };
                    }
                    _ => {
                        self.drop_lexeme();

                        return Ok(left);
                    }
                }
            } else {
                self.drop_lexeme();

                return Ok(left);
            }
        }
    }

    fn parse_mul_div(&mut self) -> Result<Option<Expr>, String> {
        let mut left = self.parse_paren()?;

        let mut op = None;
        loop {
            let lexem = self.next_lexeme();
            if let Some(Lexeme::ArithmeticOperator(s)) = lexem {
                let new_op = ArithmeticOp::from(s);
                match new_op {
                    Some(ArithmeticOp::Multiply)
                    | Some(ArithmeticOp::Divide)
                    | Some(ArithmeticOp::Modulo) => {
                        let expr = self.parse_paren()?;
                        if op.is_none() {
                            op = new_op.clone();
                        }

                        left = match left {
                            Some(left) => {
                                Some(Expr::arithmetic_op(left, new_op.unwrap(), expr.unwrap()))
                            }
                            None => expr,
                        };
                    }
                    _ => {
                        self.drop_lexeme();

                        return Ok(left);
                    }
                }
            } else {
                self.drop_lexeme();

                return Ok(left);
            }
        }
    }

    fn parse_paren(&mut self) -> Result<Option<Expr>, String> {
        match self.next_lexeme() {
            Some(Lexeme::Open) => {
                let result = self.parse_expr();
                if let Some(Lexeme::Close) = self.next_lexeme() {
                    result
                } else {
                    Err("Unmatched parenthesis".to_string())
                }
            }
            Some(Lexeme::CurlyOpen) => {
                let result = self.parse_expr();
                if let Some(Lexeme::CurlyClose) = self.next_lexeme() {
                    result
                } else {
                    Err("Unmatched parenthesis".to_string())
                }
            }
            _ => {
                self.drop_lexeme();
                self.parse_func_scalar()
            }
        }
    }

    fn parse_list(&mut self) -> Result<Expr, String> {
        match self.next_lexeme() {
            Some(Lexeme::Open) => {
                let result = {
                    if let Some(Lexeme::Select) = self.next_lexeme() {
                        self.lexer.push_state();
                        let mut parser = Parser::new(&mut self.lexer);
                        let query = parser.parse(self.debug)?;
                        self.lexer.pop_state();
                        self.push_lexeme(Lexeme::Close);
                        Expr::subquery(query)
                    } else {
                        self.drop_lexeme();
                        let mut result = Expr::new();
                        let args = self.parse_args()?;
                        result.set_args(args.unwrap());
                        result
                    }
                };

                if let Some(Lexeme::Close) = self.next_lexeme() {
                    Ok(result)
                } else {
                    self.drop_lexeme();
                    Err("Unmatched parenthesis".to_string())
                }
            }
            Some(Lexeme::CurlyOpen) => {
                let mut result = Expr::new();
                let args = self.parse_args()?;
                result.set_args(args.unwrap());
                if let Some(Lexeme::CurlyClose) = self.next_lexeme() {
                    Ok(result)
                } else {
                    Err("Unmatched parenthesis".to_string())
                }
            }
            _ => {
                self.drop_lexeme();
                Err("Error parsing list".to_string())
            }
        }
    }

    fn parse_args(&mut self) -> Result<Option<Vec<Expr>>, String> {
        let mut args = vec![];

        loop {
            match self.next_lexeme() {
                Some(Lexeme::Comma) => {}
                _ => {
                    self.drop_lexeme();
                    match self.parse_expr() {
                        Ok(Some(expr)) => args.push(expr),
                        _ => {
                            self.drop_lexeme();
                            break
                        }
                    }
                }
            }
        }

        Ok(Some(args))
    }

    fn parse_func_scalar(&mut self) -> Result<Option<Expr>, String> {
        let mut lexem = self.next_lexeme();
        let mut minus = false;

        if let Some(Lexeme::ArithmeticOperator(ref s)) = lexem {
            if s == "-" {
                minus = true;
                lexem = self.next_lexeme();
            } else if s == "+" {
                // nop
            } else {
                self.drop_lexeme();
            }
        }

        match lexem {
            Some(Lexeme::String(ref s)) | Some(Lexeme::RawString(ref s)) => {
                if let Ok((field, root_alias)) = Field::parse_field(s) {
                    let mut expr = Expr::field_with_root_alias(field, root_alias);
                    expr.minus = minus;
                    return Ok(Some(expr));
                }

                if let Ok(function) = Function::from_str(s) {
                    match self.parse_function(function) {
                        Ok(expr) => {
                            let mut expr = expr;
                            expr.minus = minus;
                            return Ok(Some(expr));
                        }
                        Err(err) => return Err(err),
                    }
                }

                let mut expr = Expr::value(s.to_string());
                expr.minus = minus;

                Ok(Some(expr))
            }
            _ => Err("Error parsing expression, expecting string".to_string()),
        }
    }

    fn parse_function(&mut self, function: Function) -> Result<Expr, String> {
        let is_boolean_function = function.is_boolean_function();
        let mut function_expr = Expr::function(function);

        let mut curly_mode = false;
        if let Some(lexem) = self.next_lexeme() {
            if lexem != Lexeme::Open && lexem != Lexeme::CurlyOpen {
                if is_boolean_function {
                    return Ok(function_expr);
                }

                return Err("Error in function expression".to_string());
            }

            if lexem == Lexeme::CurlyOpen {
                curly_mode = true;
            }
        }

        if let Ok(Some(function_arg)) = self.parse_expr() {
            function_expr.add_left(function_arg);
        } else {
            return Ok(function_expr);
        }

        let mut args = vec![];

        loop {
            match self.next_lexeme() {
                Some(Lexeme::Comma) => match self.parse_expr() {
                    Ok(Some(expr)) => args.push(expr),
                    _ => {
                        return Err("Error in function expression".to_string());
                    }
                },
                Some(lexem)
                    if (lexem == Lexeme::Close && !curly_mode)
                        || (lexem == Lexeme::CurlyClose && curly_mode) =>
                {
                    function_expr.set_args(args);
                    return Ok(function_expr);
                }
                _ => {
                    return Err("Error in function expression".to_string());
                }
            }
        }
    }

    fn parse_group_by(&mut self) -> Result<Vec<Expr>, String> {
        let mut group_by_fields: Vec<Expr> = vec![];

        if let Some(Lexeme::RawString(s)) = self.next_lexeme() {
            if s.to_lowercase() == "group" {
                if let Some(Lexeme::By) = self.next_lexeme() {
                    loop {
                        match self.next_lexeme() {
                            Some(Lexeme::Comma) => {}
                            Some(Lexeme::RawString(_)) => {
                                self.drop_lexeme();
                                let group_field = self.parse_expr().unwrap().unwrap();
                                group_by_fields.push(group_field);
                            }
                            _ => {
                                self.drop_lexeme();
                                break;
                            }
                        }
                    }
                } else {
                    self.drop_lexeme();
                }
            } else {
                self.drop_lexeme();
            }            
        } else {
            self.drop_lexeme();
        }

        Ok(group_by_fields)
    }

    fn parse_order_by(&mut self, fields: &[Expr]) -> Result<(Vec<Expr>, Vec<bool>), String> {
        let mut order_by_fields: Vec<Expr> = vec![];
        let mut order_by_directions: Vec<bool> = vec![];

        if let Some(Lexeme::Order) = self.next_lexeme() {
            if let Some(Lexeme::By) = self.next_lexeme() {
                loop {
                    match self.next_lexeme() {
                        Some(Lexeme::Comma) => {}
                        Some(Lexeme::RawString(ref ordering_field)) => {
                            let actual_field = match ordering_field.parse::<usize>() {
                                Ok(idx) => fields[idx - 1].clone(),
                                _ => {
                                    self.drop_lexeme();
                                    self.parse_expr().unwrap().unwrap()
                                }
                            };
                            order_by_fields.push(actual_field);
                            order_by_directions.push(true);
                        }
                        Some(Lexeme::DescendingOrder) => {
                            let cnt = order_by_directions.len();
                            order_by_directions[cnt - 1] = false;
                        }
                        _ => {
                            self.drop_lexeme();
                            break;
                        }
                    }
                }
            } else {
                self.drop_lexeme();
            }
        } else {
            self.drop_lexeme();
        }

        Ok((order_by_fields, order_by_directions))
    }

    fn parse_limit(&mut self) -> Result<u32, &str> {
        let lexem = self.next_lexeme();
        match lexem {
            Some(Lexeme::Limit) => {
                let lexem = self.next_lexeme();
                match lexem {
                    Some(Lexeme::RawString(s)) | Some(Lexeme::String(s)) => {
                        if let Ok(limit) = s.parse() {
                            return Ok(limit);
                        } else {
                            return Err("Error parsing limit");
                        }
                    }
                    _ => {
                        self.drop_lexeme();
                        return Err("Error parsing limit, limit value not found");
                    }
                }
            }
            _ => {
                self.drop_lexeme();
            }
        }

        Ok(0)
    }

    fn parse_output_format(&mut self) -> Result<OutputFormat, &str> {
        let lexem = self.next_lexeme();
        match lexem {
            Some(Lexeme::Into) => {
                let lexem = self.next_lexeme();
                match lexem {
                    Some(Lexeme::RawString(s)) | Some(Lexeme::String(s)) => {
                        return match OutputFormat::from(&s) {
                            Some(output_format) => Ok(output_format),
                            None => Err("Unknown output format"),
                        };
                    }
                    _ => {
                        self.drop_lexeme();
                        return Err("Error parsing output format");
                    }
                }
            }
            _ => {
                self.drop_lexeme();
            }
        }

        Ok(OutputFormat::Tabs)
    }

    pub(crate) fn there_are_remaining_lexemes(&mut self) -> bool {
        let result = self.next_lexeme().is_some();
        if result {
            self.drop_lexeme();
        }

        result
    }

    fn next_lexeme(&mut self) -> Option<Lexeme> {
        self.index += 1;

        match self.lexemes.get(self.index as usize) {
            Some(lexeme) => {
                Some(lexeme.clone())
            }
            None => {
                let lexeme = self.lexer.next_lexeme();
                match lexeme {
                    Some(ref lexeme) => {
                        if self.debug {
                            dbg!(&lexeme);
                        }

                        self.lexemes.push(lexeme.clone());

                        Some(lexeme.clone())
                    }
                    _ => {
                        None
                    }
                }
            }
        }
    }

    fn push_lexeme(&mut self, lexeme: Lexeme) {
        self.lexemes.push(lexeme);
    }

    fn drop_lexeme(&mut self) {
        self.index -= 1;
    }

    fn negate_expr_op(expr: &Expr) -> Expr {
        let mut result = expr.clone();

        if let Some(left) = &expr.left {
            result.left = Some(Box::from(Self::negate_expr_op(left)));
        }

        if let &Some(op) = &expr.op {
            result.op = Some(Op::negate(op));
        }

        if let Some(right) = &expr.right {
            result.right = Some(Box::from(Self::negate_expr_op(right)));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_query() {
        let query = "select name, path ,size , fsize from /";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![
                Expr::field(Field::Name),
                Expr::field(Field::Path),
                Expr::field(Field::Size),
                Expr::field(Field::FormattedSize),
            ]
        );
    }

    #[test]
    fn query() {
        let query = "select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' gitignore , /test6 mindepth 3, /test7 archives DFS, /test8 dfs where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 2, size desc limit 50";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![
                Expr::field(Field::Name),
                Expr::field(Field::Path),
                Expr::field(Field::Size),
                Expr::field(Field::FormattedSize)
            ]
        );

        assert_eq!(
            query.roots,
            vec![
                Root::new(
                    String::from("/test"),
                    RootOptions::from(0, 2, false, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test2"),
                    RootOptions::from(0, 0, true, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test3"),
                    RootOptions::from(0, 3, true, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test4"),
                    RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test5"),
                    RootOptions::from(0, 0, false, false, false, Some(true), None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test6"),
                    RootOptions::from(3, 0, false, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test7"),
                    RootOptions::from(0, 0, true, false, false, None, None, None, Dfs, false, None)
                ),
                Root::new(
                    String::from("/test8"),
                    RootOptions::from(0, 0, false, false, false, None, None, None, Dfs, false, None)
                ),
            ]
        );

        let expr = Expr::logical_op(
            Expr::op(
                Expr::field(Field::Name),
                Op::Eq,
                Expr::value(String::from("xxx")),
            ),
            LogicalOp::Or,
            Expr::logical_op(
                Expr::op(
                    Expr::field(Field::Name),
                    Op::Ne,
                    Expr::value(String::from("123")),
                ),
                LogicalOp::And,
                Expr::logical_op(
                    Expr::op(
                        Expr::field(Field::Size),
                        Op::Gt,
                        Expr::value(String::from("456")),
                    ),
                    LogicalOp::Or,
                    Expr::op(
                        Expr::field(Field::FormattedSize),
                        Op::Lte,
                        Expr::value(String::from("758")),
                    ),
                ),
            ),
        );
        // Query expression must be reordered due to the weight difference of its branches
        assert_eq!(query.expr, Some(expr));
        assert_eq!(
            query.ordering_fields,
            vec![Expr::field(Field::Path), Expr::field(Field::Size)]
        );
        assert_eq!(query.ordering_asc, vec![true, false]);
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn query_with_not() {
        let query = "select name from /test where name not like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_single_not() {
        let query = "select name from /test where not name like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_multiple_not() {
        let query = "select name from /test where not name like '%.tmp' and not name like '%.tst'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let left = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );
        let right = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tst")),
        );
        let expr = Expr::logical_op(left, LogicalOp::And, right);

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_multiple_not_paren() {
        let query =
            "select name from /test where (not name like '%.tmp') and (not name like '%.tst')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let left = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );
        let right = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tst")),
        );
        let expr = Expr::logical_op(left, LogicalOp::And, right);

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_double_not() {
        let query = "select name from /test where not not name like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::Like,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_triple_not() {
        let query = "select name from /test where not not not name like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn broken_query() {
        let query = "select name, path ,size , fsize from / where name != 'foobar' order by size desc limit 10 into csv this is unexpected";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false);

        assert!(query.is_ok());
        assert!(p.there_are_remaining_lexemes());
    }

    #[test]
    fn path_with_spaces() {
        let query = "select name from '/opt/Some Cool Dir/Test This'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/opt/Some Cool Dir/Test This"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );
    }

    #[test]
    fn simple_boolean_syntax() {
        let query = "select name from /home/user where is_audio or is_video";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select name from /home/user where is_audio = true or is_video = true";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn simple_boolean_function_syntax() {
        let query = "select name from /home/user where CONTAINS('foobar') or CONTAINS('bazz')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select name from /home/user where CONTAINS('foobar') = true or CONTAINS('bazz') = true";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn simple_function_without_args_syntax_in_where() {
        let query = "select name, caps from /home/user where HAS_CAPS()";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select name, caps from /home/user where HAS_CAPS";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn simple_function_without_args_syntax() {
        let query = "select CURDATE()";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select CURDATE";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn from_at_the_end_of_the_query() {
        let query = "select name where not name like '%.tmp' from /test gitignore mindepth 2";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(2, 0, false, false, false, Some(true), None, None, Bfs, false, None)
            ),]
        );

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_implicit_root() {
        let query = "select name, size";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(String::from("."), RootOptions::new()),]
        );
    }

    #[test]
    fn query_with_implicit_root_and_root_options() {
        let query = "select name, size depth 2";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("."),
                RootOptions::from(0, 2, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );
    }

    #[test]
    fn use_curly_braces() {
        let query = "select name, (1 + 2) from /home/user limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select name, {1 + 2} from /home/user limit 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_group_by() {
        let query = "select AVG(size) from /test group by mime";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![Expr::function_left(
                Function::Avg,
                Some(Box::new(Expr::field(Field::Size)))
            )]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        assert_eq!(
            query.grouping_fields,
            vec![Expr::field(Field::Mime)]
        );
    }

    #[test]
    fn query_with_between() {
        let query = "select name, size from /test where size between 5mb and 6mb";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select name, size from /test where size gte 5mb and size lte 6mb";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_dfs() {
        let query = "select name from /test dfs group by mime";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Dfs, false, None)
            ),]
        );
    }

    #[test]
    fn reordered_expr_branches_with_different_weights() {
        let query = "select name from /test where CONTAINS('foobar') or name like 'foobar'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let query2 = "select name from /test where name like 'foobar' or CONTAINS('foobar')";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_value_in_string_args() {
        let query = "select name from /test where name in ('foo', 'bar')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("foo")),
            Expr::value(String::from("bar")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::In,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));

        let query = "select name from /test where name not in (foo, bar)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("foo")),
            Expr::value(String::from("bar")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotIn,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_value_in_int_args() {
        let query = "select name from /test where size in (100, 200)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("100")),
            Expr::value(String::from("200")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::In,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_value_in_float_args() {
        let query = "select name from /test where size in (100.0, 200.0)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("100.0")),
            Expr::value(String::from("200.0")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::In,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn simple_subquery() {
        let query = "select name from /test where size > 100 and size in (select size from /test2 where size > 50)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        assert!(query.expr.unwrap().left.unwrap().right.is_some());
    }

    #[test]
    fn complex_subquery() {
        let query = "select name from /test1 where size > 100 and size in (select size from /test2 where name in (select name from /test3 where modified in (select modified from /test4 where size < 200)))";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test1"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        // Verify the main query expression exists
        assert!(query.expr.is_some());

        // Function to recursively find subqueries in an expression
        fn find_subqueries(expr: &Expr, count: &mut usize) {
            // Check if this expression has a subquery
            if expr.subquery.is_some() {
                *count += 1;
                // Check if the subquery has an expression
                if let Some(subquery) = &expr.subquery {
                    if let Some(subquery_expr) = &subquery.expr {
                        // Continue searching in the subquery's expression
                        find_subqueries(subquery_expr, count);
                    }
                }
            }

            // Check the left branch
            if let Some(left) = &expr.left {
                find_subqueries(left, count);
            }

            // Check the right branch
            if let Some(right) = &expr.right {
                find_subqueries(right, count);
            }
        }

        // Count the number of subqueries
        let mut subquery_count = 0;
        find_subqueries(&query.expr.unwrap(), &mut subquery_count);

        // We expect 3 levels of nested subqueries
        assert_eq!(subquery_count, 3);
    }

    #[test]
    fn root_with_alias() {
        let query = "select name from /test as test_alias";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, Some(String::from("test_alias")))
            ),]
        );
    }

    #[test]
    fn root_and_field_with_alias() {
        let query = "select test_alias.name from /test as test_alias";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![Expr::field_with_root_alias(Field::Name, Some(String::from("test_alias")))]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, false, None, None, None, Bfs, false, Some(String::from("test_alias")))
            ),]
        );
    }

    #[test]
    fn broken_root_path() {
        // Comma immediately after FROM means no valid root path provided
        // Parser should fall back to default root "."
        let query = "select name from , where size > 0";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(String::from("."), RootOptions::new())]
        );
    }

    #[test]
    fn parse_root_options_fails_on_incomplete_option() {
        // "mindepth" requires a number, omitting it must produce an error
        let query = "select name from /test mindepth";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }
}

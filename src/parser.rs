//! Handles the parsing of the query string

use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

use directories::UserDirs;

use crate::expr::Expr;
use crate::field::Field;
use crate::function::Function;
use crate::lexer::Lexem;
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

pub struct Parser {
    lexems: Vec<Lexem>,
    index: usize,
    roots_parsed: bool,
    where_parsed: bool,
}

impl Parser {
    pub fn new() -> Parser {
        Parser {
            lexems: vec![],
            index: 0,
            roots_parsed: false,
            where_parsed: false,
        }
    }

    pub fn parse(&mut self, query: Vec<String>, debug: bool) -> Result<Query, String> {
        let mut lexer = Lexer::new(query);
        while let Some(lexem) = lexer.next_lexem() {
            match lexem {
                Lexem::String(s) if s.is_empty() => {}
                _ => self.lexems.push(lexem) 
            }            
        }

        if debug {
            dbg!(&self.lexems);
        }

        let fields = self.parse_fields()?;
        let mut roots = self.parse_roots();
        let root_options = self.parse_root_options();
        self.roots_parsed = true;
        let expr = self.parse_where()?;
        self.where_parsed = true;
        let grouping_fields = self.parse_group_by()?;
        let (ordering_fields, ordering_asc) = self.parse_order_by(&fields)?;
        let mut limit = self.parse_limit()?;
        let output_format = self.parse_output_format()?;

        if roots.is_empty() {
            roots = self.parse_roots();
        }

        if roots.is_empty() {
            roots.push(Root::default(root_options));
        }

        if self.there_are_remaining_lexems() {
            if debug {
                dbg!(&fields);
                dbg!(&roots);
            }

            return Err(String::from(
                "Could not parse tokens at the end of the query",
            ));
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
            grouping_fields: Rc::new(grouping_fields),
            ordering_fields: Rc::new(ordering_fields),
            ordering_asc: Rc::new(ordering_asc),
            limit,
            output_format,
        })
    }

    fn parse_fields(&mut self) -> Result<Vec<Expr>, String> {
        let mut fields = vec![];

        loop {
            let lexem = self.next_lexem();
            match lexem {
                Some(Lexem::Comma) => {
                    // skip
                }
                Some(Lexem::String(ref s))
                | Some(Lexem::RawString(ref s))
                | Some(Lexem::ArithmeticOperator(ref s)) => {
                    if s.to_ascii_lowercase() != "select" {
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
                                if let Some(Lexem::By) = self.next_lexem() {
                                    self.drop_lexem();
                                    self.drop_lexem();
                                    break;
                                } else {
                                    self.drop_lexem();
                                }
                            }

                            self.drop_lexem();

                            if Self::is_root_option_keyword(s) {
                                break;
                            }

                            if let Ok(Some(field)) = self.parse_expr() {
                                fields.push(field);
                            }
                        }
                    }
                }
                Some(Lexem::Open) | Some(Lexem::CurlyOpen) => {
                    self.drop_lexem();
                    if let Ok(Some(field)) = self.parse_expr() {
                        fields.push(field);
                    }
                }
                _ => {
                    self.drop_lexem();
                    break;
                }
            }
        }

        if fields.is_empty() {
            return Err(String::from("Error parsing fields, no selector found"));
        }

        Ok(fields)
    }

    fn parse_roots(&mut self) -> Vec<Root> {
        enum RootParsingMode {
            Unknown,
            From,
            Root,
            Comma,
        }

        let mut roots: Vec<Root> = Vec::new();
        let mut mode = RootParsingMode::Unknown;

        if let Some(ref lexem) = self.next_lexem() {
            match lexem {
                &Lexem::From => {
                    mode = RootParsingMode::From;
                }
                _ => {
                    self.drop_lexem();
                }
            }
        }

        if let RootParsingMode::From = mode {
            let mut path: String = String::from("");
            let mut root_options = RootOptions::new();

            loop {
                let lexem = self.next_lexem();
                match lexem {
                    Some(ref lexem) => match lexem {
                        Lexem::String(s) | Lexem::RawString(s) => match mode {
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
                                self.drop_lexem();
                                match self.parse_root_options() {
                                    Some(options) => root_options = options,
                                    None => {
                                        roots.push(Root::new(path, RootOptions::new()));
                                        break
                                    }
                                }
                            }
                            _ => {}
                        },
                        Lexem::Comma => {
                            if !path.is_empty() {
                                roots.push(Root::new(path, root_options));

                                path = String::from("");
                                root_options = RootOptions::new();

                                mode = RootParsingMode::Comma;
                            } else {
                                self.drop_lexem();
                                break;
                            }
                        }
                        _ => {
                            if !path.is_empty() {
                                roots.push(Root::new(path, root_options));
                            }

                            self.drop_lexem();
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

        roots
    }

    fn parse_root_options(&mut self) -> Option<RootOptions> {
        enum RootParsingMode {
            Unknown,
            Options,
            MinDepth,
            Depth,
        }

        let mut mode = RootParsingMode::Unknown;

        let mut min_depth: u32 = 0;
        let mut max_depth: u32 = 0;
        let mut archives = false;
        let mut symlinks = false;
        let mut gitignore = None;
        let mut hgignore = None;
        let mut dockerignore = None;
        let mut traversal = Bfs;
        let mut regexp = false;

        loop {
            let lexem = self.next_lexem();
            match lexem {
                Some(ref lexem) => match lexem {
                    Lexem::String(s) | Lexem::RawString(s) => match mode {
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
                            } else {
                                self.drop_lexem();
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
                                    self.drop_lexem();
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
                                    self.drop_lexem();
                                    break;
                                }
                            }
                        }
                    },
                    Lexem::Operator(s) if s.eq("rx") => {
                        regexp = true;
                        mode = RootParsingMode::Options;
                    }
                    _ => {
                        self.drop_lexem();
                        break;
                    }
                },
                None => {
                    break;
                }
            }
        }

        match mode {
            RootParsingMode::Unknown => None,
            _ => Some(RootOptions {
                min_depth,
                max_depth,
                archives,
                symlinks,
                gitignore,
                hgignore,
                dockerignore,
                traversal,
                regexp,
            }),
        }
    }

    fn is_root_option_keyword(s: &str) -> bool {
        s.to_ascii_lowercase() == "depth"
            || s.to_ascii_lowercase() == "mindepth"
            || s.to_ascii_lowercase() == "maxdepth"
            || s.starts_with("arc")
            || s.starts_with("sym")
            || s.starts_with("git")
            || s.starts_with("hg")
            || s.starts_with("dock")
            || s.starts_with("nogit")
            || s.starts_with("nohg")
            || s.starts_with("nodock")
            || s == "bfs"
            || s == "dfs"
            || s.starts_with("regex")
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
        match self.next_lexem() {
            Some(Lexem::Where) => self.parse_expr(),
            _ => {
                self.drop_lexem();
                Ok(None)
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_and()?;

        let mut right: Option<Expr> = None;
        loop {
            let lexem = self.next_lexem();
            match lexem {
                Some(Lexem::Or) => {
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
                    self.drop_lexem();

                    return match right {
                        Some(right) => {
                            Ok(Some(Expr::logical_op(left.unwrap(), LogicalOp::Or, right)))
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
            let lexem = self.next_lexem();
            match lexem {
                Some(Lexem::And) => {
                    let expr = self.parse_cond()?;
                    right = match right {
                        Some(right) => Some(Expr::logical_op(right, LogicalOp::And, expr.unwrap())),
                        None => expr,
                    };
                }
                _ => {
                    self.drop_lexem();

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
            if let Some(Lexem::Not) = self.next_lexem() {
                negate = !negate;
            } else {
                self.drop_lexem();
                break;
            }
        }

        let left = self.parse_add_sub()?;

        let mut not = false;

        let lexem = self.next_lexem();
        match lexem {
            Some(Lexem::Not) => {
                not = true;
            }
            _ => {
                self.drop_lexem();
            }
        };

        let lexem = self.next_lexem();
        let mut result = match lexem {
            Some(Lexem::Operator(s)) if s.as_str() == "between" => {
                let left_between = self.parse_add_sub()?;

                let and_lexem = self.next_lexem();
                if and_lexem.is_none() || and_lexem.unwrap() != Lexem::And {
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
            Some(Lexem::Operator(s)) => {
                let right = self.parse_add_sub()?;
                let op = Op::from_with_not(s, not);
                Ok(Some(Expr::op(left.unwrap(), op.unwrap(), right.unwrap())))
            }
            _ => {
                self.drop_lexem();
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
            let lexem = self.next_lexem();
            if let Some(Lexem::ArithmeticOperator(s)) = lexem {
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
                        self.drop_lexem();

                        return Ok(left);
                    }
                }
            } else {
                self.drop_lexem();

                return Ok(left);
            }
        }
    }

    fn parse_mul_div(&mut self) -> Result<Option<Expr>, String> {
        let mut left = self.parse_paren()?;

        let mut op = None;
        loop {
            let lexem = self.next_lexem();
            if let Some(Lexem::ArithmeticOperator(s)) = lexem {
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
                        self.drop_lexem();

                        return Ok(left);
                    }
                }
            } else {
                self.drop_lexem();

                return Ok(left);
            }
        }
    }

    fn parse_paren(&mut self) -> Result<Option<Expr>, String> {
        match self.next_lexem() {
            Some(Lexem::Open) => {
                let result = self.parse_expr();
                if let Some(Lexem::Close) = self.next_lexem() {
                    result
                } else {
                    Err("Unmatched parenthesis".to_string())
                }
            }
            Some(Lexem::CurlyOpen) => {
                let result = self.parse_expr();
                if let Some(Lexem::CurlyClose) = self.next_lexem() {
                    result
                } else {
                    Err("Unmatched parenthesis".to_string())
                }
            }
            _ => {
                self.drop_lexem();
                self.parse_func_scalar()
            }
        }
    }

    fn parse_func_scalar(&mut self) -> Result<Option<Expr>, String> {
        let mut lexem = self.next_lexem();
        let mut minus = false;

        if let Some(Lexem::ArithmeticOperator(ref s)) = lexem {
            if s == "-" {
                minus = true;
                lexem = self.next_lexem();
            } else if s == "+" {
                // nop
            } else {
                self.drop_lexem();
            }
        }

        match lexem {
            Some(Lexem::String(ref s)) | Some(Lexem::RawString(ref s)) => {
                if let Ok(field) = Field::from_str(s) {
                    let mut expr = Expr::field(field);
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
        if let Some(lexem) = self.next_lexem() {
            if lexem != Lexem::Open && lexem != Lexem::CurlyOpen {
                if is_boolean_function {
                    return Ok(function_expr);
                }

                return Err("Error in function expression".to_string());
            }

            if lexem == Lexem::CurlyOpen {
                curly_mode = true;
            }
        }

        if let Ok(Some(function_arg)) = self.parse_expr() {
            function_expr.left = Some(Box::from(function_arg));
        } else {
            return Ok(function_expr);
        }

        let mut args = vec![];

        loop {
            match self.next_lexem() {
                Some(Lexem::Comma) => match self.parse_expr() {
                    Ok(Some(expr)) => args.push(expr),
                    _ => {
                        return Err("Error in function expression".to_string());
                    }
                },
                Some(lexem)
                    if (lexem == Lexem::Close && !curly_mode)
                        || (lexem == Lexem::CurlyClose && curly_mode) =>
                {
                    function_expr.args = Some(args);
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

        if let Some(Lexem::RawString(s)) = self.next_lexem() {
            if s.to_lowercase() == "group" {
                if let Some(Lexem::By) = self.next_lexem() {
                    loop {
                        match self.next_lexem() {
                            Some(Lexem::Comma) => {}
                            Some(Lexem::RawString(_)) => {
                                self.drop_lexem();
                                let group_field = self.parse_expr().unwrap().unwrap();
                                group_by_fields.push(group_field);
                            }
                            _ => {
                                self.drop_lexem();
                                break;
                            }
                        }
                    }
                } else {
                    self.drop_lexem();
                }
            } else {
                self.drop_lexem();
            }            
        } else {
            self.drop_lexem();
        }

        Ok(group_by_fields)
    }

    fn parse_order_by(&mut self, fields: &[Expr]) -> Result<(Vec<Expr>, Vec<bool>), String> {
        let mut order_by_fields: Vec<Expr> = vec![];
        let mut order_by_directions: Vec<bool> = vec![];

        if let Some(Lexem::Order) = self.next_lexem() {
            if let Some(Lexem::By) = self.next_lexem() {
                loop {
                    match self.next_lexem() {
                        Some(Lexem::Comma) => {}
                        Some(Lexem::RawString(ref ordering_field)) => {
                            let actual_field = match ordering_field.parse::<usize>() {
                                Ok(idx) => fields[idx - 1].clone(),
                                _ => {
                                    self.drop_lexem();
                                    self.parse_expr().unwrap().unwrap()
                                }
                            };
                            order_by_fields.push(actual_field);
                            order_by_directions.push(true);
                        }
                        Some(Lexem::DescendingOrder) => {
                            let cnt = order_by_directions.len();
                            order_by_directions[cnt - 1] = false;
                        }
                        _ => {
                            self.drop_lexem();
                            break;
                        }
                    }
                }
            } else {
                self.drop_lexem();
            }
        } else {
            self.drop_lexem();
        }

        Ok((order_by_fields, order_by_directions))
    }

    fn parse_limit(&mut self) -> Result<u32, &str> {
        let lexem = self.next_lexem();
        match lexem {
            Some(Lexem::Limit) => {
                let lexem = self.next_lexem();
                match lexem {
                    Some(Lexem::RawString(s)) | Some(Lexem::String(s)) => {
                        if let Ok(limit) = s.parse() {
                            return Ok(limit);
                        } else {
                            return Err("Error parsing limit");
                        }
                    }
                    _ => {
                        self.drop_lexem();
                        return Err("Error parsing limit, limit value not found");
                    }
                }
            }
            _ => {
                self.drop_lexem();
            }
        }

        Ok(0)
    }

    fn parse_output_format(&mut self) -> Result<OutputFormat, &str> {
        let lexem = self.next_lexem();
        match lexem {
            Some(Lexem::Into) => {
                let lexem = self.next_lexem();
                match lexem {
                    Some(Lexem::RawString(s)) | Some(Lexem::String(s)) => {
                        return match OutputFormat::from(&s) {
                            Some(output_format) => Ok(output_format),
                            None => Err("Unknown output format"),
                        };
                    }
                    _ => {
                        self.drop_lexem();
                        return Err("Error parsing output format");
                    }
                }
            }
            _ => {
                self.drop_lexem();
            }
        }

        Ok(OutputFormat::Tabs)
    }

    fn there_are_remaining_lexems(&mut self) -> bool {
        let result = self.next_lexem().is_some();
        if result {
            self.drop_lexem();
        }

        result
    }

    fn next_lexem(&mut self) -> Option<Lexem> {
        let lexem = self.lexems.get(self.index);
        self.index += 1;

        lexem.cloned()
    }

    fn drop_lexem(&mut self) {
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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
                    RootOptions::from(0, 2, false, false, None, None, None, Bfs, false)
                ),
                Root::new(
                    String::from("/test2"),
                    RootOptions::from(0, 0, true, false, None, None, None, Bfs, false)
                ),
                Root::new(
                    String::from("/test3"),
                    RootOptions::from(0, 3, true, false, None, None, None, Bfs, false)
                ),
                Root::new(
                    String::from("/test4"),
                    RootOptions::from(0, 0, false, false, None, None, None, Bfs, false)
                ),
                Root::new(
                    String::from("/test5"),
                    RootOptions::from(0, 0, false, false, Some(true), None, None, Bfs, false)
                ),
                Root::new(
                    String::from("/test6"),
                    RootOptions::from(3, 0, false, false, None, None, None, Bfs, false)
                ),
                Root::new(
                    String::from("/test7"),
                    RootOptions::from(0, 0, true, false, None, None, None, Dfs, false)
                ),
                Root::new(
                    String::from("/test8"),
                    RootOptions::from(0, 0, false, false, None, None, None, Dfs, false)
                ),
            ]
        );

        let expr = Expr::logical_op(
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
            LogicalOp::Or,
            Expr::op(
                Expr::field(Field::Name),
                Op::Eq,
                Expr::value(String::from("xxx")),
            ),
        );

        assert_eq!(query.expr, Some(expr));
        assert_eq!(
            query.ordering_fields,
            Rc::new(vec![Expr::field(Field::Path), Expr::field(Field::Size)])
        );
        assert_eq!(query.ordering_asc, Rc::new(vec![true, false]));
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn query_with_not() {
        let query = "select name from /test where name not like '%.tmp'";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false)
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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false)
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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false);

        assert!(query.is_err());
    }

    #[test]
    fn path_with_spaces() {
        let query = "select name from '/opt/Some Cool Dir/Test This'";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/opt/Some Cool Dir/Test This"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false)
            ),]
        );
    }

    #[test]
    fn simple_boolean_syntax() {
        let query = "select name from /home/user where is_audio or is_video";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        let query2 = "select name from /home/user where is_audio = true or is_video = true";
        let mut p2 = Parser::new();
        let query2 = p2.parse(vec![query2.to_string()], false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn simple_boolean_function_syntax() {
        let query = "select name from /home/user where CONTAINS('foobar') or CONTAINS('bazz')";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        let query2 = "select name from /home/user where CONTAINS('foobar') = true or CONTAINS('bazz') = true";
        let mut p2 = Parser::new();
        let query2 = p2.parse(vec![query2.to_string()], false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn simple_function_without_args_syntax_in_where() {
        let query = "select name, caps from /home/user where HAS_CAPS()";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        let query2 = "select name, caps from /home/user where HAS_CAPS";
        let mut p2 = Parser::new();
        let query2 = p2.parse(vec![query2.to_string()], false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn simple_function_without_args_syntax() {
        let query = "select CURDATE()";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        let query2 = "select CURDATE";
        let mut p2 = Parser::new();
        let query2 = p2.parse(vec![query2.to_string()], false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn from_at_the_end_of_the_query() {
        let query = "select name where not name like '%.tmp' from /test gitignore mindepth 2";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(2, 0, false, false, Some(true), None, None, Bfs, false)
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
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(String::from("."), RootOptions::new()),]
        );
    }

    #[test]
    fn query_with_implicit_root_and_root_options() {
        let query = "select name, size depth 2";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("."),
                RootOptions::from(0, 2, false, false, None, None, None, Bfs, false)
            ),]
        );
    }

    #[test]
    fn use_curly_braces() {
        let query = "select name, (1 + 2) from /home/user limit 1";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        let query2 = "select name, {1 + 2} from /home/user limit 1";
        let mut p2 = Parser::new();
        let query2 = p2.parse(vec![query2.to_string()], false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_group_by() {
        let query = "select AVG(size) from /test group by mime";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

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
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false)
            ),]
        );

        assert_eq!(
            query.grouping_fields,
            Rc::new(vec![Expr::field(Field::Mime)])
        );
    }

    #[test]
    fn query_with_between() {
        let query = "select name, size from /test where size between 5mb and 6mb";
        let mut p = Parser::new();
        let query = p.parse(vec![query.to_string()], false).unwrap();

        let query2 = "select name, size from /test where size gte 5mb and size lte 6mb";
        let mut p2 = Parser::new();
        let query2 = p2.parse(vec![query2.to_string()], false).unwrap();

        assert_eq!(query.expr, query2.expr);
    }
}

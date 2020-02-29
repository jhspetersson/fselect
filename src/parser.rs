use std::rc::Rc;
use std::str::FromStr;

use crate::expr::Expr;
use crate::lexer::Lexer;
use crate::lexer::Lexem;
use crate::field::Field;
use crate::function::Function;
use crate::operators::ArithmeticOp;
use crate::operators::LogicalOp;
use crate::operators::Op;
use crate::query::OutputFormat;
use crate::query::Query;
use crate::query::Root;
use crate::query::TraversalMode::{Bfs, Dfs};

pub struct Parser {
    lexems: Vec<Lexem>,
    index: usize,
}

impl Parser {
    pub fn new() -> Parser {
        Parser {
            lexems: vec![],
            index: 0
        }
    }

    pub fn parse(&mut self, query: &str) -> Result<Query, String> {
        let mut lexer = Lexer::new(query);
        while let Some(lexem) = lexer.next_lexem() {
            self.lexems.push(lexem);
        }

        let fields = self.parse_fields()?;
        let roots = self.parse_roots();
        let expr = self.parse_where()?;
        let (ordering_fields, ordering_asc) = self.parse_order_by(&fields)?;
        let limit = self.parse_limit()?;
        let output_format = self.parse_output_format()?;

        if self.is_something_left() {
            return Err(String::from("Could not parse tokens at the end of the query"));
        }

        Ok(Query {
            fields,
            roots,
            expr,
            ordering_fields,
            ordering_asc: Rc::new(ordering_asc),
            limit,
            output_format,
        })
    }

    fn parse_fields(&mut self) -> Result<Vec<Expr>, String> {
        let mut fields = vec![];

        loop {
            let lexem = self.get_lexem();
            match lexem {
                Some(Lexem::Comma) => {
                    // skip
                },
                Some(Lexem::String(ref s)) | Some(Lexem::RawString(ref s)) | Some(Lexem::ArithmeticOperator(ref s)) => {
                    if s.to_ascii_lowercase() != "select" {
                        if s == "*" && fields.is_empty() {
                            #[cfg(unix)]
                                {
                                    fields.push(Expr::field(Field::Mode));
                                    fields.push(Expr::field(Field::User));
                                    fields.push(Expr::field(Field::Group));
                                }

                            fields.push(Expr::field(Field::Size));
                            fields.push(Expr::field(Field::Path));
                        } else {
                            self.drop_lexem();
                            if let Ok(Some(field)) = self.parse_expr() {
                                fields.push(field);
                            }
                        }
                    }
                },
                _ => {
                    self.drop_lexem();
                    break;
                }
            }
        }

        if fields.is_empty() {
            return Err(String::from("Error parsing fields, no selector found"))
        }

        Ok(fields)
    }

    fn parse_roots(&mut self) -> Vec<Root> {
        enum RootParsingMode {
            Unknown, From, Root, MinDepth, Depth, Options, Comma
        }

        let mut roots: Vec<Root> = Vec::new();
        let mut mode = RootParsingMode::Unknown;

        let lexem = self.get_lexem();
        match lexem {
            Some(ref lexem) => {
                match lexem {
                    &Lexem::From => {
                        mode = RootParsingMode::From;
                    },
                    _ => {
                        self.drop_lexem();
                        roots.push(Root::default());
                    }
                }
            },
            None => {
                roots.push(Root::default());
            }
        }

        if let RootParsingMode::From = mode {
            let mut path: String = String::from("");
            let mut min_depth: u32 = 0;
            let mut depth: u32 = 0;
            let mut archives = false;
            let mut symlinks = false;
            let mut gitignore = false;
            let mut hgignore = false;
            let mut traversal = Bfs;

            loop {
                let lexem = self.get_lexem();
                match lexem {
                    Some(ref lexem) => {
                        match lexem {
                            Lexem::String(ref s) | Lexem::RawString(ref s) => {
                                match mode {
                                    RootParsingMode::From | RootParsingMode::Comma => {
                                        path = s.to_string();
                                        mode = RootParsingMode::Root;
                                    },
                                    RootParsingMode::Root | RootParsingMode::Options => {
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
                                            gitignore = true;
                                            mode = RootParsingMode::Options;
                                        } else if s.starts_with("hg") {
                                            hgignore = true;
                                            mode = RootParsingMode::Options;
                                        } else if s == "bfs" {
                                            traversal = Bfs;
                                            mode = RootParsingMode::Options;
                                        } else if s == "dfs" {
                                            traversal = Dfs;
                                            mode = RootParsingMode::Options;
                                        } else {
                                            self.drop_lexem();
                                            break;
                                        }
                                    },
                                    RootParsingMode::MinDepth => {
                                        let d: Result<u32, _> = s.parse();
                                        match d {
                                            Ok(d) => {
                                                min_depth = d;
                                                mode = RootParsingMode::Options;
                                            },
                                            _ => {
                                                self.drop_lexem();
                                                break;
                                            }
                                        }
                                    },
                                    RootParsingMode::Depth => {
                                        let d: Result<u32, _> = s.parse();
                                        match d {
                                            Ok(d) => {
                                                depth = d;
                                                mode = RootParsingMode::Options;
                                            },
                                            _ => {
                                                self.drop_lexem();
                                                break;
                                            }
                                        }
                                    },
                                    _ => { }
                                }
                            },
                            Lexem::Comma => {
                                if path.len() > 0 {
                                    roots.push(Root::new(path, min_depth, depth, archives, symlinks, gitignore, hgignore, traversal));

                                    path = String::from("");
                                    min_depth = 0;
                                    depth = 0;
                                    archives = false;
                                    symlinks = false;
                                    gitignore = false;
                                    hgignore = false;
                                    traversal = Bfs;

                                    mode = RootParsingMode::Comma;
                                } else {
                                    self.drop_lexem();
                                    break;
                                }
                            },
                            _ => {
                                if path.len() > 0 {
                                    roots.push(Root::new(path, min_depth, depth, archives, symlinks, gitignore, hgignore, traversal));
                                }

                                self.drop_lexem();
                                break
                            }
                        }
                    },
                    None => {
                        if path.len() > 0 {
                            roots.push(Root::new(path, min_depth, depth, archives, symlinks, gitignore, hgignore, traversal));
                        }
                        break;
                    }
                }
            }
        }

        roots
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
        match self.get_lexem() {
            Some(Lexem::Where) => {
                self.parse_expr()
            },
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
            let lexem = self.get_lexem();
            match lexem {
                Some(Lexem::Or) => {
                    let expr = self.parse_and()?;
                    right = match right {
                        Some(right) => Some(Expr::logical_op(right, LogicalOp::Or, expr.clone().unwrap())),
                        None => expr
                    };
                },
                _ => {
                    self.drop_lexem();

                    return match right {
                        Some(right) => Ok(Some(Expr::logical_op(left.unwrap(), LogicalOp::Or, right))),
                        None => Ok(left)
                    }
                }
            }
        }
    }

    fn parse_and(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_cond()?;

        let mut right: Option<Expr> = None;
        loop {
            let lexem = self.get_lexem();
            match lexem {
                Some(Lexem::And) => {
                    let expr = self.parse_cond()?;
                    right = match right {
                        Some(right) => Some(Expr::logical_op(right, LogicalOp::And, expr.clone().unwrap())),
                        None => expr
                    };
                },
                _ => {
                    self.drop_lexem();

                    return match right {
                        Some(right) => Ok(Some(Expr::logical_op(left.unwrap(), LogicalOp::And, right))),
                        None => Ok(left)
                    }
                }
            }
        }
    }

    fn parse_cond(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_add_sub()?;

        let lexem = self.get_lexem();
        match lexem {
            Some(Lexem::Operator(s)) => {
                let right = self.parse_add_sub()?;
                let op = Op::from(s);
                Ok(Some(Expr::op(left.unwrap(), op.unwrap(), right.unwrap())))
            },
            _ => {
                self.drop_lexem();
                Ok(left)
            }
        }
    }

    fn parse_add_sub(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_mul_div()?;

        let mut right: Option<Expr> = None;
        let mut op = None;
        loop {
            let lexem = self.get_lexem();
            if let Some(Lexem::ArithmeticOperator(s)) = lexem {
                let new_op = ArithmeticOp::from(s);
                match new_op {
                    Some(ArithmeticOp::Add) | Some(ArithmeticOp::Subtract) => {
                        let expr = self.parse_mul_div()?;
                        op = new_op.clone();
                        right = match right {
                            Some(right) => Some(Expr::arithmetic_op(right, new_op.unwrap(), expr.clone().unwrap())),
                            None => expr
                        };
                    },
                    _ => {
                        self.drop_lexem();

                        return match right {
                            Some(right) => Ok(Some(Expr::arithmetic_op(left.unwrap(), op.unwrap(), right))),
                            None => Ok(left)
                        }
                    }
                }
            } else {
                self.drop_lexem();

                return match right {
                    Some(right) => Ok(Some(Expr::arithmetic_op(left.unwrap(), op.unwrap(), right))),
                    None => Ok(left)
                }
            }
        }
    }

    fn parse_mul_div(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_paren()?;

        let mut right: Option<Expr> = None;
        let mut op = None;
        loop {
            let lexem = self.get_lexem();
            if let Some(Lexem::ArithmeticOperator(s)) = lexem {
                let new_op = ArithmeticOp::from(s);
                match new_op {
                    Some(ArithmeticOp::Multiply) | Some(ArithmeticOp::Divide) => {
                        let expr = self.parse_paren()?;
                        op = new_op.clone();
                        right = match right {
                            Some(right) => Some(Expr::arithmetic_op(right, new_op.unwrap(), expr.clone().unwrap())),
                            None => expr
                        };
                    },
                    _ => {
                        self.drop_lexem();

                        return match right {
                            Some(right) => Ok(Some(Expr::arithmetic_op(left.unwrap(), op.unwrap(), right))),
                            None => Ok(left)
                        }
                    }
                }
            } else {
                self.drop_lexem();

                return match right {
                    Some(right) => Ok(Some(Expr::arithmetic_op(left.unwrap(), op.unwrap(), right))),
                    None => Ok(left)
                }
            }
        }
    }

    fn parse_paren(&mut self) -> Result<Option<Expr>, String> {
        if let Some(Lexem::Open) = self.get_lexem() {
            let result = self.parse_expr();
            if let Some(Lexem::Close) = self.get_lexem() {
                result
            } else {
                Err("Unmatched parenthesis".to_string())
            }
        } else {
            self.drop_lexem();
            self.parse_func_scalar()
        }
    }

    fn parse_func_scalar(&mut self) -> Result<Option<Expr>, String> {
        let mut lexem = self.get_lexem();
        let mut minus = false;

        if let Some(Lexem::ArithmeticOperator(ref s)) = lexem {
            if s == "-" {
                minus = true;
                lexem = self.get_lexem();
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
                        },
                        Err(err) => return Err(err)
                    }
                }

                let mut expr = Expr::value(s.to_string());
                expr.minus = minus;

                Ok(Some(expr))
            }
            _ => {
                Err("Error parsing expression, expecting string".to_string())
            }
        }
    }

    fn parse_function(&mut self, function: Function) -> Result<Expr, String> {
        let mut function_expr = Expr::function(function);

        if let Some(lexem) = self.get_lexem() {
            if lexem != Lexem::Open {
                return Err("Error in function expression".to_string());
            }
        }

        if let Ok(Some(function_arg)) = self.parse_expr() {
            function_expr.left = Some(Box::from(function_arg));
        }

        // TODO: try find a comma, then parse another expr, repeat until it's a close paren

        let mut args = vec![];

        loop {
            match self.get_lexem() {
                Some(lexem) if lexem == Lexem::Comma => {
                    match self.parse_expr() {
                        Ok(Some(expr)) => args.push(expr),
                        _ => {
                            return Err("Error in function expression".to_string());
                        }
                    }
                },
                Some(lexem) if lexem == Lexem::Close => {
                    function_expr.args = Some(args);
                    return Ok(function_expr);
                },
                _ => {
                    return Err("Error in function expression".to_string());
                }
            }
        }
    }

    fn parse_order_by(&mut self, fields: &Vec<Expr>) -> Result<(Vec<Expr>, Vec<bool>), String> {
        let mut order_by_fields: Vec<Expr> = vec![];
        let mut order_by_directions: Vec<bool> = vec![];

        if let Some(Lexem::Order) = self.get_lexem() {
            if let Some(Lexem::By) = self.get_lexem() {
                loop {
                    match self.get_lexem() {
                        Some(Lexem::Comma) => {},
                        Some(Lexem::RawString(ref ordering_field)) => {
                            let actual_field = match ordering_field.parse::<usize>() {
                                Ok(idx) => fields[idx - 1].clone(),
                                _ => Expr::field(Field::from_str(ordering_field)?),
                            };
                            order_by_fields.push(actual_field.clone());
                            order_by_directions.push(true);
                        },
                        Some(Lexem::DescendingOrder) => {
                            let cnt = order_by_directions.len();
                            order_by_directions[cnt - 1] = false;
                        },
                        _ => {
                            self.drop_lexem();
                            break;
                        },
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
        let lexem = self.get_lexem();
        match lexem {
            Some(Lexem::Limit) => {
                let lexem = self.get_lexem();
                match lexem {
                    Some(Lexem::RawString(s)) | Some(Lexem::String(s)) => {
                        if let Ok(limit) = s.parse() {
                            return Ok(limit);
                        } else {
                            return Err("Error parsing limit");
                        }
                    },
                    _ => {
                        self.drop_lexem();
                        return Err("Error parsing limit, limit value not found");
                    }
                }
            },
            _ => {
                self.drop_lexem();
            }
        }

        Ok(0)
    }

    fn parse_output_format(&mut self) -> Result<OutputFormat, &str>{
        let lexem = self.get_lexem();
        match lexem {
            Some(Lexem::Into) => {
                let lexem = self.get_lexem();
                match lexem {
                    Some(Lexem::RawString(s)) | Some(Lexem::String(s)) => {
                        return match OutputFormat::from(&s) {
                            Some(output_format) => Ok(output_format),
                            None => Err("Unknown output format")
                        };
                    },
                    _ => {
                        self.drop_lexem();
                        return Err("Error parsing output format");
                    }
                }
            },
            _ => {
                self.drop_lexem();
            }
        }

        Ok(OutputFormat::Tabs)
    }

    fn is_something_left(&mut self) -> bool {
        self.get_lexem().is_some()
    }

    fn get_lexem(&mut self) -> Option<Lexem> {
        let lexem = self.lexems.get(self.index );
        self.index += 1;

        match lexem {
            Some(lexem) => Some(lexem.clone()),
            None => None
        }
    }

    fn drop_lexem(&mut self) {
        self.index -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_query() {
        let query = "select name, path ,size , fsize from /";
        let mut p = Parser::new();
        let query = p.parse(&query).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name),
                                      Expr::field(Field::Path),
                                      Expr::field(Field::Size),
                                      Expr::field(Field::FormattedSize),
        ]);
    }

    #[test]
    fn query() {
        let query = "select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' gitignore , /test6 mindepth 3, /test7 archives DFS, /test8 dfs where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 2, size desc limit 50";
        let mut p = Parser::new();
        let query = p.parse(&query).unwrap();

        assert_eq!(query.fields, vec![Expr::field(Field::Name),
                                      Expr::field(Field::Path),
                                      Expr::field(Field::Size),
                                      Expr::field(Field::FormattedSize)
        ]);

        assert_eq!(query.roots, vec![
            Root::new(String::from("/test"), 0, 2, false, false, false, false, Bfs),
            Root::new(String::from("/test2"), 0, 0, true, false, false, false, Bfs),
            Root::new(String::from("/test3"), 0, 3, true, false, false, false, Bfs),
            Root::new(String::from("/test4"), 0, 0, false, false, false, false, Bfs),
            Root::new(String::from("/test5"), 0, 0, false, false, true, false, Bfs),
            Root::new(String::from("/test6"), 3, 0, false, false, false, false, Bfs),
            Root::new(String::from("/test7"), 0, 0, true, false, false, false, Dfs),
            Root::new(String::from("/test8"), 0, 0, false, false, false, false, Dfs),
        ]);

        let expr = Expr::logical_op(
                Expr::logical_op(
                    Expr::op(Expr::field(Field::Name), Op::Ne, Expr::value(String::from("123"))),
                    LogicalOp::And,
                    Expr::logical_op(
                        Expr::op(Expr::field(Field::Size), Op::Gt, Expr::value(String::from("456"))),
                        LogicalOp::Or,
                        Expr::op(Expr::field(Field::FormattedSize), Op::Lte, Expr::value(String::from("758"))),
                    ),
                ),
            LogicalOp::Or,
                Expr::op(Expr::field(Field::Name), Op::Eq, Expr::value(String::from("xxx")))
        );

        assert_eq!(query.expr, Some(expr));
        assert_eq!(query.ordering_fields, vec![Expr::field(Field::Path), Expr::field(Field::Size)]);
        assert_eq!(query.ordering_asc, Rc::new(vec![true, false]));
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn broken_query() {
        let query = "select name, path ,size , fsize from / where name != 'foobar' order by size desc limit 10 into csv this is unexpected";
        let mut p = Parser::new();
        let query = p.parse(&query);

        assert!(query.is_err());
    }
}

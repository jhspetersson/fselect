extern crate regex;
extern crate serde;

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::ops::Index;
use std::rc::Rc;
use std::str::FromStr;

use chrono::DateTime;
use chrono::Local;
use regex::Captures;
use regex::Regex;

use lexer::Lexer;
use lexer::Lexem;
use field::Field;
use util::parse_datetime;

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

    fn parse_fields(&mut self) -> Result<Vec<ColumnExpr>, String> {
        let mut fields = vec![];
        let mut skip = 0;

        let lexems = &self.lexems;
        for lexem in lexems {
            match lexem {
                Lexem::Comma => {
                    skip += 1;
                },
                Lexem::String(ref s) | Lexem::RawString(ref s) => {
                    if s.to_ascii_lowercase() != "select" {
                        if s == "*" {
                            #[cfg(unix)]
                            {
                                fields.push(ColumnExpr::field(Field::Mode));
                                fields.push(ColumnExpr::field(Field::User));
                                fields.push(ColumnExpr::field(Field::Group));
                            }

                            fields.push(ColumnExpr::field(Field::Size));
                            fields.push(ColumnExpr::field(Field::Path));
                        } else {
                            let field = match Field::from_str(s) {
                                Ok(field) => ColumnExpr::field(field),
                                _ => ColumnExpr::value(s.to_string())
                            };
                            fields.push(field);
                        }
                    }

                    skip += 1;
                },
                _ => break
            }
        }

        self.index = skip;

        if fields.is_empty() {
            return Err(String::from("Error parsing fields, no selector found"))
        }

        Ok(fields)
    }

    fn parse_roots(&mut self) -> Vec<Root> {
        enum RootParsingMode {
            Unknown, From, Root, Depth, Options, Comma
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
            let mut depth: u32 = 0;
            let mut archives = false;
            let mut symlinks = false;
            let mut gitignore = false;

            loop {
                let lexem = self.get_lexem();
                match lexem {
                    Some(ref lexem) => {
                        match lexem {
                            &Lexem::String(ref s) | &Lexem::RawString(ref s) => {
                                match mode {
                                    RootParsingMode::From | RootParsingMode::Comma => {
                                        path = s.to_string();
                                        mode = RootParsingMode::Root;
                                    },
                                    RootParsingMode::Root | RootParsingMode::Options => {
                                        if s.to_ascii_lowercase() == "depth" {
                                            mode = RootParsingMode::Depth;
                                        } else if s.to_ascii_lowercase().starts_with("arc") {
                                            archives = true;
                                            mode = RootParsingMode::Options;
                                        } else if s.to_ascii_lowercase().starts_with("symlink") {
                                            symlinks = true;
                                            mode = RootParsingMode::Options;
                                        } else if s.to_ascii_lowercase().starts_with("gitignore") {
                                            gitignore = true;
                                            mode = RootParsingMode::Options;
                                        } else {
                                            self.drop_lexem();
                                            break;
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
                            &Lexem::Comma => {
                                if path.len() > 0 {
                                    roots.push(Root::new(path, depth, archives, symlinks, gitignore));

                                    path = String::from("");
                                    depth = 0;
                                    archives = false;
                                    symlinks = false;
                                    gitignore = false;

                                    mode = RootParsingMode::Comma;
                                } else {
                                    self.drop_lexem();
                                    break;
                                }
                            },
                            _ => {
                                if path.len() > 0 {
                                    roots.push(Root::new(path, depth, archives, symlinks, gitignore));
                                }

                                self.drop_lexem();
                                break
                            }
                        }
                    },
                    None => {
                        if path.len() > 0 {
                            roots.push(Root::new(path, depth, archives, symlinks, gitignore));
                        }
                        break;
                    }
                }
            }
        }

        roots
    }

    fn parse_where(&mut self) -> Result<Option<Box<Expr>>, String> {
        let lexem = self.get_lexem();

        match lexem {
            Some(Lexem::Where) => {
                self.parse_or()
            },
            _ => {
                self.drop_lexem();
                Ok(None)
            }
        }
    }

    fn parse_or(&mut self) -> Result<Option<Box<Expr>>, String> {
        let node = self.parse_and();
        match node {
            Ok(mut node) => {
                loop {
                    let lexem = self.get_lexem();
                    if let Some(Lexem::Or) = lexem {
                        match self.parse_and() {
                            Ok(and) => {
                                node = Some(Box::new(Expr::node(node, Some(LogicalOp::Or), and)));
                            },
                            Err(err) => {
                                return Err(err);
                            }
                        }
                    } else {
                        self.drop_lexem();
                        break;
                    }
                }

                Ok(node)
            },
            Err(err) => Err(err)
        }
    }

    fn parse_and(&mut self) -> Result<Option<Box<Expr>>, String> {
        let node = self.parse_cond();
        match node {
            Ok(mut node) => {
                loop {
                    let lexem = self.get_lexem();
                    if let Some(Lexem::And) = lexem {
                        match self.parse_cond() {
                            Ok(cond) => {
                                node = Some(Box::new(Expr::node(node, Some(LogicalOp::And), cond)));
                            },
                            Err(err) => {
                                return Err(err);
                            }
                        }
                    } else {
                        self.drop_lexem();
                        break;
                    }
                }

                Ok(node)
            },
            Err(err) => Err(err)
        }
    }

    fn parse_cond(&mut self) -> Result<Option<Box<Expr>>, String> {
        let lexem = self.get_lexem();

        match lexem {
            Some(Lexem::RawString(ref s)) => {

                let lexem2 = self.get_lexem();

                if let Some(Lexem::Operator(ref s2)) = lexem2 {

                    let lexem3 = self.get_lexem();

                    match lexem3 {
                        Some(Lexem::String(ref s3)) | Some(Lexem::RawString(ref s3)) => {
                            let op = Op::from(s2.to_string());
                            let mut expr: Expr;
                            let field;
                            match Field::from_str(s) {
                                Ok(field_) => field = field_,
                                Err(err) => return Err(err)
                            }
                            if let Some(Op::Rx) = op {
                                let regex;
                                match Regex::new(&s3) {
                                    Ok(regex_) => regex = regex_,
                                    _ => return Err("Error parsing regular expression".to_string())
                                }
                                expr = Expr::leaf_regex(field, op, s3.to_string(), regex);
                            } else if let Some(Op::Like) = op {
                                let pattern = convert_like_to_pattern(s3);
                                let regex;
                                match Regex::new(&pattern) {
                                    Ok(regex_) => regex = regex_,
                                    _ => return Err("Error parsing LIKE expression".to_string())
                                }

                                expr = Expr::leaf_regex(field, op, s3.to_string(), regex);
                            } else {
                                expr = match is_glob(s3) {
                                    true => {
                                        let pattern = convert_glob_to_pattern(s3);
                                        let regex;
                                        match Regex::new(&pattern) {
                                            Ok(regex_) => regex = regex_,
                                            _ => return Err("Error parsing glob pattern".to_string())
                                        }

                                        Expr::leaf_regex(field, op, s3.to_string(), regex)
                                    },
                                    false => Expr::leaf(field, op, s3.to_string())
                                };
                            };

                            let field = &Field::from_str(s)?;
                            if field.is_datetime_field() {
                                match parse_datetime(s3) {
                                    Ok((dt_from, dt_to)) => {
                                        expr.dt_from = Some(dt_from);
                                        expr.dt_to = Some(dt_to);
                                    },
                                    Err(err) => {
                                        return Err(err)
                                    }
                                }
                            }

                            Ok(Some(Box::new(expr)))
                        },
                        _ => Err("Error parsing condition, no operand found".to_string())
                    }
                } else {
                    Err("Error parsing condition, no operator found".to_string())
                }
            },
            Some(Lexem::Open) => {
                let expr_result = self.parse_or();
                let lexem4 = self.get_lexem();

                match lexem4 {
                    Some(Lexem::Close) => expr_result,
                    _ => Ok(None)
                }
            },
            _ => Ok(None)
        }
    }

    fn parse_order_by(&mut self, fields: &Vec<ColumnExpr>) -> Result<(Vec<ColumnExpr>, Vec<bool>), String> {
        let mut order_by_fields: Vec<ColumnExpr> = vec![];
        let mut order_by_directions: Vec<bool> = vec![];

        if let Some(Lexem::Order) = self.get_lexem() {
            if let Some(Lexem::By) = self.get_lexem() {
                loop {
                    use std::str::FromStr;
                    match self.get_lexem() {
                        Some(Lexem::Comma) => {},
                        Some(Lexem::RawString(ref ordering_field)) => {
                            let actual_field = match ordering_field.parse::<usize>() {
                                Ok(idx) => fields[idx - 1].clone(),
                                _ => ColumnExpr::field(Field::from_str(ordering_field)?),
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


    fn parse_limit<'a>(&mut self) -> Result<u32, &'a str> {
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

    fn parse_output_format<'a>(&mut self) -> Result<OutputFormat, &'a str>{
        let lexem = self.get_lexem();
        match lexem {
            Some(Lexem::Into) => {
                let lexem = self.get_lexem();
                match lexem {
                    Some(Lexem::RawString(s)) | Some(Lexem::String(s)) => {
                        let s = s.to_lowercase();
                        if s == "lines" {
                            return Ok(OutputFormat::Lines);
                        } else if s == "list" {
                            return Ok(OutputFormat::List);
                        } else if s == "csv" {
                            return Ok(OutputFormat::Csv);
                        } else if s == "json" {
                            return Ok(OutputFormat::Json);
                        } else if s == "tabs" {
                            return Ok(OutputFormat::Tabs);
                        } else {
                            return Err("Unknown output format");
                        }
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

fn is_glob(s: &str) -> bool {
    s.contains("*") || s.contains('?')
}

fn convert_glob_to_pattern(s: &str) -> String {
    let string = s.to_string();
    let regex = Regex::new("(\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$)").unwrap();
    let string = regex.replace_all(&string, |c: &Captures| {
        match c.index(0) {
            "." => "\\.",
            "*" => ".*",
            "?" => ".",
            "[" => "\\[",
            "]" => "\\]",
            "(" => "\\(",
            ")" => "\\)",
            "^" => "\\^",
            "$" => "\\$",
            _ => panic!("Error parsing glob")
        }.to_string()
    });

    format!("^(?i){}$", string)
}

fn convert_like_to_pattern(s: &str) -> String {
    let string = s.to_string();
    let regex = Regex::new("(%|_|\\?|\\.|\\*|\\[|\\]|\\(|\\)|\\^|\\$)").unwrap();
    let string = regex.replace_all(&string, |c: &Captures| {
        match c.index(0) {
            "%" => ".*",
            "_" => ".",
            "?" => ".?",
            "." => "\\.",
            "*" => "\\*",
            "[" => "\\[",
            "]" => "\\]",
            "(" => "\\(",
            ")" => "\\)",
            "^" => "\\^",
            "$" => "\\$",
            _ => panic!("Error parsing like expression")
        }.to_string()
    });

    format!("^(?i){}$", string)
}

#[derive(Debug, Clone)]
pub struct Query {
    pub fields: Vec<ColumnExpr>,
    pub roots: Vec<Root>,
    pub expr: Option<Box<Expr>>,
    pub ordering_fields: Vec<ColumnExpr>,
    pub ordering_asc: Rc<Vec<bool>>,
    pub limit: u32,
    pub output_format: OutputFormat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub path: String,
    pub depth: u32,
    pub archives: bool,
    pub symlinks: bool,
    pub gitignore: bool,
}

impl Root {
    fn new(path: String, depth: u32, archives: bool, symlinks: bool, gitignore: bool) -> Root {
        Root { path, depth, archives, symlinks, gitignore }
    }

    fn default() -> Root {
        Root { path: String::from("."), depth: 0, archives: false, symlinks: false, gitignore: false }
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
pub struct ColumnExpr {
    pub left: Option<Box<ColumnExpr>>,
    pub arithmetic_op: Option<ArithmeticOp>,
    pub right: Option<Box<ColumnExpr>>,
    pub field: Option<Field>,
    pub val: Option<String>,
}

impl ColumnExpr {
    pub fn field(field: Field) -> ColumnExpr {
        ColumnExpr {
            left: None,
            arithmetic_op: None,
            right: None,
            field: Some(field),
            val: None,
        }
    }

    fn value(value: String) -> ColumnExpr {
        ColumnExpr {
            left: None,
            arithmetic_op: None,
            right: None,
            field: None,
            val: Some(value),
        }
    }
}

impl Display for ColumnExpr {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        if let Some(ref field) = self.field {
            fmt.write_str(&field.to_string())?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub left: Option<Box<Expr>>,
    pub logical_op: Option<LogicalOp>,
    pub right: Option<Box<Expr>>,

    pub field: Option<ColumnExpr>,
    pub op: Option<Op>,
    pub val: Option<String>,
    pub regex: Option<Regex>,

    pub dt_from: Option<DateTime<Local>>,
    pub dt_to: Option<DateTime<Local>>,
}

impl Expr {
    fn node(left: Option<Box<Expr>>, logical_op: Option<LogicalOp>, right: Option<Box<Expr>>) -> Expr {
        Expr {
            left,
            logical_op,
            right,

            field: None,
            op: None,
            val: None,
            regex: None,

            dt_from: None,
            dt_to: None,
        }
    }

    fn leaf(field: Field, op: Option<Op>, val: String) -> Expr {
        Expr {
            left: None,
            logical_op: None,
            right: None,

            field: Some(ColumnExpr::field(field)),
            op,
            val: Some(val),
            regex: None,

            dt_from: None,
            dt_to: None,
        }
    }

    fn leaf_regex(field: Field, op: Option<Op>, val: String, regex: Regex) -> Expr {
        Expr {
            left: None,
            logical_op: None,
            right: None,

            field: Some(ColumnExpr::field(field)),
            op,
            val: Some(val),
            regex: Some(regex),

            dt_from: None,
            dt_to: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Eeq,
    Ene,
    Gt,
    Gte,
    Lt,
    Lte,
    Rx,
    Like,
}

impl Op {
    fn from(text: String) -> Option<Op> {
        match text.to_lowercase().as_str() {
            "=" | "==" | "eq" => Some(Op::Eq),
            "!=" | "<>" | "ne" => Some(Op::Ne),
            "===" => Some(Op::Eeq),
            "!==" => Some(Op::Ene),
            ">" | "gt" => Some(Op::Gt),
            ">=" | "gte" | "ge" => Some(Op::Gte),
            "<" | "lt" => Some(Op::Lt),
            "<=" | "lte" | "le" => Some(Op::Lte),
            "~=" | "=~" | "regexp" | "rx" => Some(Op::Rx),
            "like" => Some(Op::Like),
            _ => None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Divide,
    Multiply,
}

impl ArithmeticOp {
    fn from(text: String) -> Option<ArithmeticOp> {
        match text.to_lowercase().as_str() {
            "+" | "plus" => Some(ArithmeticOp::Add),
            "-" | "minus"  => Some(ArithmeticOp::Subtract),
            "mul" => Some(ArithmeticOp::Divide),
            "div" => Some(ArithmeticOp::Multiply),
            _ => None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Tabs, Lines, List, Csv, Json
}

#[cfg(test)]
impl PartialEq for Expr {
    fn eq(&self, other: &Expr) -> bool {
        self.left == other.left
            && self.logical_op == other.logical_op
            && self.right == other.right

            && self.field == other.field
            && self.op == other.op
            && self.val == other.val

            && match self.regex {
            Some(ref left_rx) => {
                match other.regex {
                    Some(ref right_rx) => {
                        left_rx.as_str() == right_rx.as_str()
                    },
                    _ => false
                }
            },
            None => {
                match other.regex {
                    None => true,
                    _ => false
                }
            }
        }

            && self.dt_from == other.dt_from
            && self.dt_to == other.dt_to
    }

    fn ne(&self, other: &Expr) -> bool {
        !self.eq(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query() {
        let query = "select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' gitignore where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 2, size desc limit 50";
        let mut p = Parser::new();
        let query = p.parse(&query).unwrap();

        assert_eq!(query.fields, vec![ColumnExpr::field(Field::Name), ColumnExpr::field(Field::Path), ColumnExpr::field(Field::Size), ColumnExpr::field(Field::FormattedSize)]);
        assert_eq!(query.roots, vec![
            Root::new(String::from("/test"), 2, false, false, false),
            Root::new(String::from("/test2"), 0, true, false, false),
            Root::new(String::from("/test3"), 3, true, false, false),
            Root::new(String::from("/test4"), 0, false, false, false),
            Root::new(String::from("/test5"), 0, false, false, true),
        ]);

        let expr = Expr::node(
            Some(Box::new(
                Expr::node(
                    Some(Box::new(Expr::leaf(Field::Name, Some(Op::Ne), String::from("123")))),
                    Some(LogicalOp::And),
                    Some(Box::new(Expr::node(
                        Some(Box::new(Expr::leaf(Field::Size, Some(Op::Gt), String::from("456")))),
                        Some(LogicalOp::Or),
                        Some(Box::new(Expr::leaf(Field::FormattedSize, Some(Op::Lte), String::from("758")))),
                    ))),
                )
            )),
            Some(LogicalOp::Or),
            Some(Box::new(
                Expr::leaf(Field::Name, Some(Op::Eq), String::from("xxx"))
            ))
        );

        assert_eq!(query.expr, Some(Box::new(expr)));
        assert_eq!(query.ordering_fields, vec![ColumnExpr::field(Field::Path), ColumnExpr::field(Field::Size)]);
        assert_eq!(query.ordering_asc, Rc::new(vec![true, false]));
        assert_eq!(query.limit, 50);
    }
}

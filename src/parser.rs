extern crate regex;

use std::ops::Index;
use std::rc::Rc;

use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::LocalResult;
use chrono::TimeZone;
use regex::Captures;
use regex::Regex;

use lexer::Lexer;
use lexer::Lexem;

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

    pub fn parse<'a>(&mut self, query: &str) -> Result<Query, &'a str> {
        let mut lexer = Lexer::new(query);
        while let Some(lexem) = lexer.next_lexem() {
            self.lexems.push(lexem);
        }

        let fields;
        match self.parse_fields() {
            Ok(fields_) => {
                fields = fields_;
            },
            Err(err) => {
                return Err(err);
            }
        }
        let roots = self.parse_roots();

        let expr;
        match self.parse_where() {
            Ok(expr_) => {
                expr = expr_;
            },
            Err(err) => {
                return Err(err);
            }
        }
        let (ordering_fields, ordering_asc) = self.parse_order_by(&fields);

        let limit;
        match self.parse_limit() {
            Ok(limit_) => {
                limit = limit_;
            }
            Err(err) => {
                return Err(err);
            }
        }

        let output_format;
        match self.parse_output_format() {
            Ok(format_) => {
                output_format = format_;
            },
            Err(err) => {
                return Err(err);
            }
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

    fn parse_fields<'a>(&mut self) -> Result<Vec<String>, &'a str> {
        let mut fields = vec![];
        let mut skip = 0;

        let lexems = &self.lexems;
        for lexem in lexems {
            match lexem {
                &Lexem::Field(ref s) => {
                    if s.to_ascii_lowercase() != "select" {
                        if s == "*" {
                            #[cfg(unix)]
                            {
                                fields.push("mode".to_string());
                                fields.push("user".to_string());
                                fields.push("group".to_string());
                            }

                            fields.push("size".to_string());
                            fields.push("path".to_string());
                        } else {
                            fields.push(s.to_string());
                        }
                    }

                    skip += 1;
                },
                &Lexem::Comma => {
                    skip += 1;
                },
                _ => break
            }
        }

        self.index = skip;

        if fields.is_empty() {
            return Err("Error parsing fields, no selector found")
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

            loop {
                let lexem = self.get_lexem();
                match lexem {
                    Some(ref lexem) => {
                        match lexem {
                            &Lexem::String(ref s) | &Lexem::Field(ref s) => {
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
                                    roots.push(Root::new(path, depth, archives, symlinks));

                                    path = String::from("");
                                    depth = 0;
                                    archives = false;
                                    symlinks = false;

                                    mode = RootParsingMode::Comma;
                                } else {
                                    self.drop_lexem();
                                    break;
                                }
                            },
                            _ => {
                                if path.len() > 0 {
                                    roots.push(Root::new(path, depth, archives, symlinks));
                                }

                                self.drop_lexem();
                                break
                            }
                        }
                    },
                    None => {
                        if path.len() > 0 {
                            roots.push(Root::new(path, depth, archives, symlinks));
                        }
                        break;
                    }
                }
            }
        }

        roots
    }

    fn parse_where<'a>(&mut self) -> Result<Option<Box<Expr>>, &'a str> {
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

    fn parse_or<'a>(&mut self) -> Result<Option<Box<Expr>>, &'a str> {
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

    fn parse_and<'a>(&mut self) -> Result<Option<Box<Expr>>, &'a str> {
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

    fn parse_cond<'a>(&mut self) -> Result<Option<Box<Expr>>, &'a str> {
        let lexem = self.get_lexem();

        match lexem {
            Some(Lexem::Field(ref s)) => {

                let lexem2 = self.get_lexem();

                if let Some(Lexem::Operator(ref s2)) = lexem2 {

                    let lexem3 = self.get_lexem();

                    match lexem3 {
                        Some(Lexem::String(ref s3)) | Some(Lexem::Field(ref s3)) => {
                            let op = Op::from(s2.to_string());
                            let mut expr: Expr;
                            if let Some(Op::Rx) = op {
                                let regex;
                                match Regex::new(&s3) {
                                    Ok(regex_) => regex = regex_,
                                    _ => return Err("Error parsing regular expression")
                                }
                                expr = Expr::leaf_regex(s.to_string(), op, s3.to_string(), regex);
                            } else if let Some(Op::Like) = op {
                                let pattern = convert_like_to_pattern(s3);
                                let regex;
                                match Regex::new(&pattern) {
                                    Ok(regex_) => regex = regex_,
                                    _ => return Err("Error parsing LIKE expression")
                                }

                                expr = Expr::leaf_regex(s.to_string(), op, s3.to_string(), regex);
                            } else {
                                expr = match is_glob(s3) {
                                    true => {
                                        let pattern = convert_glob_to_pattern(s3);
                                        let regex;
                                        match Regex::new(&pattern) {
                                            Ok(regex_) => regex = regex_,
                                            _ => return Err("Error parsing glob pattern")
                                        }

                                        Expr::leaf_regex(s.to_string(), op, s3.to_string(), regex)
                                    },
                                    false => Expr::leaf(s.to_string(), op, s3.to_string())
                                };
                            };

                            if is_datetime_field(s) {
                                match parse_datetime(s3) {
                                    Ok((dt_from, dt_to)) => {
                                        expr.dt_from = Some(dt_from);
                                        expr.dt_to = Some(dt_to);
                                    },
                                    Err(err) => {
                                        return Err(err);
                                    }
                                }
                            }

                            Ok(Some(Box::new(expr)))
                        },
                        _ => Err("Error parsing condition, no operand found")
                    }
                } else {
                    Err("Error parsing condition, no operator found")
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

    fn parse_order_by(&mut self, fields: &Vec<String>) -> (Vec<String>, Vec<bool>) {
        let mut order_by_fields: Vec<String> = vec![];
        let mut order_by_directions: Vec<bool> = vec![];

        match self.get_lexem() {
            Some(Lexem::OrderBy) => {
            },
            _ => {
                self.drop_lexem();
                return (order_by_fields, order_by_directions);
            },
        };

        loop {
            match self.get_lexem() {
                Some(Lexem::Comma) => {},
                Some(Lexem::Field(ref ordering_field)) => {
                    let actual_field = match ordering_field.parse::<usize>() {
                        Ok(idx) => &fields[idx - 1],
                        _ => ordering_field,
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

        (order_by_fields, order_by_directions)
    }


    fn parse_limit<'a>(&mut self) -> Result<u32, &'a str> {
        let lexem = self.get_lexem();
        match lexem {
            Some(Lexem::Limit) => {
                let lexem = self.get_lexem();
                match lexem {
                    Some(Lexem::Field(s)) | Some(Lexem::String(s)) => {
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
                    Some(Lexem::Field(s)) | Some(Lexem::String(s)) => {
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

fn is_datetime_field(s: &str) -> bool {
    s.to_ascii_lowercase() == "created" ||
        s.to_ascii_lowercase() == "accessed" ||
        s.to_ascii_lowercase() == "modified"
}

fn parse_datetime<'a>(s: &str) -> Result<(DateTime<Local>, DateTime<Local>), &'a str> {
    if s == "today" {
        let date = Local::now().date();
        let start = date.and_hms(0, 0, 0);
        let finish = date.and_hms(23, 59, 59);

        return Ok((start, finish));
    }

    if s == "yesterday" {
        let date = Local::now().date() - Duration::days(1);
        let start = date.and_hms(0, 0, 0);
        let finish = date.and_hms(23, 59, 59);

        return Ok((start, finish));
    }

    let regex = Regex::new("(\\d{4})-(\\d{1,2})-(\\d{1,2}) ?(\\d{1,2})?:?(\\d{1,2})?:?(\\d{1,2})?").unwrap();
    match regex.captures(s) {
        Some(cap) => {
            let year: i32 = cap[1].parse().unwrap();
            let month: u32 = cap[2].parse().unwrap();
            let day: u32 = cap[3].parse().unwrap();

            let hour_start: u32;
            let hour_finish: u32;
            match cap.get(4) {
                Some(val) => {
                    hour_start = val.as_str().parse().unwrap();
                    hour_finish = hour_start;
                },
                None => {
                    hour_start = 0;
                    hour_finish = 23;
                }
            }

            let min_start: u32;
            let min_finish: u32;
            match cap.get(5) {
                Some(val) => {
                    min_start = val.as_str().parse().unwrap();
                    min_finish = min_start;
                },
                None => {
                    min_start = 0;
                    min_finish = 23;
                }
            }

            let sec_start: u32;
            let sec_finish: u32;
            match cap.get(6) {
                Some(val) => {
                    sec_start = val.as_str().parse().unwrap();
                    sec_finish = min_start;
                },
                None => {
                    sec_start = 0;
                    sec_finish = 23;
                }
            }

            match Local.ymd_opt(year, month, day) {
                LocalResult::Single(date) => {
                    let start = date.and_hms(hour_start, min_start, sec_start);
                    let finish = date.and_hms(hour_finish, min_finish, sec_finish);

                    Ok((start, finish))
                },
                _ => Err("Error converting date/time to local")
            }
        },
        None => {
            Err("Error parsing date/time value")
        }
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub fields: Vec<String>,
    pub roots: Vec<Root>,
    pub expr: Option<Box<Expr>>,
    pub ordering_fields: Vec<String>,
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
}

impl Root {
    fn new(path: String, depth: u32, archives: bool, symlinks: bool) -> Root {
        Root { path, depth, archives, symlinks }
    }

    fn default() -> Root {
        Root { path: String::from("."), depth: 0, archives: false, symlinks: false }
    }
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub left: Option<Box<Expr>>,
    pub logical_op: Option<LogicalOp>,
    pub right: Option<Box<Expr>>,

    pub field: Option<String>,
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

    fn leaf(field: String, op: Option<Op>, val: String) -> Expr {
        Expr {
            left: None,
            logical_op: None,
            right: None,

            field: Some(field),
            op,
            val: Some(val),
            regex: None,

            dt_from: None,
            dt_to: None,
        }
    }

    fn leaf_regex(field: String, op: Option<Op>, val: String, regex: Regex) -> Expr {
        Expr {
            left: None,
            logical_op: None,
            right: None,

            field: Some(field),
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
        let query = "select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' limit 50";
        let mut p = Parser::new();
        let query = p.parse(&query).unwrap();

        assert_eq!(query.fields, vec![String::from("name"), String::from("path"), String::from("size"), String::from("fsize")]);
        assert_eq!(query.roots, vec![
            Root::new(String::from("/test"), 2, false, false),
            Root::new(String::from("/test2"), 0, true, false),
            Root::new(String::from("/test3"), 3, true, false),
            Root::new(String::from("/test4"), 0, false, false),
            Root::new(String::from("/test5"), 0, false, false),
        ]);

        let expr = Expr::node(
            Some(Box::new(
                Expr::node(
                    Some(Box::new(Expr::leaf(String::from("name"), Some(Op::Ne), String::from("123")))),
                    Some(LogicalOp::And),
                    Some(Box::new(Expr::node(
                        Some(Box::new(Expr::leaf(String::from("size"), Some(Op::Gt), String::from("456")))),
                        Some(LogicalOp::Or),
                        Some(Box::new(Expr::leaf(String::from("fsize"), Some(Op::Lte), String::from("758")))),
                    ))),
                )
            )),
            Some(LogicalOp::Or),
            Some(Box::new(
                Expr::leaf(String::from("name"), Some(Op::Eq), String::from("xxx"))
            ))
        );

        assert_eq!(query.expr, Some(Box::new(expr)));
        assert_eq!(query.limit, 50);
    }
}

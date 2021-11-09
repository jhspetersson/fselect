use std::str::FromStr;

use regex::Regex;

use crate::field::Field;
use crate::function::Function;

#[derive(Clone, PartialEq, Debug)]
pub enum Lexem {
    RawString(String),
    Comma,
    From,
    Where,
    Operator(String),
    String(String),
    Open,
    Close,
    ArithmeticOperator(String),
    And,
    Or,
    Not,
    Order,
    By,
    DescendingOrder,
    Limit,
    Into,
}

#[derive(Debug, PartialEq)]
enum LexingMode {
    Undefined,
    RawString,
    Comma,
    Operator,
    ArithmeticOperator,
    String,
    Open,
    Close,
}

pub struct Lexer<'a> {
    input: &'a str,
    index: usize,
    before_from: bool,
    after_open: bool,
    after_where: bool,
    after_operator: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &str) -> Lexer {
        return Lexer { input, index: 0, before_from: true, after_open: false, after_where: false, after_operator: false, }
    }

    pub fn next_lexem(&mut self) -> Option<Lexem> {
        let mut s = String::new();
        let mut mode = LexingMode::Undefined;

        for c in self.input.chars().skip(self.index) {
            match mode {
                LexingMode::Comma | LexingMode::Open | LexingMode::Close => {
                    break
                },
                LexingMode::String => {
                    self.index += 1;
                    if c == '\'' {
                        break
                    }
                    s.push(c);
                },
                LexingMode::Operator => {
                    if !self.is_op_char(c) {
                        break
                    }

                    self.index += 1;
                    s.push(c);
                },
                LexingMode::ArithmeticOperator => {
                    break;
                },
                LexingMode::RawString => {
                    let is_date = c == '-' && looks_like_date(&s);
                    if !is_date {
                        if self.is_arithmetic_op_char(c) {
                            let maybe_expr = looks_like_expression(&s);
                            if maybe_expr {
                                break;
                            }
                        } else if c == ' ' || c == ',' || c == '(' || c == ')' || self.is_op_char(c) {
                            break
                        }
                    }

                    self.index += 1;
                    s.push(c);
                },
                LexingMode::Undefined => {
                    self.index += 1;
                    match c {
                        ' ' => {},
                        '\'' => mode = LexingMode::String,
                        ',' => mode = LexingMode::Comma,
                        '(' => mode = LexingMode::Open,
                        ')' => mode = LexingMode::Close,
                        _ => {
                            mode = if self.is_op_char(c) {
                                LexingMode::Operator
                            } else if self.is_arithmetic_op_char(c) {
                                LexingMode::ArithmeticOperator
                            } else {
                                LexingMode::RawString
                            };
                            s.push(c);
                        }
                    }

                    if mode == LexingMode::Open {
                        self.after_open = true;
                    } else {
                        self.after_open = false;
                    }
                },
            }
        }

        let lexem = match mode {
            LexingMode::String => Some(Lexem::String(s)),
            LexingMode::Operator => Some(Lexem::Operator(s)),
            LexingMode::ArithmeticOperator => Some(Lexem::ArithmeticOperator(s)),
            LexingMode::Comma => Some(Lexem::Comma),
            LexingMode::Open => Some(Lexem::Open),
            LexingMode::Close => Some(Lexem::Close),
            LexingMode::RawString => {
                match s.to_lowercase().as_str() {
                    "from" => { self.before_from = false; Some(Lexem::From) },
                    "where" => { self.after_where = true; Some(Lexem::Where) },
                    "or" => Some(Lexem::Or),
                    "and" => Some(Lexem::And),
                    "not" if self.after_where => Some(Lexem::Not),
                    "order" => Some(Lexem::Order),
                    "by" => Some(Lexem::By),
                    "asc" => self.next_lexem(),
                    "desc" => Some(Lexem::DescendingOrder),
                    "limit" => Some(Lexem::Limit),
                    "into" => Some(Lexem::Into),
                    "eq" | "ne" | "gt" | "lt" | "ge" | "le" | "gte" | "lte" |
                    "regexp" | "rx" | "like" => Some(Lexem::Operator(s)),
                    "mul" | "div" | "mod" | "plus" | "minus" => Some(Lexem::ArithmeticOperator(s)),
                    _ => Some(Lexem::RawString(s)),
                }
            },
            _ => None
        };

        self.after_operator = match lexem {
            Some(Lexem::Operator(_)) => true,
            _ => false
        };

        lexem
    }

    fn is_arithmetic_op_char(&self, c: char) -> bool {
        match c {
            '+' | '-' => self.before_from || self.after_where,
            '*' | '/' | '%' => (self.before_from || self.after_where) && !self.after_open && !self.after_operator,
            _ => false
        }
    }

    fn is_op_char(&self, c: char) -> bool {
        if !self.before_from && !self.after_where {
            return false;
        }

        match c {
            '=' | '!' | '<' | '>' | '~' => true,
                _ => false
        }
    }
}


lazy_static! {
    static ref DATE_ALIKE_REGEX: Regex = Regex::new("(\\d{4})-?(\\d{2})?").unwrap();
}

fn looks_like_expression(s: &str) -> bool {
    !s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|s| Field::from_str(s).is_err() && Function::from_str(s).is_err() && s.parse::<i64>().is_err())
}

fn looks_like_date(s: &str) -> bool {
    match DATE_ALIKE_REGEX.captures(s) {
        Some(cap) => {
            let year = cap[1].parse::<i32>();
            let year_ok = match year {
                Ok(year) => year >= 1970 && year < 3000, // optimistic assumption
                _ => false
            };

            if !year_ok {
                return false;
            }

            match cap.get(2) {
                Some(month) => {
                    let month = month.as_str().parse::<i32>();
                    let month_ok = match month {
                        Ok(month) => month >= 1 && month <= 12,
                        _ => false
                    };

                    month_ok
                },
                _ => true
            }
        },
        _ => false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_date_test() {
        assert!(looks_like_date("2018"));
        assert!(looks_like_date("2018-01"));
    }

    #[test]
    fn lexems() {
        let mut lexer = Lexer::new("select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 1 ,3 desc , path asc limit 50");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("select"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("fsize"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/test"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("depth"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("2"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/test2"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("archives"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/test3"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("depth"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("3"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("archives"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/test4"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::String(String::from("/test5"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("!="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("123"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::And));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("gt"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("456"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Or));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("fsize"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("lte"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("758"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Or));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::String(String::from("xxx"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Order));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("1"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("3"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::DescendingOrder));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Limit));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("50"))));
    }

    #[test]
    fn spaces() {
        let lexer = Lexer::new("path,size from . where size=0");
        assert_spaces(lexer);

        let lexer = Lexer::new("path,size from . where size =0");
        assert_spaces(lexer);

        let lexer = Lexer::new("path,size from . where size= 0");
        assert_spaces(lexer);

        let lexer = Lexer::new("path,size from . where size = 0");
        assert_spaces(lexer);

        let lexer = Lexer::new("path,size from . where size   =     0");
        assert_spaces(lexer);
    }

    fn assert_spaces(mut lexer: Lexer) {
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("0"))));
    }

    #[test]
    fn func_calls() {
        let mut lexer = Lexer::new("name, length(name),UPPER( name ) from .");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("length"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("UPPER"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
    }

    #[test]
    fn func_calls2() {
        let mut lexer = Lexer::new("select name, upper(name) from . depth 1");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("select"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("upper"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("depth"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("1"))));
    }

    #[test]
    fn func_calls3() {
        let mut lexer = Lexer::new("select name, rand() from . depth 1 order by rand() limit 10");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("select"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("rand"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("depth"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("1"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Order));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("rand"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Limit));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("10"))));
    }

    #[test]
    fn agg_func_calls() {
        let mut lexer = Lexer::new("COUNT(*), MIN(size), AVG(size), MAX(size) from .");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("COUNT"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("*"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("MIN"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("AVG"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("MAX"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
    }

    #[test]
    fn arithmetic_operators() {
        let mut lexer = Lexer::new("width + height, width-height, width mul height, path from .");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("width"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::ArithmeticOperator(String::from("+"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("height"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("width"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::ArithmeticOperator(String::from("-"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("height"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("width"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::ArithmeticOperator(String::from("mul"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("height"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
    }

    #[test]
    fn context_sensitive_date_string() {
        let mut lexer = Lexer::new("size,modified,path from . where modified gt 2018-08-01 and name='*.txt' order by modified");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("modified"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("modified"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("gt"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("2018-08-01"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::And));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::String(String::from("*.txt"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Order));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("modified"))));
    }

    #[test]
    fn root_with_dashes() {
        let mut lexer = Lexer::new("path from ./foo-bar");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("./foo-bar"))));
    }

    #[test]
    fn another_workaround_for_raw_paths() {
        let mut lexer = Lexer::new("name, size where path eq \\*some/stuff-inside/\\*.rs");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("eq"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("\\*some/stuff-inside/\\*.rs"))));
    }

    #[test]
    fn mime_types() {
        let mut lexer = Lexer::new("mime from . where mime = application/pkcs8+pem");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("mime"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("."))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("mime"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("application/pkcs8+pem"))));
    }

    #[test]
    fn raw_abs_path_after_eq() {
        let mut lexer = Lexer::new("abspath,absdir,name where absdir = /home/user/docs");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("abspath"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/home/user/docs"))));

        let mut lexer = Lexer::new("abspath,absdir,name where absdir == /home/user/docs");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("abspath"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("=="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/home/user/docs"))));

        let mut lexer = Lexer::new("abspath,absdir,name where absdir === /home/user/docs");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("abspath"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("==="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/home/user/docs"))));

        let mut lexer = Lexer::new("abspath,absdir,name where absdir eq /home/user/docs");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("abspath"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("absdir"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("eq"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/home/user/docs"))));
    }

    #[test]
    fn star_field() {
        let mut lexer = Lexer::new("select modified,* from /test");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("select"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("modified"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::ArithmeticOperator(String::from("*"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/test"))));

        let mut lexer = Lexer::new("select modified, * from /test limit 10");

        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("select"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("modified"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::ArithmeticOperator(String::from("*"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString(String::from("/test"))));
    }
}

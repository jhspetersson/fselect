//! Lexer to tokenizes SQL-like syntax into lexems

use std::str::FromStr;
use std::sync::LazyLock;
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
    CurlyOpen,
    CurlyClose,
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
    SingleQuotedString,
    DoubleQuotedString,
    BackticksQuotedString,
    Open,
    Close,
}

pub struct Lexer {
    input: Vec<String>,
    input_index: usize,
    char_index: isize,
    before_from: bool,
    possible_search_root: bool,
    after_open: bool,
    after_where: bool,
    after_operator: bool,
}

impl Lexer {
    pub fn new(input: Vec<String>) -> Lexer {
        Lexer {
            input,
            input_index: 0,
            char_index: 0,
            before_from: true,
            possible_search_root: false,
            after_open: false,
            after_where: false,
            after_operator: false,
        }
    }

    pub fn next_lexem(&mut self) -> Option<Lexem> {
        let mut s = String::new();
        let mut mode = LexingMode::Undefined;

        loop {
            let input_part = self.input.get(self.input_index);
            if input_part.is_none() {
                break;
            }
            let input_part = input_part.unwrap();
            
            let c;
            if self.char_index == -1 {
                c = ' ';
            } else {
                let input_char = input_part.chars().nth(self.char_index as usize);
                if input_char.is_none() {
                    self.input_index += 1;
                    self.char_index = -1;
                    self.possible_search_root = false;
                    continue;
                }
                c = input_char.unwrap();
            }
            
            match mode {
                LexingMode::Comma | LexingMode::Open | LexingMode::Close => break,
                LexingMode::SingleQuotedString => {
                    self.char_index += 1;
                    if c == '\'' {
                        break;
                    }
                    s.push(c);
                }
                LexingMode::DoubleQuotedString => {
                    self.char_index += 1;
                    if c == '"' {
                        break;
                    }
                    s.push(c);
                }
                LexingMode::BackticksQuotedString => {
                    self.char_index += 1;
                    if c == '`' {
                        break;
                    }
                    s.push(c);
                }
                LexingMode::Operator => {
                    if !self.is_op_char(c) {
                        break;
                    }

                    self.char_index += 1;
                    s.push(c);
                }
                LexingMode::ArithmeticOperator => {
                    break;
                }
                LexingMode::RawString => {
                    let is_date = c == '-' && looks_like_date(&s);
                    if !is_date {
                        if self.is_arithmetic_op_char(c) {
                            let maybe_expr = looks_like_expression(&s);
                            if maybe_expr {
                                break;
                            }
                        } else if (self.input.len() == 1 
                                || (self.input.len() > 1 && !self.possible_search_root)) 
                            && (c == ' ' || c == ',' || is_paren_char(c) || self.is_op_char(c)) {
                            break;
                        }
                    }

                    self.char_index += 1;
                    s.push(c);
                }
                LexingMode::Undefined => {
                    self.char_index += 1;
                    match c {
                        ' ' => {}
                        '\'' => mode = LexingMode::SingleQuotedString,
                        '"' => mode = LexingMode::DoubleQuotedString,
                        '`' => mode = LexingMode::BackticksQuotedString,
                        ',' => mode = LexingMode::Comma,
                        '(' | '{' => {
                            s.push(c);
                            mode = LexingMode::Open
                        }
                        ')' | '}' => {
                            s.push(c);
                            mode = LexingMode::Close
                        }
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

                    self.after_open = mode == LexingMode::Open;
                }
            }
        }

        let lexem = match mode {
            LexingMode::SingleQuotedString => Some(Lexem::String(s)),
            LexingMode::DoubleQuotedString => Some(Lexem::String(s)),
            LexingMode::BackticksQuotedString => Some(Lexem::String(s)),
            LexingMode::Operator => Some(Lexem::Operator(s)),
            LexingMode::ArithmeticOperator => Some(Lexem::ArithmeticOperator(s)),
            LexingMode::Comma => Some(Lexem::Comma),
            LexingMode::Open if &s == "(" => {
                s.clear();
                Some(Lexem::Open)
            }
            LexingMode::Open if &s == "{" => {
                s.clear();
                Some(Lexem::CurlyOpen)
            }
            LexingMode::Close if &s == ")" => {
                s.clear();
                Some(Lexem::Close)
            }
            LexingMode::Close if &s == "}" => {
                s.clear();
                Some(Lexem::CurlyClose)
            }
            LexingMode::RawString => match s.to_lowercase().as_str() {
                "from" => {
                    self.before_from = false;
                    self.after_where = false;
                    Some(Lexem::From)
                }
                "where" => {
                    self.after_where = true;
                    Some(Lexem::Where)
                }
                "or" => Some(Lexem::Or),
                "and" => Some(Lexem::And),
                "not" if self.after_where => Some(Lexem::Not),
                "order" => Some(Lexem::Order),
                "by" => Some(Lexem::By),
                "asc" => self.next_lexem(),
                "desc" => Some(Lexem::DescendingOrder),
                "limit" => Some(Lexem::Limit),
                "into" => Some(Lexem::Into),
                "eq" | "ne" | "gt" | "lt" | "ge" | "le" | "gte" | "lte" | "regexp" | "rx"
                | "like" | "between" => Some(Lexem::Operator(s)),
                "mul" | "div" | "mod" | "plus" | "minus" => Some(Lexem::ArithmeticOperator(s)),
                _ => Some(Lexem::RawString(s)),
            },
            _ => None,
        };

        self.possible_search_root = matches!(lexem, Some(Lexem::From))
                || (matches!(lexem, Some(Lexem::Comma)) && !self.before_from && !self.after_where);
        self.after_operator = matches!(lexem, Some(Lexem::Operator(_)));

        lexem
    }

    fn is_arithmetic_op_char(&self, c: char) -> bool {
        match c {
            '+' | '-' => self.before_from || self.after_where,
            '*' | '/' | '%' => {
                (self.before_from || self.after_where) && !self.after_open && !self.after_operator
            }
            _ => false,
        }
    }

    fn is_op_char(&self, c: char) -> bool {
        if !self.before_from && !self.after_where {
            return false;
        }

        matches!(c, '=' | '!' | '<' | '>' | '~')
    }
}

fn is_paren_char(c: char) -> bool {
    c == '(' || c == ')' || c == '{' || c == '}'
}

static DATE_ALIKE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(\\d{4})-?(\\d{2})?").unwrap()
});

fn looks_like_expression(s: &str) -> bool {
    !s.split(|c: char| !c.is_ascii_alphanumeric()).any(|s| {
        Field::from_str(s).is_err() && Function::from_str(s).is_err() && s.parse::<i64>().is_err()
    })
}

fn looks_like_date(s: &str) -> bool {
    match DATE_ALIKE_REGEX.captures(s) {
        Some(cap) => {
            let year = cap[1].parse::<i32>();
            let year_ok = match year {
                Ok(year) => (1970..3000).contains(&year), // optimistic assumption
                _ => false,
            };

            if !year_ok {
                return false;
            }

            match cap.get(2) {
                Some(month) => {
                    let month = month.as_str().parse::<i32>();

                    match month {
                        Ok(month) => (1..=12).contains(&month),
                        _ => false,
                    }
                }
                _ => true,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! lexer {
        ($($str:literal),+) => {
            {
                let quote = vec![$($str.to_string()),+];
                Lexer::new(quote)
            }
        }
    }

    #[test]
    fn looks_like_date_test() {
        assert!(looks_like_date("2018"));
        assert!(looks_like_date("2018-01"));
    }

    #[test]
    fn lexems() {
        let mut lexer = lexer!("select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 1 ,3 desc , path asc limit 50");
        
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("fsize")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("2")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test2")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("archives")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test3")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("3")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("archives")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test4")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::String(String::from("/test5")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("!=")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("123")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::And));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("gt")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("456")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Or));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("fsize")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("lte")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("758")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Or));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::String(String::from("xxx"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Order));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("1")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("3")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::DescendingOrder));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Limit));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("50")))
        );
    }

    #[test]
    fn spaces() {
        let lexer = lexer!("path,size from . where size=0");
        assert_spaces(lexer);

        let lexer = lexer!("path,size from . where size =0");
        assert_spaces(lexer);

        let lexer = lexer!("path,size from . where size= 0");
        assert_spaces(lexer);

        let lexer = lexer!("path,size from . where size = 0");
        assert_spaces(lexer);

        let lexer = lexer!("path,size from . where size   =     0");
        assert_spaces(lexer);
    }

    fn assert_spaces(mut lexer: Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("0")))
        );
    }

    #[test]
    fn func_calls() {
        let mut lexer = lexer!("name, length(name),UPPER( name ) from .");
        assert_func_calls_lexems(&mut lexer);
    }

    #[test]
    fn func_calls_with_multiple_input_parts() {
        let mut lexer = lexer!("name,", "length(name),UPPER(", "name", ")", "from", ".");
        assert_func_calls_lexems(&mut lexer);
    }

    fn assert_func_calls_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("length")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("UPPER")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
    }

    #[test]
    fn func_calls2() {
        let mut lexer = lexer!("select name, upper(name) from . depth 1");
        assert_func_calls2_lexems(&mut lexer);
    }

    #[test]
    fn func_calls2_with_multiple_input_parts() {
        let mut lexer = lexer!("select", "name,", "upper(name)", "from", ".", "depth", "1");
        assert_func_calls2_lexems(&mut lexer);
    }

    fn assert_func_calls2_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("upper")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("1")))
        );
    }

    #[test]
    fn func_calls3() {
        let mut lexer = lexer!("select name, rand() from . depth 1 order by rand() limit 10");
        assert_func_calls3_lexems(&mut lexer);
    }

    #[test]
    fn func_calls3_with_multiple_input_parts() {
        let mut lexer = lexer!(
            "select", "name,", "rand()", "from", ".", "depth", "1", "order", "by", "rand()", "limit", "10"
        );
        assert_func_calls3_lexems(&mut lexer);
    }

    fn assert_func_calls3_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("rand")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("1")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Order));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("rand")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Limit));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("10")))
        );
    }

    #[test]
    fn agg_func_calls() {
        let mut lexer = lexer!("COUNT(*), MIN(size), AVG(size), MAX(size) from .");
        assert_agg_func_calls_lexems(&mut lexer);
    }

    #[test]
    fn agg_func_calls_with_multiple_input_parts() {
        let mut lexer = lexer!("COUNT(*),", "MIN(size),", "AVG(size),", "MAX(size)", "from", ".");
        assert_agg_func_calls_lexems(&mut lexer);
    }

    fn assert_agg_func_calls_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("COUNT")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("*")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("MIN")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("AVG")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("MAX")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
    }

    #[test]
    fn arithmetic_operators() {
        let mut lexer = lexer!("width + height, width-height, width mul height, path from .");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("width")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::ArithmeticOperator(String::from("+")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("height")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("width")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::ArithmeticOperator(String::from("-")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("height")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("width")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::ArithmeticOperator(String::from("mul")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("height")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
    }

    #[test]
    fn context_sensitive_date_string() {
        let mut lexer = lexer!("size,modified,path from . where modified gt 2018-08-01 and name='*.txt' order by modified");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("modified")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("gt")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("2018-08-01")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::And));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::String(String::from("*.txt")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Order));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("modified")))
        );
    }

    #[test]
    fn root_with_dashes() {
        let mut lexer = lexer!("path from ./foo-bar");
        assert_root_with_dashes_lexems(&mut lexer);
    }

    #[test]
    fn root_with_dashes_with_multiple_input_parts() {
        let mut lexer = lexer!("path", "from", "./foo-bar");
        assert_root_with_dashes_lexems(&mut lexer);
    }

    fn assert_root_with_dashes_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("./foo-bar")))
        );
    }

    #[test]
    fn another_workaround_for_raw_paths() {
        let mut lexer = lexer!("name, size where path eq \\*some/stuff-inside/\\*.rs");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("path")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("eq")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(
                "\\*some/stuff-inside/\\*.rs"
            )))
        );
    }

    #[test]
    fn mime_types() {
        let mut lexer = lexer!("mime from . where mime = application/pkcs8+pem");
        assert_mime_types_lexems(&mut lexer);
    }

    #[test]
    fn mime_types_with_multiple_input_parts() {
        let mut lexer = lexer!("mime", "from", ".", "where", "mime", "=", "application/pkcs8+pem");
        assert_mime_types_lexems(&mut lexer);
    }

    fn assert_mime_types_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("mime")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from(".")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("mime")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("application/pkcs8+pem")))
        );
    }

    #[test]
    fn raw_abs_path_after_eq() {
        let mut lexer = lexer!("abspath,absdir,name where absdir = /home/user/docs");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/home/user/docs")))
        );

        let mut lexer = lexer!("abspath,absdir,name where absdir == /home/user/docs");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("==")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/home/user/docs")))
        );

        let mut lexer = lexer!("abspath,absdir,name where absdir === /home/user/docs");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("===")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/home/user/docs")))
        );

        let mut lexer = lexer!("abspath,absdir,name where absdir eq /home/user/docs");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("absdir")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("eq")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/home/user/docs")))
        );
    }

    #[test]
    fn star_field() {
        let mut lexer = lexer!("select modified,* from /test");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::ArithmeticOperator(String::from("*")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test")))
        );

        let mut lexer = lexer!("select modified, * from /test limit 10");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::ArithmeticOperator(String::from("*")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test")))
        );
    }

    #[test]
    fn lexer_with_multiple_input_parts() {
        let mut lexer = lexer!("select", "modified,*", "from", "/test");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::ArithmeticOperator(String::from("*")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test")))
        );
    }

    #[test]
    fn group_by() {
        let mut lexer = lexer!("select AVG(size) from /test group by mime");
        assert_group_by_lexems(&mut lexer);
    }

    #[test]
    fn group_by_with_multiple_input_parts() {
        let mut lexer = lexer!("select", "AVG(size)", "from", "/test", "group", "by", "mime");
        assert_group_by_lexems(&mut lexer);
    }

    fn assert_group_by_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("AVG")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/test")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::RawString("group".to_owned())));
        assert_eq!(lexer.next_lexem(), Some(Lexem::By));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("mime")))
        );
    }

    #[test]
    fn between_op() {
        let mut lexer = lexer!("select name from /home/user where size between 1000000 and 2000000");
        assert_between_op_lexems(&mut lexer);
    }

    #[test]
    fn between_op_with_multiple_input_parts() {
        let mut lexer = lexer!(
            "select", "name", "from", "/home/user", "where", "size", "between", "1000000", "and", "2000000"
        );
        assert_between_op_lexems(&mut lexer);
    }

    fn assert_between_op_lexems(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("/home/user")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("size")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::Operator(String::from("between")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("1000000")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::And));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("2000000")))
        );
    }

    #[test]
    fn spaces_in_path_with_single_quotes() {
        let mut lexer = lexer!("select name from '/home/user/foo bar/'");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::String(String::from("/home/user/foo bar/")))
        );
    }

    #[test]
    fn spaces_in_path_with_double_quotes() {
        let mut lexer = lexer!("select name from \"/home/user/foo bar/\"");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::String(String::from("/home/user/foo bar/")))
        );
    }

    #[test]
    fn spaces_in_path_with_backticks() {
        let mut lexer = lexer!("select name from `/home/user/foo bar/`");

        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("select")))
        );
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(
            lexer.next_lexem(),
            Some(Lexem::String(String::from("/home/user/foo bar/")))
        );
    }
}

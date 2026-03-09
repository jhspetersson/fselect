//! Lexer to tokenize SQL-like syntax into lexemes

use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;

use crate::field::Field;
use crate::function::Function;

#[derive(Clone, PartialEq, Debug)]
pub enum Lexeme {
    Select,
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
    Offset,
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

#[derive(Clone)]
struct LexerState {
    first_lexeme: bool,
    before_from: bool,
    possible_search_root: bool,
    after_open: bool,
    after_where: bool,
    after_operator: bool,
    after_logical: bool,
}

impl LexerState {
    fn new() -> Self {
        LexerState {
            first_lexeme: true,
            before_from: true,
            possible_search_root: false,
            after_open: false,
            after_where: false,
            after_operator: false,
            after_logical: false,
        }
    }
}

#[derive(Clone)]
pub struct Lexer {
    input: Vec<String>,
    input_index: usize,
    char_index: isize,
    state: Box<LexerState>,
    state_history: Vec<Box<LexerState>>,
}

impl Lexer {
    pub fn new(input: Vec<String>) -> Lexer {
        Lexer {
            input,
            input_index: 0,
            char_index: 0,
            state: Box::new(LexerState::new()),
            state_history: vec![],
        }
    }

    pub fn get_input_string(&self) -> String {
        self.input.join(" ")
    }

    pub fn push_state(&mut self) {
        self.state_history.push(self.state.clone());
        self.state = Box::new(LexerState::new());
    }

    pub fn pop_state(&mut self) {
        if let Some(state) = self.state_history.pop() {
            self.state = state;
        } else {
            self.state = Box::new(LexerState::new());
        }
    }

    pub fn next_lexeme(&mut self) -> Option<Lexeme> {
        let mut s = String::new();
        let mut mode = LexingMode::Undefined;
        let mut quote_closed = false;

        loop {
            let input_part = self.input.get(self.input_index);
            if input_part.is_none() {
                break;
            }
            let input_part = input_part.unwrap();
            
            let c;
            if self.char_index == -1 {
                match mode {
                    LexingMode::SingleQuotedString
                    | LexingMode::DoubleQuotedString
                    | LexingMode::BackticksQuotedString => {
                        self.char_index = 0;
                        continue;
                    }
                    LexingMode::RawString if s.ends_with('-') && looks_like_date(&s[..s.len() - 1]) => {
                        self.char_index = 0;
                        continue;
                    }
                    LexingMode::Operator => {
                        self.char_index = 0;
                        continue;
                    }
                    _ => {
                        c = ' ';
                    }
                }
            } else {
                let input_char = input_part.chars().nth(self.char_index as usize);
                if input_char.is_none() {
                    self.input_index += 1;
                    self.char_index = -1;
                    self.state.possible_search_root = false;
                    continue;
                }
                c = input_char.unwrap();
            }
            
            match mode {
                LexingMode::Comma | LexingMode::Open | LexingMode::Close => break,
                LexingMode::SingleQuotedString => {
                    self.char_index += 1;
                    if c == '\'' {
                        quote_closed = true;
                        break;
                    }
                    s.push(c);
                }
                LexingMode::DoubleQuotedString => {
                    self.char_index += 1;
                    if c == '"' {
                        quote_closed = true;
                        break;
                    }
                    s.push(c);
                }
                LexingMode::BackticksQuotedString => {
                    self.char_index += 1;
                    if c == '`' {
                        quote_closed = true;
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
                    let is_date = c == '-' && looks_like_date(&s) && {
                        let next_char = input_part.chars().nth((self.char_index + 1) as usize)
                            .or_else(|| self.input.get(self.input_index + 1)
                                .and_then(|p| p.chars().next()));
                        matches!(next_char, Some('0'..='9'))
                    };
                    if !is_date {
                        if self.is_arithmetic_op_char(c) {
                            let maybe_expr = looks_like_expression(&s);
                            if maybe_expr {
                                break;
                            }
                        } else if (self.input.len() == 1 
                                || (self.input.len() > 1 && !self.state.possible_search_root))
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

                    if c != ' ' {
                        self.state.after_open = mode == LexingMode::Open;
                    }
                }
            }
        }

        let lexeme = match mode {
            LexingMode::SingleQuotedString if quote_closed => Some(Lexeme::String(s)),
            LexingMode::DoubleQuotedString if quote_closed => Some(Lexeme::String(s)),
            LexingMode::BackticksQuotedString if quote_closed => Some(Lexeme::String(s)),
            LexingMode::Operator => Some(Lexeme::Operator(s)),
            LexingMode::ArithmeticOperator => Some(Lexeme::ArithmeticOperator(s)),
            LexingMode::Comma => Some(Lexeme::Comma),
            LexingMode::Open if &s == "(" => {
                s.clear();
                Some(Lexeme::Open)
            }
            LexingMode::Open if &s == "{" => {
                s.clear();
                Some(Lexeme::CurlyOpen)
            }
            LexingMode::Close if &s == ")" => {
                s.clear();
                Some(Lexeme::Close)
            }
            LexingMode::Close if &s == "}" => {
                s.clear();
                Some(Lexeme::CurlyClose)
            }
            LexingMode::RawString => match s.to_lowercase().as_str() {
                "select" if !self.state.after_operator => {
                    Some(Lexeme::Select)
                }
                "from" if !self.state.after_operator && !self.state.after_logical => {
                    self.state.before_from = false;
                    self.state.after_where = false;
                    Some(Lexeme::From)
                }
                "where" if !self.state.after_operator && !self.state.after_logical => {
                    self.state.before_from = false;
                    self.state.after_where = true;
                    Some(Lexeme::Where)
                }
                "or" if self.state.after_where && !self.state.after_operator && !self.state.after_logical => Some(Lexeme::Or),
                "and" if self.state.after_where && !self.state.after_operator && !self.state.after_logical => Some(Lexeme::And),
                "not" if self.state.after_where && !self.state.after_operator => Some(Lexeme::Not),
                "order" if !self.state.after_operator && !self.state.after_logical => {
                    self.state.after_where = false;
                    Some(Lexeme::Order)
                }
                "by" if !self.state.after_operator && !self.state.after_logical => Some(Lexeme::By),
                "asc" if !self.state.after_operator && !self.state.before_from && !self.state.after_logical => self.next_lexeme(),
                "desc" if !self.state.after_operator && !self.state.before_from && !self.state.after_logical => Some(Lexeme::DescendingOrder),
                "limit" if !self.state.after_operator && !self.state.after_logical => {
                    self.state.after_where = false;
                    Some(Lexeme::Limit)
                }
                "offset" if !self.state.after_operator && !self.state.after_logical => {
                    self.state.after_where = false;
                    Some(Lexeme::Offset)
                }
                "into" if !self.state.after_operator && !self.state.after_logical => {
                    self.state.after_where = false;
                    Some(Lexeme::Into)
                }
                "exists" if self.state.after_where && !self.state.after_operator => Some(Lexeme::Operator(s.to_lowercase())),
                "eq" | "ne" | "gt" | "lt" | "ge" | "le" | "gte" | "lte" | "regexp" | "rx"
                | "like" | "between" | "in" if self.state.after_where && !self.state.after_operator && !self.state.after_logical => Some(Lexeme::Operator(s.to_lowercase())),
                "mul" | "div" | "mod" | "plus" | "minus" if (self.state.before_from || self.state.after_where) && !self.state.after_operator && !self.state.after_logical => Some(Lexeme::ArithmeticOperator(s)),
                _ => Some(Lexeme::RawString(s)),
            },
            _ => None,
        };

        self.state.first_lexeme = false;
        self.state.possible_search_root = matches!(lexeme, Some(Lexeme::From))
                || (matches!(lexeme, Some(Lexeme::Comma)) && !self.state.before_from && !self.state.after_where);
        self.state.after_operator = matches!(lexeme, Some(Lexeme::Operator(_)));
        self.state.after_logical = matches!(lexeme, Some(Lexeme::Where) | Some(Lexeme::And) | Some(Lexeme::Or) | Some(Lexeme::Open) | Some(Lexeme::CurlyOpen))
                || (matches!(lexeme, Some(Lexeme::Comma)) && self.state.after_where);

        lexeme
    }

    fn is_arithmetic_op_char(&self, c: char) -> bool {
        match c {
            '+' | '-' => (self.state.before_from || self.state.after_where) && !self.state.after_operator,
            '*' | '/' | '%' => {
                (self.state.before_from || self.state.after_where) && !self.state.after_open && !self.state.after_operator
            }
            _ => false,
        }
    }

    fn is_op_char(&self, c: char) -> bool {
        if !self.state.before_from && !self.state.after_where {
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
        Field::parse_field(s).is_err() && Function::from_str(s).is_err() && s.parse::<i64>().is_err()
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
    fn lexemes() {
        let mut lexer = lexer!("select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 1 ,3 desc , path asc limit 50");
        
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("fsize")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("2")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test2")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("archives")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test3")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("3")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("archives")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test4")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::String(String::from("/test5")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("!=")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("123")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("gt")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("456")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Or));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("fsize")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("lte")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("758")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Or));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::String(String::from("xxx"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Order));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("1")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("3")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::DescendingOrder));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Limit));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("50")))
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
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("0")))
        );
    }

    #[test]
    fn func_calls() {
        let mut lexer = lexer!("name, length(name),UPPER( name ) from .");
        assert_func_calls_lexemes(&mut lexer);
    }

    #[test]
    fn func_calls_with_multiple_input_parts() {
        let mut lexer = lexer!("name,", "length(name),UPPER(", "name", ")", "from", ".");
        assert_func_calls_lexemes(&mut lexer);
    }

    fn assert_func_calls_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("length")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("UPPER")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
    }

    #[test]
    fn func_calls2() {
        let mut lexer = lexer!("select name, upper(name) from . depth 1");
        assert_func_calls2_lexemes(&mut lexer);
    }

    #[test]
    fn func_calls2_with_multiple_input_parts() {
        let mut lexer = lexer!("select", "name,", "upper(name)", "from", ".", "depth", "1");
        assert_func_calls2_lexemes(&mut lexer);
    }

    fn assert_func_calls2_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("upper")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("1")))
        );
    }

    #[test]
    fn func_calls3() {
        let mut lexer = lexer!("select name, rand() from . depth 1 order by rand() limit 10");
        assert_func_calls3_lexemes(&mut lexer);
    }

    #[test]
    fn func_calls3_with_multiple_input_parts() {
        let mut lexer = lexer!(
            "select", "name,", "rand()", "from", ".", "depth", "1", "order", "by", "rand()", "limit", "10"
        );
        assert_func_calls3_lexemes(&mut lexer);
    }

    fn assert_func_calls3_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("rand")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("depth")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("1")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Order));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("rand")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Limit));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("10")))
        );
    }

    #[test]
    fn agg_func_calls() {
        let mut lexer = lexer!("COUNT(*), MIN(size), AVG(size), MAX(size) from .");
        assert_agg_func_calls_lexemes(&mut lexer);
    }

    #[test]
    fn agg_func_calls_with_multiple_input_parts() {
        let mut lexer = lexer!("COUNT(*),", "MIN(size),", "AVG(size),", "MAX(size)", "from", ".");
        assert_agg_func_calls_lexemes(&mut lexer);
    }

    fn assert_agg_func_calls_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("COUNT")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("*")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("MIN")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("AVG")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("MAX")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
    }

    #[test]
    fn arithmetic_operators() {
        let mut lexer = lexer!("width + height, width-height, width mul height, path from .");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("width")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("+")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("height")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("width")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("-")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("height")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("width")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("mul")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("height")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
    }

    #[test]
    fn context_sensitive_date_string() {
        let mut lexer = lexer!("size,modified,path from . where modified gt 2018-08-01 and name='*.txt' order by modified");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("modified")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("gt")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("2018-08-01")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::String(String::from("*.txt")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Order));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("modified")))
        );
    }

    #[test]
    fn root_with_dashes() {
        let mut lexer = lexer!("path from ./foo-bar");
        assert_root_with_dashes_lexemes(&mut lexer);
    }

    #[test]
    fn root_with_dashes_with_multiple_input_parts() {
        let mut lexer = lexer!("path", "from", "./foo-bar");
        assert_root_with_dashes_lexemes(&mut lexer);
    }

    fn assert_root_with_dashes_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("./foo-bar")))
        );
    }

    #[test]
    fn another_workaround_for_raw_paths() {
        let mut lexer = lexer!("name, size where path eq \\*some/stuff-inside/\\*.rs");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("path")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("eq")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(
                "\\*some/stuff-inside/\\*.rs"
            )))
        );
    }

    #[test]
    fn mime_types() {
        let mut lexer = lexer!("mime from . where mime = application/pkcs8+pem");
        assert_mime_types_lexemes(&mut lexer);
    }

    #[test]
    fn mime_types_with_multiple_input_parts() {
        let mut lexer = lexer!("mime", "from", ".", "where", "mime", "=", "application/pkcs8+pem");
        assert_mime_types_lexemes(&mut lexer);
    }

    fn assert_mime_types_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("mime")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from(".")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("mime")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("application/pkcs8+pem")))
        );
    }

    #[test]
    fn raw_abs_path_after_eq() {
        let mut lexer = lexer!("abspath,absdir,name where absdir = /home/user/docs");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/home/user/docs")))
        );

        let mut lexer = lexer!("abspath,absdir,name where absdir == /home/user/docs");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("==")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/home/user/docs")))
        );

        let mut lexer = lexer!("abspath,absdir,name where absdir === /home/user/docs");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("===")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/home/user/docs")))
        );

        let mut lexer = lexer!("abspath,absdir,name where absdir eq /home/user/docs");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("abspath")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("absdir")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("eq")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/home/user/docs")))
        );
    }

    #[test]
    fn star_field() {
        let mut lexer = lexer!("select modified,* from /test");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("*")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test")))
        );

        let mut lexer = lexer!("select modified, * from /test limit 10");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("*")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test")))
        );
    }

    #[test]
    fn lexer_with_multiple_input_parts() {
        let mut lexer = lexer!("select", "modified,*", "from", "/test");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("modified")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("*")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test")))
        );
    }

    #[test]
    fn group_by() {
        let mut lexer = lexer!("select AVG(size) from /test group by mime");
        assert_group_by_lexemes(&mut lexer);
    }

    #[test]
    fn group_by_with_multiple_input_parts() {
        let mut lexer = lexer!("select", "AVG(size)", "from", "/test", "group", "by", "mime");
        assert_group_by_lexemes(&mut lexer);
    }

    fn assert_group_by_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("AVG")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/test")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString("group".to_owned())));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("mime")))
        );
    }

    #[test]
    fn between_op() {
        let mut lexer = lexer!("select name from /home/user where size between 1000000 and 2000000");
        assert_between_op_lexemes(&mut lexer);
    }

    #[test]
    fn between_op_with_multiple_input_parts() {
        let mut lexer = lexer!(
            "select", "name", "from", "/home/user", "where", "size", "between", "1000000", "and", "2000000"
        );
        assert_between_op_lexemes(&mut lexer);
    }

    fn assert_between_op_lexemes(lexer: &mut Lexer) {
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("/home/user")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from("between")))
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("1000000")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("2000000")))
        );
    }

    #[test]
    fn spaces_in_path_with_single_quotes() {
        let mut lexer = lexer!("select name from '/home/user/foo bar/'");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::String(String::from("/home/user/foo bar/")))
        );
    }

    #[test]
    fn spaces_in_path_with_double_quotes() {
        let mut lexer = lexer!("select name from \"/home/user/foo bar/\"");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::String(String::from("/home/user/foo bar/")))
        );
    }

    #[test]
    fn spaces_in_path_with_backticks() {
        let mut lexer = lexer!("select name from `/home/user/foo bar/`");

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select)
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("name")))
        );
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::String(String::from("/home/user/foo bar/")))
        );
    }

    #[test]
    fn unterminated_single_quoted_string() {
        let mut lexer = lexer!("name from . where name = 'unterminated");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        let lexeme = lexer.next_lexeme();
        assert_ne!(
            lexeme,
            Some(Lexeme::String(String::from("unterminated"))),
            "Unterminated single-quoted string should not silently produce a valid String lexeme"
        );
    }

    #[test]
    fn unterminated_double_quoted_string() {
        let mut lexer = lexer!("name where name = \"unterminated");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        let lexeme = lexer.next_lexeme();
        assert_ne!(
            lexeme,
            Some(Lexeme::String(String::from("unterminated"))),
            "Unterminated double-quoted string should not silently produce a valid String lexeme"
        );
    }

    #[test]
    fn unterminated_backtick_quoted_string() {
        let mut lexer = lexer!("name where name = `unterminated");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        let lexeme = lexer.next_lexeme();
        assert_ne!(
            lexeme,
            Some(Lexeme::String(String::from("unterminated"))),
            "Unterminated backtick-quoted string should not silently produce a valid String lexeme"
        );
    }

    #[test]
    fn asc_consumed_as_value_in_where() {
        let mut lexer = lexer!("name from . where name = asc");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("asc"))),
            "asc used as a value after an operator should produce RawString, not be silently consumed"
        );
    }

    #[test]
    fn asc_as_column_consumes_next_token() {
        let mut lexer = lexer!("select asc from .");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Select));

        let lexeme = lexer.next_lexeme();
        assert_eq!(
            lexeme,
            Some(Lexeme::RawString(String::from("asc"))),
            "asc in SELECT column list should be RawString, not silently consumed"
        );
    }

    #[test]
    fn not_as_value_after_operator_in_where() {
        let mut lexer = lexer!("name where name = not");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("not"))),
            "not used as a value after an operator should produce RawString, not Lexeme::Not"
        );
    }

    #[test]
    fn from_as_value_corrupts_operator_recognition() {
        let mut lexer = lexer!("name where name = from and size > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        lexer.next_lexeme();
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from(">"))),
            "Operator > should be recognized even after 'from' appears as a value in WHERE"
        );
    }

    #[test]
    fn select_as_value_in_where() {
        let mut lexer = lexer!("name where name = select");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("select"))),
            "select used as a value after an operator should produce RawString, not Lexeme::Select"
        );
    }

    #[test]
    fn after_open_reset_by_whitespace() {
        let mut lexer = lexer!("COUNT(*) from .");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("COUNT"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        let no_space = lexer.next_lexeme();
        assert_eq!(no_space, Some(Lexeme::RawString(String::from("*"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));

        let mut lexer = lexer!("COUNT( * ) from .");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("COUNT"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("*"))),
            "COUNT( * ) with space should treat * as RawString, same as COUNT(*)"
        );
    }

    #[test]
    fn desc_as_value_after_operator() {
        let mut lexer = lexer!("name where name = desc");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("desc"))),
            "desc used as a value after an operator should produce RawString, not DescendingOrder"
        );
    }

    #[test]
    fn or_as_value_after_operator() {
        let mut lexer = lexer!("name where name = or");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("or"))),
            "or used as a value after an operator should produce RawString, not Lexeme::Or"
        );
    }

    #[test]
    fn and_as_value_after_operator() {
        let mut lexer = lexer!("name where name = and");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("and"))),
            "and used as a value after an operator should produce RawString, not Lexeme::And"
        );
    }

    #[test]
    fn where_as_value_after_operator() {
        let mut lexer = lexer!("name where name = where");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("where"))),
            "where used as a value after an operator should produce RawString, not Lexeme::Where"
        );
    }

    #[test]
    fn limit_as_value_after_operator() {
        let mut lexer = lexer!("name where name = limit");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("limit"))),
            "limit used as a value after an operator should produce RawString, not Lexeme::Limit"
        );
    }

    #[test]
    fn date_heuristic_prevents_arithmetic() {
        let mut lexer = lexer!("select 2020-size from .");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Select));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("2020"))),
            "2020 should be its own token, not merged with -size by the date heuristic"
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::ArithmeticOperator(String::from("-"))),
            "Minus should be recognized as arithmetic operator"
        );
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("size"))),
            "size should be a separate RawString token"
        );
    }

    #[test]
    fn word_operator_as_value_after_operator() {
        let mut lexer = lexer!("name where name = eq");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("eq"))),
            "eq used as a value after an operator should produce RawString, not Operator"
        );
    }

    #[test]
    fn word_operator_in_as_value_after_operator() {
        let mut lexer = lexer!("name where name like in");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("like"))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("in"))),
            "in used as a value after an operator should produce RawString, not Operator"
        );
    }

    #[test]
    fn arithmetic_word_as_value_after_operator() {
        let mut lexer = lexer!("name where name = mul");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("mul"))),
            "mul used as a value after an operator should produce RawString, not ArithmeticOperator"
        );
    }

    #[test]
    fn arithmetic_word_mod_as_value_after_operator() {
        let mut lexer = lexer!("name where name = mod");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("mod"))),
            "mod used as a value after an operator should produce RawString, not ArithmeticOperator"
        );
    }

    #[test]
    fn date_across_input_part_boundary() {
        let mut lexer = lexer!("name from . where modified > 2018-08-01");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("modified"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from(">"))));
        let single_part = lexer.next_lexeme();
        assert_eq!(single_part, Some(Lexeme::RawString(String::from("2018-08-01"))));

        let mut lexer = lexer!("name", "from", ".", "where", "modified", ">", "2018-", "08-01");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("modified"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from(">"))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("2018-08-01"))),
            "Date split across input parts should still be recognized as a single date token"
        );
    }

    #[test]
    fn quoted_string_across_input_parts_injects_space() {
        let mut lexer = lexer!("'foobar'");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::String(String::from("foobar"))));

        let mut lexer = lexer!("'foo", "bar'");
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::String(String::from("foobar"))),
            "Quoted string spanning input parts should not have a space injected at the boundary"
        );
    }

    #[test]
    fn order_as_field_in_where() {
        let mut lexer = lexer!("name from . where order > 5");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("order"))),
            "order in WHERE field position should be RawString, not Order keyword"
        );
    }

    #[test]
    fn by_as_field_in_where() {
        let mut lexer = lexer!("name from . where by > 5");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("by"))),
            "by in WHERE field position should be RawString, not By keyword"
        );
    }

    #[test]
    fn or_in_select_column_list() {
        let mut lexer = lexer!("select or from .");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Select));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("or"))),
            "or in SELECT column list should be RawString, not Or keyword"
        );
    }

    #[test]
    fn and_in_select_column_list() {
        let mut lexer = lexer!("select and from .");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Select));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("and"))),
            "and in SELECT column list should be RawString, not And keyword"
        );
    }

    #[test]
    fn word_operator_in_select_column_list() {
        let mut lexer = lexer!("select gt from .");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Select));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("gt"))),
            "gt in SELECT column list should be RawString, not Operator"
        );
    }

    #[test]
    fn word_operator_in_order_by() {
        let mut lexer = lexer!("name from . where size > 0 order by gt");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from(">"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("0"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Order));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("gt"))),
            "gt in ORDER BY should be RawString, not Operator"
        );
    }

    #[test]
    fn operator_split_across_input_parts() {
        let mut lexer = lexer!("name from . where size >=10");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));
        let single_part = lexer.next_lexeme();
        assert_eq!(single_part, Some(Lexeme::Operator(String::from(">="))));

        let mut lexer = lexer!("name", "from", ".", "where", "size >", "= 10");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from(">="))),
            "Operator >= split across input parts should still be a single Operator"
        );
    }

    #[test]
    fn minus_as_value_start_vs_star_as_value_start() {
        let mut lexer = lexer!("name from . where name = *test");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        let star_result = lexer.next_lexeme();
        assert_eq!(star_result, Some(Lexeme::RawString(String::from("*test"))));

        let mut lexer = lexer!("name from . where name = -test");
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("-test"))),
            "- at value start should behave like * and produce RawString, not ArithmeticOperator"
        );
    }

    #[test]
    fn from_in_where_field_position_corrupts_state() {
        let mut lexer = lexer!("name from . where from and size > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("from"))),
            "from in WHERE field position should be RawString, not From keyword"
        );
    }

    #[test]
    fn select_in_where_field_position() {
        let mut lexer = lexer!("name from . where select > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Select),
            "select cannot be blocked in field position without breaking subquery support"
        );
    }

    #[test]
    fn from_after_and_corrupts_state() {
        let mut lexer = lexer!("name from . where name = foo and from and size > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("foo"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("from"))),
            "from after AND should be RawString in field position"
        );

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::Operator(String::from(">"))),
            "> should still be recognized as operator after from in field position"
        );
    }

    #[test]
    fn from_after_open_paren_in_where() {
        let mut lexer = lexer!("name from . where (from > 0)");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("from"))),
            "from after ( in WHERE should be RawString in field position"
        );
    }

    #[test]
    fn word_operator_in_field_position_after_and() {
        let mut lexer = lexer!("name from . where name = foo and gt > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("="))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("foo"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::And));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("gt"))),
            "gt after AND should be RawString in field position, not Operator"
        );
    }

    #[test]
    fn arithmetic_word_in_field_position_after_open() {
        let mut lexer = lexer!("name from . where (mul > 0)");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("mul"))),
            "mul after ( in WHERE should be RawString in field position, not ArithmeticOperator"
        );
    }

    #[test]
    fn desc_in_select_column_list() {
        let mut lexer = lexer!("select desc, name from .");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Select));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("desc"))),
            "desc in SELECT column list should be RawString like asc, not DescendingOrder"
        );
    }

    #[test]
    fn word_operator_in_curly_brace_set() {
        let mut lexer = lexer!("name from . where name in {eq}");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("in"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::CurlyOpen));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("eq"))),
            "eq inside curly brace set should be RawString, not Operator"
        );
    }

    #[test]
    fn from_in_curly_brace_set_corrupts_state() {
        let mut lexer = lexer!("name from . where name in {from} and size > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("in"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::CurlyOpen));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("from"))),
            "from inside curly brace set should be RawString, not From keyword"
        );

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::CurlyClose));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::And),
            "and after curly brace set should still be recognized"
        );
    }

    #[test]
    fn word_operator_after_comma_in_paren_list() {
        let mut lexer = lexer!("name from . where name in (foo, eq)");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("in"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("foo"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("eq"))),
            "eq after comma in value list should be RawString, not Operator"
        );
    }

    #[test]
    fn from_after_comma_in_paren_list_corrupts_state() {
        let mut lexer = lexer!("name from . where name in (foo, from) and size > 0");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("in"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Open));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("foo"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Comma));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("from"))),
            "from after comma in value list should be RawString, not From keyword"
        );

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Close));
        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::And),
            "and after paren list should still be recognized when from appeared as value"
        );
    }

    #[test]
    fn and_keyword_in_curly_brace_set() {
        let mut lexer = lexer!("name from . where name in {and}");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from("in"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::CurlyOpen));

        assert_eq!(
            lexer.next_lexeme(),
            Some(Lexeme::RawString(String::from("and"))),
            "and inside curly brace set should be RawString, not And keyword"
        );
    }

    #[test]
    fn operator_in_order_by_without_from() {
        let mut lexer = lexer!("name from . where size > 0 order by name = foo");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::From));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("."))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from(">"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("0"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Order));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        let with_from = lexer.next_lexeme();

        let mut lexer = lexer!("name where size > 0 order by name = foo");

        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Where));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("size"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Operator(String::from(">"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("0"))));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::Order));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::By));
        assert_eq!(lexer.next_lexeme(), Some(Lexeme::RawString(String::from("name"))));
        let without_from = lexer.next_lexeme();

        assert_eq!(
            with_from, without_from,
            "= in ORDER BY should tokenize the same with or without explicit FROM"
        );
    }
}

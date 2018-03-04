#[derive(Clone, PartialEq, Debug)]
pub enum Lexem {
    Field(String),
    Comma,
    From,
    Where,
    Operator(String),
    String(String),
    Open,
    Close,
    And,
    Or,
    Limit,
    Into,
}

#[derive(Debug)]
enum LexingMode {
    Undefined,
    Field,
    Comma,
    Operator,
    String,
    Open,
    Close,
}

pub struct Lexer<'a> {
    input: &'a str,
    index: usize
}

impl<'a> Lexer<'a> {
    pub fn new(input: &str) -> Lexer {
        return Lexer { input, index: 0 }
    }

    pub fn next_lexem(&mut self) -> Option<Lexem> {
        let mut s = String::new();
        let mut mode = LexingMode::Undefined;

        for c in self.input.chars().skip(self.index) {
            let stop = match mode {
                LexingMode::Undefined => {
                    self.index += 1;

                    if c == ' ' {
                        // skip spaces
                    } else if c == '\'' {
                        mode = LexingMode::String;
                    } else if c == ',' {
                        mode = LexingMode::Comma;
                    } else if c == '(' {
                        mode = LexingMode::Open;
                    } else if c == ')' {
                        mode = LexingMode::Close;
                    } else if is_op_char(c)  {
                        mode = LexingMode::Operator;
                        s.push(c);
                    } else {
                        mode = LexingMode::Field;
                        s.push(c);
                    }

                    false
                },
                LexingMode::String => {
                    self.index += 1;

                    if c == '\'' {
                        true
                    } else {
                        mode = LexingMode::String;
                        s.push(c);

                        false
                    }
                },
                LexingMode::Operator => {
                    self.index += 1;

                    if is_op_char(c) {
                        mode = LexingMode::Operator;
                        s.push(c);

                        false
                    } else {
                        true
                    }
                },
                LexingMode::Field => {
                    if c == ' ' || c == ',' || c == ')' {
                        true
                    } else {
                        self.index += 1;
                        mode = LexingMode::Field;
                        s.push(c);

                        false
                    }
                },
                LexingMode::Comma => {
                    true
                },
                LexingMode::Open => {
                    true
                },
                LexingMode::Close => {
                    true
                },
            };

            if stop {
                break;
            }
        }

        match mode {
            LexingMode::String => Some(Lexem::String(s)),
            LexingMode::Operator => Some(Lexem::Operator(s)),
            LexingMode::Field => {
                if s.eq_ignore_ascii_case("from")  {
                    Some(Lexem::From)
                } else if s.eq_ignore_ascii_case("where")  {
                    Some(Lexem::Where)
                } else if s.eq_ignore_ascii_case("or")  {
                    Some(Lexem::Or)
                } else if s.eq_ignore_ascii_case("and")  {
                    Some(Lexem::And)
                } else if s.eq_ignore_ascii_case("eq")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("ne")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("gt")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("lt")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("gte")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("lte")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("ge")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("le")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("regexp")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("rx")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("like")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("limit")  {
                    Some(Lexem::Limit)
                } else if s.eq_ignore_ascii_case("into")  {
                    Some(Lexem::Into)
                } else {
                    Some(Lexem::Field(s))
                }
            },
            LexingMode::Comma =>  Some(Lexem::Comma),
            LexingMode::Open =>   Some(Lexem::Open),
            LexingMode::Close =>  Some(Lexem::Close),
            _ => None
        }
    }
}

fn is_op_char(c: char) -> bool {
    match c {
        '=' => true,
        '!' => true,
        '<' => true,
        '>' => true,
        '~' => true,
        _ 	=> false
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexems() {
        let mut lexer = Lexer::new("select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' limit 50");

        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("select"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("path"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("fsize"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::From));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("/test"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("depth"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("2"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("/test2"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("archives"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("/test3"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("depth"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("3"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("archives"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("/test4"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Comma));
        assert_eq!(lexer.next_lexem(), Some(Lexem::String(String::from("/test5"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Where));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("!="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("123"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::And));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Open));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("size"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("gt"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("456"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Or));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("fsize"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("lte"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("758"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Close));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Or));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("name"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Operator(String::from("="))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::String(String::from("xxx"))));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Limit));
        assert_eq!(lexer.next_lexem(), Some(Lexem::Field(String::from("50"))));
    }
}
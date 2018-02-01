#[derive(Clone, Debug)]
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
    input: &'a String,
    index: usize
}

impl<'a> Lexer<'a> {
    pub fn new(input: &String) -> Lexer {
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
                    self.index += 1;

                    if c == ' ' {
                        true
                    } else if c == ',' {
                        true
                    } else {
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
                } else if s.eq_ignore_ascii_case("regexp")  {
                    Some(Lexem::Operator(s))
                } else if s.eq_ignore_ascii_case("rx")  {
                    Some(Lexem::Operator(s))
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
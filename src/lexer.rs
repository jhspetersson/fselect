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

pub fn next_lexem(input: &String, skip_chars: usize) -> Result<(Lexem, usize), &'static str> {
    let mut s = String::new();
    let mut mode = LexingMode::Undefined;
    let mut finished_at = skip_chars;

    for (i, c) in input.chars().skip(skip_chars).enumerate() {
        let mut next_mode = LexingMode::Undefined;
        let (stop, point) = match mode {
            LexingMode::Undefined => {
                if c == ' ' {
                    // skip spaces
                } else if c == '\'' {
                    next_mode = LexingMode::String;
                } else if c == ',' {
                    next_mode = LexingMode::Comma;
                } else if c == '(' {
                    next_mode = LexingMode::Open;
                } else if c == ')' {
                    next_mode = LexingMode::Close;
                } else if is_op_char(c)  {
                    next_mode = LexingMode::Operator;
                    s.push(c);
                } else {
                    next_mode = LexingMode::Field;
                    s.push(c);
                }

                (false, i)
            },
            LexingMode::String => {
                if c == '\'' {
                    (true, i)
                } else {
                    next_mode = LexingMode::String;
                    s.push(c);
                    (false, i)
                }
            },
            LexingMode::Operator => {
                if is_op_char(c) {
                    next_mode = LexingMode::Operator;
                    s.push(c);
                    (false, i)
                } else {
                    (true, i - 1)
                }
            },
            LexingMode::Field => {
                if c == ' ' {
                    (true, i)
                } else if c == ',' {
                    (true, i - 1)
                } else {
                    next_mode = LexingMode::Field;
                    s.push(c);
                    (false, i)
                }
            },
            LexingMode::Comma => {
                (true, i - 1)
            },
            LexingMode::Open => {
                (true, i - 1)
            },
            LexingMode::Close => {
                (true, i - 1)
            },
        };

        if stop == true {
            finished_at = point;
            break;
        } else {
            mode = next_mode;
        }
    }

    let next_start = skip_chars + finished_at + 1;

    match mode {
        LexingMode::String => Ok((Lexem::String(s), next_start)),
        LexingMode::Operator => Ok((Lexem::Operator(s), next_start)),
        LexingMode::Field => {
            if s.eq_ignore_ascii_case("from")  {
                Ok((Lexem::From, next_start))
            } else if s.eq_ignore_ascii_case("where")  {
                Ok((Lexem::Where, next_start))
            } else if s.eq_ignore_ascii_case("or")  {
                Ok((Lexem::Or, next_start))
            } else if s.eq_ignore_ascii_case("and")  {
                Ok((Lexem::And, next_start))
            } else if s.eq_ignore_ascii_case("eq")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("ne")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("gt")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("lt")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("gte")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("lte")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("regexp")  {
                Ok((Lexem::Operator(s), next_start))
            } else if s.eq_ignore_ascii_case("rx")  {
                Ok((Lexem::Operator(s), next_start))
            } else {
                Ok((Lexem::Field(s), next_start))
            }
        },
        LexingMode::Comma =>  Ok((Lexem::Comma, next_start)),
        LexingMode::Open =>   Ok((Lexem::Open, next_start)),
        LexingMode::Close =>  Ok((Lexem::Close, next_start)),
        _ => Err("Error parsing query")
    }
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


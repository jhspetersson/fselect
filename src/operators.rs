use crate::function::Variant;

#[derive(Debug, Clone, Eq, Hash, PartialEq, PartialOrd, Serialize)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, PartialOrd, Serialize)]
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
    NotRx,
    Like,
    NotLike,
}

impl Op {
    pub fn from(text: String) -> Option<Op> {
        match text.to_lowercase().as_str() {
            "=" | "==" | "eq" => Some(Op::Eq),
            "!=" | "<>" | "ne" => Some(Op::Ne),
            "===" | "eeq" => Some(Op::Eeq),
            "!==" | "ene" => Some(Op::Ene),
            ">" | "gt" => Some(Op::Gt),
            ">=" | "gte" | "ge" => Some(Op::Gte),
            "<" | "lt" => Some(Op::Lt),
            "<=" | "lte" | "le" => Some(Op::Lte),
            "~=" | "=~" | "regexp" | "rx" => Some(Op::Rx),
            "!=~" | "!~=" | "notrx" => Some(Op::NotRx),
            "like" => Some(Op::Like),
            "notlike" => Some(Op::NotLike),
            _ => None
        }
    }

    pub fn from_with_not(text: String, not: bool) -> Option<Op> {
        let op = Op::from(text);
        match op {
            Some(op) if not => {
                match op {
                    Op::Eq => Some(Op::Ne),
                    Op::Ne => Some(Op::Eq),
                    Op::Eeq => Some(Op::Ene),
                    Op::Ene => Some(Op::Eeq),
                    Op::Gt => Some(Op::Lt),
                    Op::Lt => Some(Op::Gt),
                    Op::Gte => Some(Op::Lte),
                    Op::Lte => Some(Op::Gte),
                    Op::Rx => Some(Op::NotRx),
                    Op::NotRx => Some(Op::Rx),
                    Op::Like => Some(Op::NotLike),
                    Op::NotLike => Some(Op::Like),
                }
            },
            _ => op
        }
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Divide,
    Multiply,
}

impl ArithmeticOp {
    pub fn from(text: String) -> Option<ArithmeticOp> {
        match text.to_lowercase().as_str() {
            "+" | "plus" => Some(ArithmeticOp::Add),
            "-" | "minus"  => Some(ArithmeticOp::Subtract),
            "*" | "mul" => Some(ArithmeticOp::Multiply),
            "/" | "div" => Some(ArithmeticOp::Divide),
            _ => None
        }
    }

    pub fn calc(&self, left: &Variant, right: &Variant) -> Variant {
        let result = match &self {
            ArithmeticOp::Add => left.to_float() + right.to_float(),
            ArithmeticOp::Subtract => left.to_float() - right.to_float(),
            ArithmeticOp::Multiply => left.to_float() * right.to_float(),
            ArithmeticOp::Divide => left.to_float() / right.to_float(),
        };

        return Variant::from_float(result);
    }
}

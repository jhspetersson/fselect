//! Defines the arithmetic operators used in the query language

use crate::util::Variant;

#[derive(Debug, Clone, Eq, Hash, PartialEq, PartialOrd, Serialize)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq, PartialOrd, Serialize)]
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
    Between,
    NotBetween,
    In,
    NotIn,
    Exists,
    NotExists,
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
            "between" => Some(Op::Between),
            "in" => Some(Op::In),
            "exists" => Some(Op::Exists),
            _ => None,
        }
    }

    pub fn from_with_not(text: String, not: bool) -> Option<Op> {
        let op = Op::from(text);
        match op {
            Some(op) if not => Some(Self::negate(op)),
            _ => op,
        }
    }

    pub fn negate(op: Op) -> Op {
        match op {
            Op::Eq => Op::Ne,
            Op::Ne => Op::Eq,
            Op::Eeq => Op::Ene,
            Op::Ene => Op::Eeq,
            Op::Gt => Op::Lt,
            Op::Lt => Op::Gt,
            Op::Gte => Op::Lte,
            Op::Lte => Op::Gte,
            Op::Rx => Op::NotRx,
            Op::NotRx => Op::Rx,
            Op::Like => Op::NotLike,
            Op::NotLike => Op::Like,
            Op::Between => Op::NotBetween,
            Op::NotBetween => Op::Between,
            Op::In => Op::NotIn,
            Op::NotIn => Op::In,
            Op::Exists => Op::NotExists,
            Op::NotExists => Op::Exists,
        }
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Divide,
    Multiply,
    Modulo,
}

impl ArithmeticOp {
    pub fn from(text: String) -> Option<ArithmeticOp> {
        match text.to_lowercase().as_str() {
            "+" | "plus" => Some(ArithmeticOp::Add),
            "-" | "minus" => Some(ArithmeticOp::Subtract),
            "*" | "mul" => Some(ArithmeticOp::Multiply),
            "/" | "div" => Some(ArithmeticOp::Divide),
            "%" | "mod" => Some(ArithmeticOp::Modulo),
            _ => None,
        }
    }

    pub fn calc(&self, left: &Variant, right: &Variant) -> Variant {
        let result = match &self {
            ArithmeticOp::Add => left.to_float() + right.to_float(),
            ArithmeticOp::Subtract => left.to_float() - right.to_float(),
            ArithmeticOp::Multiply => left.to_float() * right.to_float(),
            ArithmeticOp::Divide => left.to_float() / right.to_float(),
            ArithmeticOp::Modulo => left.to_float() % right.to_float(),
        };

        Variant::from_float(result)
    }
}

//! Defines the arithmetic operators used in the query language

use crate::util::Variant;

#[derive(Debug, Clone, Eq, Hash, PartialEq, PartialOrd, Serialize)]
pub enum LogicalOp {
    And,
    Or,
}

impl LogicalOp {
    pub fn negate(&self) -> LogicalOp {
        match self {
            LogicalOp::And => LogicalOp::Or,
            LogicalOp::Or => LogicalOp::And,
        }
    }
}

// Note: there are no `Between`/`NotBetween` variants. The parser rewrites
// `x between a and b` into `x >= a and x <= b` (and the negation into
// `x < a or x > b`), so a BETWEEN op never reaches evaluation.
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
    In,
    NotIn,
    Exists,
    NotExists,
    IsNull,
    IsNotNull,
}

impl Op {
    pub fn mirrored(&self) -> Option<Op> {
        match self {
            Op::Gt => Some(Op::Lt),
            Op::Gte => Some(Op::Lte),
            Op::Lt => Some(Op::Gt),
            Op::Lte => Some(Op::Gte),
            Op::Eq | Op::Ne | Op::Eeq | Op::Ene => Some(*self),
            _ => None,
        }
    }

    pub fn from(text: &str) -> Option<Op> {
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
            "in" => Some(Op::In),
            "notin" => Some(Op::NotIn),
            "exists" => Some(Op::Exists),
            "notexists" => Some(Op::NotExists),
            "isnull" => Some(Op::IsNull),
            "isnotnull" => Some(Op::IsNotNull),
            _ => None,
        }
    }

    pub fn from_with_not(text: &str, not: bool) -> Option<Op> {
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
            Op::Gt => Op::Lte,
            Op::Lte => Op::Gt,
            Op::Lt => Op::Gte,
            Op::Gte => Op::Lt,
            Op::Rx => Op::NotRx,
            Op::NotRx => Op::Rx,
            Op::Like => Op::NotLike,
            Op::NotLike => Op::Like,
            Op::In => Op::NotIn,
            Op::NotIn => Op::In,
            Op::Exists => Op::NotExists,
            Op::NotExists => Op::Exists,
            Op::IsNull => Op::IsNotNull,
            Op::IsNotNull => Op::IsNull,
        }
    }

    pub fn checks_null(&self) -> bool {
        matches!(self, Op::IsNull | Op::IsNotNull)
    }

    pub fn is_relational(&self) -> bool {
        matches!(self, Op::Eq | Op::Ne | Op::Eeq | Op::Ene | Op::Gt | Op::Gte | Op::Lt | Op::Lte)
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

impl std::fmt::Display for ArithmeticOp {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            ArithmeticOp::Add => "+",
            ArithmeticOp::Subtract => "-",
            ArithmeticOp::Multiply => "*",
            ArithmeticOp::Divide => "/",
            ArithmeticOp::Modulo => "%",
        })
    }
}

impl ArithmeticOp {
    pub fn from(text: &str) -> Option<ArithmeticOp> {
        match text.to_lowercase().as_str() {
            "+" | "plus" => Some(ArithmeticOp::Add),
            "-" | "minus" => Some(ArithmeticOp::Subtract),
            "*" | "mul" => Some(ArithmeticOp::Multiply),
            "/" | "div" => Some(ArithmeticOp::Divide),
            "%" | "mod" => Some(ArithmeticOp::Modulo),
            _ => None,
        }
    }

    pub fn calc(&self, left: &Variant, right: &Variant) -> Result<Variant, String> {
        let right_val = right.to_float();

        if matches!(self, ArithmeticOp::Divide | ArithmeticOp::Modulo) && right_val == 0.0 {
            return Err("Division by zero".to_string());
        }

        let result = match &self {
            ArithmeticOp::Add => left.to_float() + right_val,
            ArithmeticOp::Subtract => left.to_float() - right_val,
            ArithmeticOp::Multiply => left.to_float() * right_val,
            ArithmeticOp::Divide => left.to_float() / right_val,
            ArithmeticOp::Modulo => left.to_float() % right_val,
        };

        if !result.is_finite() {
            return Err(format!("Arithmetic overflow: result is {}", result));
        }

        Ok(Variant::from_float(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_op_displays_as_its_symbol() {
        assert_eq!(ArithmeticOp::Add.to_string(), "+");
        assert_eq!(ArithmeticOp::Subtract.to_string(), "-");
        assert_eq!(ArithmeticOp::Multiply.to_string(), "*");
        assert_eq!(ArithmeticOp::Divide.to_string(), "/");
        assert_eq!(ArithmeticOp::Modulo.to_string(), "%");
    }

    #[test]
    fn op_from_between_is_none() {
        // BETWEEN is rewritten by the parser into >=/<= comparisons and has
        // no Op variant of its own.
        assert_eq!(Op::from("between"), None);
        assert_eq!(Op::from("notbetween"), None);
    }

    #[test]
    fn op_from_notin() {
        assert_eq!(Op::from("notin"), Some(Op::NotIn));
    }

    #[test]
    fn op_from_notexists() {
        assert_eq!(Op::from("notexists"), Some(Op::NotExists));
    }

    #[test]
    fn calc_divide_by_zero_returns_error() {
        let result = ArithmeticOp::Divide.calc(
            &Variant::from_float(1.0),
            &Variant::from_float(0.0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn calc_modulo_by_zero_returns_error() {
        let result = ArithmeticOp::Modulo.calc(
            &Variant::from_float(1.0),
            &Variant::from_float(0.0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn calc_zero_divided_by_zero_returns_error() {
        let result = ArithmeticOp::Divide.calc(
            &Variant::from_float(0.0),
            &Variant::from_float(0.0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn op_from_notlike_exists() {
        assert_eq!(Op::from("notlike"), Some(Op::NotLike));
    }

    #[test]
    fn op_from_notrx_exists() {
        assert_eq!(Op::from("notrx"), Some(Op::NotRx));
    }

    #[test]
    fn op_negate_roundtrip() {
        let ops = vec![
            Op::Eq, Op::Ne, Op::Eeq, Op::Ene, Op::Gt, Op::Gte, Op::Lt, Op::Lte,
            Op::Rx, Op::NotRx, Op::Like, Op::NotLike,
            Op::In, Op::NotIn, Op::Exists, Op::NotExists,
        ];
        for op in ops {
            assert_eq!(Op::negate(Op::negate(op)), op);
        }
    }

    #[test]
    fn op_from_with_not_negates() {
        assert_eq!(Op::from_with_not("eq", true), Some(Op::Ne));
        assert_eq!(Op::from_with_not("eq", false), Some(Op::Eq));
    }

    #[test]
    fn op_from_with_not_unknown_returns_none() {
        assert_eq!(Op::from_with_not("garbage", true), None);
        assert_eq!(Op::from_with_not("garbage", false), None);
    }

    #[test]
    fn arithmetic_op_from_all_variants() {
        assert_eq!(ArithmeticOp::from("+"), Some(ArithmeticOp::Add));
        assert_eq!(ArithmeticOp::from("plus"), Some(ArithmeticOp::Add));
        assert_eq!(ArithmeticOp::from("-"), Some(ArithmeticOp::Subtract));
        assert_eq!(ArithmeticOp::from("minus"), Some(ArithmeticOp::Subtract));
        assert_eq!(ArithmeticOp::from("*"), Some(ArithmeticOp::Multiply));
        assert_eq!(ArithmeticOp::from("mul"), Some(ArithmeticOp::Multiply));
        assert_eq!(ArithmeticOp::from("/"), Some(ArithmeticOp::Divide));
        assert_eq!(ArithmeticOp::from("div"), Some(ArithmeticOp::Divide));
        assert_eq!(ArithmeticOp::from("%"), Some(ArithmeticOp::Modulo));
        assert_eq!(ArithmeticOp::from("mod"), Some(ArithmeticOp::Modulo));
    }

    #[test]
    fn arithmetic_op_from_unknown() {
        assert_eq!(ArithmeticOp::from("garbage"), None);
    }

    #[test]
    fn calc_basic_operations() {
        let a = Variant::from_float(10.0);
        let b = Variant::from_float(3.0);
        assert_eq!(ArithmeticOp::Add.calc(&a, &b).unwrap().to_float(), 13.0);
        assert_eq!(ArithmeticOp::Subtract.calc(&a, &b).unwrap().to_float(), 7.0);
        assert_eq!(ArithmeticOp::Multiply.calc(&a, &b).unwrap().to_float(), 30.0);
        let div = ArithmeticOp::Divide.calc(&a, &b).unwrap().to_float();
        assert!((div - 10.0 / 3.0).abs() < 1e-10);
        assert_eq!(ArithmeticOp::Modulo.calc(&a, &b).unwrap().to_float(), 1.0);
    }

    #[test]
    fn calc_add_overflow_returns_error() {
        let result = ArithmeticOp::Add.calc(
            &Variant::from_float(f64::MAX),
            &Variant::from_float(f64::MAX),
        );
        assert!(result.is_err(), "f64::MAX + f64::MAX should error, not silently become 0");
    }

    #[test]
    fn calc_multiply_overflow_returns_error() {
        let result = ArithmeticOp::Multiply.calc(
            &Variant::from_float(f64::MAX),
            &Variant::from_float(2.0),
        );
        assert!(result.is_err(), "f64::MAX * 2 should error, not silently become 0");
    }

    #[test]
    fn calc_subtract_overflow_returns_error() {
        let result = ArithmeticOp::Subtract.calc(
            &Variant::from_float(f64::MAX),
            &Variant::from_float(-f64::MAX),
        );
        assert!(result.is_err(), "f64::MAX - (-f64::MAX) should error, not silently become 0");
    }

    #[test]
    fn logical_op_negate() {
        assert_eq!(LogicalOp::And.negate(), LogicalOp::Or);
        assert_eq!(LogicalOp::Or.negate(), LogicalOp::And);
    }
}

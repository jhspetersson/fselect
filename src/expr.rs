use std::collections::HashSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::field::Field;
use crate::function::Function;
use crate::operators::ArithmeticOp;
use crate::operators::LogicalOp;
use crate::operators::Op;

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
pub struct Expr {
    pub left: Option<Box<Expr>>,
    pub arithmetic_op: Option<ArithmeticOp>,
    pub logical_op: Option<LogicalOp>,
    pub op: Option<Op>,
    pub right: Option<Box<Expr>>,
    pub minus: bool,
    pub field: Option<Field>,
    pub function: Option<Function>,
    pub args: Option<Vec<Expr>>,
    pub val: Option<String>,
}

impl Expr {
    pub fn op(left: Expr, op: Op, right: Expr) -> Expr {
        Expr {
            left: Some(Box::new(left)),
            arithmetic_op: None,
            logical_op: None,
            op: Some(op),
            right: Some(Box::new(right)),
            minus: false,
            field: None,
            function: None,
            args: None,
            val: None,
        }
    }

    pub fn logical_op(left: Expr, logical_op: LogicalOp, right: Expr) -> Expr {
        Expr {
            left: Some(Box::new(left)),
            arithmetic_op: None,
            logical_op: Some(logical_op),
            op: None,
            right: Some(Box::new(right)),
            minus: false,
            field: None,
            function: None,
            args: None,
            val: None,
        }
    }

    pub fn arithmetic_op(left: Expr, arithmetic_op: ArithmeticOp, right: Expr) -> Expr {
        Expr {
            left: Some(Box::new(left)),
            arithmetic_op: Some(arithmetic_op),
            logical_op: None,
            op: None,
            right: Some(Box::new(right)),
            minus: false,
            field: None,
            function: None,
            args: None,
            val: None,
        }
    }

    pub fn field(field: Field) -> Expr {
        Expr {
            left: None,
            arithmetic_op: None,
            logical_op: None,
            op: None,
            right: None,
            minus: false,
            field: Some(field),
            function: None,
            args: None,
            val: None,
        }
    }

    pub fn function(function: Function) -> Expr {
        Expr {
            left: None,
            arithmetic_op: None,
            logical_op: None,
            op: None,
            right: None,
            minus: false,
            field: None,
            function: Some(function),
            args: Some(vec![]),
            val: None,
        }
    }

    pub fn function_left(function: Function, left: Option<Box<Expr>>) -> Expr {
        Expr {
            left,
            arithmetic_op: None,
            logical_op: None,
            op: None,
            right: None,
            minus: false,
            field: None,
            function: Some(function),
            args: Some(vec![]),
            val: None,
        }
    }

    pub fn value(value: String) -> Expr {
        Expr {
            left: None,
            arithmetic_op: None,
            logical_op: None,
            op: None,
            right: None,
            minus: false,
            field: None,
            function: None,
            args: None,
            val: Some(value),
        }
    }

    pub fn has_aggregate_function(&self) -> bool {
        if let Some(ref left) = self.left {
            if left.has_aggregate_function() {
                return true;
            }
        }

        if let Some(ref right) = self.right {
            if right.has_aggregate_function() {
                return true;
            }
        }

        if let Some(ref function) = self.function {
            if function.is_aggregate_function() {
                return true;
            }
        }

        if let Some(ref args) = self.args {
            for arg in args {
                if arg.has_aggregate_function() {
                    return true;
                }
            }
        }

        false
    }

    pub fn get_required_fields(&self) -> HashSet<Field> {
        let mut result = HashSet::new();

        if let Some(ref left) = self.left {
            result.extend(left.get_required_fields());
        }

        if let Some(ref right) = self.right {
            result.extend(right.get_required_fields());
        }

        if let Some(field) = self.field {
            result.insert(field);
        }

        if let Some(ref args) = self.args {
            for arg in args {
                result.extend(arg.get_required_fields());
            }
        }

        result
    }

    pub fn contains_numeric(&self) -> bool {
        Self::contains_numeric_field(self)
    }

    fn contains_numeric_field(expr: &Expr) -> bool {
        let field = match expr.field {
            Some(ref field) => field.is_numeric_field(),
            None => false,
        };

        if field {
            return true;
        }

        let function = match expr.function {
            Some(ref function) => function.is_numeric_function(),
            None => false,
        };

        if function {
            return true;
        }

        match expr.left {
            Some(ref left) => Self::contains_numeric_field(left),
            None => false,
        }
    }

    pub fn contains_datetime(&self) -> bool {
        Self::contains_datetime_field(self)
    }

    fn contains_datetime_field(expr: &Expr) -> bool {
        let field = match expr.field {
            Some(ref field) => field.is_datetime_field(),
            None => false,
        };

        if field {
            return true;
        }

        match expr.left {
            Some(ref left) => Self::contains_datetime_field(left),
            None => false,
        }
    }

    pub fn contains_colorized(&self) -> bool {
        Self::contains_colorized_field(self)
    }

    fn contains_colorized_field(expr: &Expr) -> bool {
        if expr.function.is_some() {
            return false;
        }

        let field = match expr.field {
            Some(ref field) => field.is_colorized_field(),
            None => false,
        };

        if field {
            return true;
        }

        match expr.left {
            Some(ref left) => Self::contains_colorized_field(left),
            None => false,
        }
    }
}

impl Display for Expr {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        use std::fmt::Write;

        if self.minus {
            fmt.write_char('-')?;
        }

        if let Some(ref function) = self.function {
            fmt.write_str(&function.to_string())?;
            fmt.write_char('(')?;
            if let Some(ref left) = self.left {
                fmt.write_str(&left.to_string())?;
            }
            fmt.write_char(')')?;
        } else if let Some(ref left) = self.left {
            fmt.write_str(&left.to_string())?;
        }

        if let Some(ref field) = self.field {
            fmt.write_str(&field.to_string())?;
        }

        if let Some(ref val) = self.val {
            fmt.write_str(val)?;
        }

        if let Some(ref right) = self.right {
            fmt.write_str(&right.to_string())?;
        }

        Ok(())
    }
}

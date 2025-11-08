use std::collections::HashSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::field::Field;
use crate::function::Function;
use crate::operators::ArithmeticOp;
use crate::operators::LogicalOp;
use crate::operators::Op;
use crate::query::Query;

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
    pub subquery: Option<Box<Query>>,
    pub root_alias: Option<String>,
    pub weight: i32,
}

impl Expr {
    pub fn new() -> Expr {
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
            val: None,
            subquery: None,
            root_alias: None,
            weight: 0,
        }
    }
    
    pub fn op(left: Expr, op: Op, right: Expr) -> Expr {
        let left_weight = left.weight;
        let right_weight = right.weight;

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
            subquery: None,
            root_alias: None,
            weight: left_weight + right_weight,
        }
    }

    pub fn logical_op(left: Expr, logical_op: LogicalOp, right: Expr) -> Expr {
        let left_weight = left.weight;
        let right_weight = right.weight;

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
            subquery: None,
            root_alias: None,
            weight: left_weight + right_weight,
        }
    }

    pub fn arithmetic_op(left: Expr, arithmetic_op: ArithmeticOp, right: Expr) -> Expr {
        let left_weight = left.weight;
        let right_weight = right.weight;

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
            subquery: None,
            root_alias: None,
            weight: left_weight + right_weight,
        }
    }

    pub fn field(field: Field) -> Expr {
        let weight = field.get_weight();
        
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
            subquery: None,
            root_alias: None,
            weight,
        }
    }

    pub fn field_with_root_alias(field: Field, root_alias: Option<String>) -> Expr {
        let weight = field.get_weight();

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
            subquery: None,
            root_alias,
            weight,
        }
    }

    pub fn function(function: Function) -> Expr {
        let weight = function.get_weight();

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
            subquery: None,
            root_alias: None,
            weight,
        }
    }

    pub fn function_left(function: Function, left: Option<Box<Expr>>) -> Expr {
        let weight = function.get_weight();
        let left_weight = match left {
            Some(ref expr) => expr.weight,
            None => 0,
        };
        
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
            subquery: None,
            root_alias: None,
            weight: weight + left_weight,
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
            subquery: None,
            root_alias: None,
            weight: 0,
        }
    }
    
    pub fn subquery(subquery: Query) -> Expr {
        let weight = match subquery.expr {
            Some(ref expr) => expr.weight,
            None => 0,
        };
        
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
            val: None,
            subquery: Some(Box::new(subquery)),
            root_alias: None,
            weight,
        }
    }
    
    pub fn add_left(&mut self, left: Expr) {
        let left_weight = left.weight;
        self.left = Some(Box::new(left));
        self.weight += left_weight;
    }
    
    pub fn set_args(&mut self, args: Vec<Expr>) {
        let mut args_weight = 0;
        for arg in &args {
            args_weight += arg.weight;
        }
        self.args = Some(args);
        self.weight += args_weight;
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
            if let Some(ref root_alias) = self.root_alias {
                fmt.write_str(&root_alias.to_string())?;
                fmt.write_char('.')?;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;
    use crate::function::Function;

    #[test]
    fn test_weight() {
        let expr = Expr::field(Field::Name);
        assert_eq!(expr.weight, 0);
        
        let expr = Expr::field(Field::Accessed);
        assert_eq!(expr.weight, 1);
        
        let expr = Expr::function(Function::Concat);
        assert_eq!(expr.weight, 0);
        
        let expr = Expr::function(Function::Contains);
        assert_eq!(expr.weight, 1024);
        
        let expr = Expr::function_left(Function::Contains, Some(Box::new(Expr::value("foo".to_string()))));
        assert_eq!(expr.weight, 1024);
        
        let expr = Expr::logical_op(
            Expr::op(
                Expr::field(Field::Size),
                Op::Gt,
                Expr::value(String::from("456")),
            ),
            LogicalOp::Or,
            Expr::op(
                Expr::field(Field::FormattedSize),
                Op::Lte,
                Expr::value(String::from("758")),
            ),
        );
        assert_eq!(expr.weight, 2);
        
        let expr = Expr::logical_op(
            Expr::logical_op(
                Expr::op(
                    Expr::field(Field::Name),
                    Op::Ne,
                    Expr::value(String::from("123")),
                ),
                LogicalOp::And,
                Expr::logical_op(
                    Expr::op(
                        Expr::field(Field::Size),
                        Op::Gt,
                        Expr::value(String::from("456")),
                    ),
                    LogicalOp::Or,
                    Expr::op(
                        Expr::field(Field::FormattedSize),
                        Op::Lte,
                        Expr::value(String::from("758")),
                    ),
                ),
            ),
            LogicalOp::Or,
            Expr::op(
                Expr::field(Field::Name),
                Op::Eq,
                Expr::value(String::from("xxx")),
            ),
        );
        assert_eq!(expr.weight, 2);
    }
}
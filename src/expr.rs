use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::{Deref, DerefMut};
use crate::field::Field;
use crate::function::Function;
use crate::operators::ArithmeticOp;
use crate::operators::LogicalOp;
use crate::operators::Op;
use crate::query::Query;
use crate::util::{str_to_bool, Variant};

pub const IS_FILE: &str = "is_file";
pub const IS_DIR: &str = "is_dir";
pub const IS_PIPE: &str = "is_pipe";
pub const IS_CHAR: &str = "is_char";
pub const IS_BLOCK: &str = "is_block";
pub const IS_SOCKET: &str = "is_socket";
pub const FILE_TYPE_PROPS: [&str; 6] = [IS_FILE, IS_DIR, IS_PIPE, IS_CHAR, IS_BLOCK, IS_SOCKET];
const LOCAL_ROOT: &str = "local_root";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    pub alias: Option<String>,
    pub weight: i32,
    #[serde(skip)]
    pub props: ExprProps,
}

#[derive(Debug, Clone, Default)]
pub struct ExprProps(HashMap<String, Variant>);

impl ExprProps {
    /// A field under a foreign root alias (`t1.is_dir` inside a correlated
    /// subquery) describes a different entry than the bare field, so its
    /// props are keyed `alias.name` and never clash with the entry's own type.
    fn scoped_key(scope: Option<&str>, key: &str) -> String {
        match scope {
            Some(scope) => format!("{}.{}", scope, key),
            None => key.to_string(),
        }
    }

    fn set_file_type(&mut self, scope: Option<&str>, key: &'static str, value: bool) {
        self.insert(Self::scoped_key(scope, key), Variant::from_bool(value));
        if value {
            for other in FILE_TYPE_PROPS.iter().filter(|other| **other != key) {
                self.insert(Self::scoped_key(scope, other), Variant::from_bool(false));
            }
        }
    }

    /// The file type these props assert, as `(scope, key)`.
    fn asserted_file_type(&self) -> Option<(Option<String>, &'static str)> {
        self.iter()
            .filter(|(_, value)| value.to_bool())
            .find_map(|(key, _)| {
                let (scope, name) = match key.rsplit_once('.') {
                    Some((scope, name)) => (Some(scope.to_string()), name),
                    None => (None, key.as_str()),
                };
                FILE_TYPE_PROPS.iter().copied().find(|prop| *prop == name).map(|prop| (scope, prop))
            })
    }

    fn is_file_type_key(key: &str) -> bool {
        let name = key.rsplit_once('.').map_or(key, |(_, name)| name);
        FILE_TYPE_PROPS.contains(&name)
    }

    pub fn clear_file_types(&mut self) {
        self.retain(|key, _| !Self::is_file_type_key(key));
    }

    pub fn mark_local_root(&mut self) {
        self.insert(LOCAL_ROOT.to_string(), Variant::from_bool(true));
    }

    pub fn is_local_root(&self) -> bool {
        self.get(LOCAL_ROOT).is_some_and(Variant::to_bool)
    }
}

impl Deref for ExprProps {
    type Target = HashMap<String, Variant>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ExprProps {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl PartialEq for ExprProps {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ExprProps {}


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
            alias: None,
            weight: 0,
            props: ExprProps::default(),
        }
    }
    
    pub fn op(left: Expr, op: Op, right: Expr) -> Expr {
        let left_weight = left.weight;
        let right_weight = right.weight;
        let props = Self::comparison_props(&left, op, &right);

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
            alias: None,
            weight: left_weight + right_weight,
            props,
        }
    }

    fn comparison_props(left: &Expr, op: Op, right: &Expr) -> ExprProps {
        let mut props = ExprProps::default();
        if let Some((field_expr, value)) = Self::bool_field_comparison(left, op, right)
            && let Some((scope, key)) = field_expr.props.asserted_file_type() {
            props.set_file_type(scope.as_deref(), key, value);
        }

        props
    }

    fn field_props(field: &Field, scope: Option<&str>) -> ExprProps {
        let mut props = ExprProps::default();
        if let Some(key) = Self::file_type_key(field) {
            props.set_file_type(scope, key, true);
        }

        props
    }

    /// For `field = literal` / `literal != field` style comparisons, the field
    /// operand (whose props carry its alias scope) and the asserted value.
    fn bool_field_comparison<'a>(left: &'a Expr, op: Op, right: &'a Expr) -> Option<(&'a Expr, bool)> {
        let negated = match op {
            Op::Eq | Op::Eeq => false,
            Op::Ne | Op::Ene => true,
            _ => return None,
        };
        let (field_expr, literal_expr) = match (left.field, right.field) {
            (Some(_), None) => (left, right),
            (None, Some(_)) => (right, left),
            _ => return None,
        };
        let literal = literal_expr.bool_literal()?;

        Some((field_expr, literal != negated))
    }

    fn file_type_key(field: &Field) -> Option<&'static str> {
        match field {
            Field::IsFile => Some(IS_FILE),
            Field::IsDir => Some(IS_DIR),
            Field::IsPipe => Some(IS_PIPE),
            Field::IsCharacterDevice => Some(IS_CHAR),
            Field::IsBlockDevice => Some(IS_BLOCK),
            Field::IsSocket => Some(IS_SOCKET),
            _ => None,
        }
    }

    fn bool_literal(&self) -> Option<bool> {
        if self.field.is_some() || self.function.is_some() || self.subquery.is_some()
            || self.left.is_some() || self.right.is_some() || self.minus {
            return None;
        }
        self.val.as_deref().and_then(str_to_bool)
    }

    #[allow(unused)]
    pub fn bool_prop(&self, key: &str) -> Option<bool> {
        self.get_prop(key).map(|v| v.to_bool())
    }


    pub fn logical_op(left: Expr, logical_op: LogicalOp, right: Expr) -> Result<Expr, String> {
        Self::build_logical_op(left, logical_op, right, true)
    }

    /// Combines operands that sit under a NOT. A contradiction there
    /// (`not (is_file and is_dir)`) is a tautology, not a query that can
    /// never match, so conflicting file-type props are dropped instead of
    /// rejected.
    pub fn negated_logical_op(left: Expr, logical_op: LogicalOp, right: Expr) -> Expr {
        Self::build_logical_op(left, logical_op, right, false)
            .expect("conflicts are not reported for negated expressions")
    }

    fn build_logical_op(left: Expr, logical_op: LogicalOp, right: Expr, reject_conflicts: bool) -> Result<Expr, String> {
        let left_weight = left.weight;
        let right_weight = right.weight;
        let props = Self::merge_props(&left, &logical_op, &right, reject_conflicts)?;

        let expr = Expr {
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
            alias: None,
            weight: left_weight + right_weight,
            props,
        };

        Ok(expr)
    }

    fn merge_props(left: &Expr, logical_op: &LogicalOp, right: &Expr, reject_conflicts: bool) -> Result<ExprProps, String> {
        let mut props = ExprProps::default();

        let mut left_keys: Vec<&String> = left.props.keys().collect();
        left_keys.sort();

        for key in left_keys {
            let left_value = &left.props[key];
            match right.props.get(key) {
                Some(right_value) if right_value != left_value => {
                    if *logical_op == LogicalOp::And && reject_conflicts {
                        return Err(format!(
                            "Conflicting conditions: '{}' requires {} = {}, but '{}' requires {} = {}",
                            left, key, left_value, right, key, right_value
                        ));
                    }
                }
                Some(_) => {
                    props.insert(key.clone(), left_value.clone());
                }
                None if *logical_op == LogicalOp::And => {
                    props.insert(key.clone(), left_value.clone());
                }
                None => {}
            }
        }

        if *logical_op == LogicalOp::And {
            for (key, right_value) in right.props.iter() {
                if !left.props.contains_key(key) {
                    props.insert(key.clone(), right_value.clone());
                }
            }
        }

        Ok(props)
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
            alias: None,
            weight: left_weight + right_weight,
            props: ExprProps::default(),
        }
    }

    pub fn field(field: Field) -> Expr {
        Self::field_with_root_alias(field, None)
    }

    /// A field of the query's own entry, optionally written with the local
    /// root alias (`t1.size` where `t1` is this query's root).
    pub fn field_with_root_alias(field: Field, root_alias: Option<String>) -> Expr {
        let props = Self::field_props(&field, None);
        Self::field_expr(field, root_alias, props)
    }

    /// A field of a different entry: a correlated reference to an outer
    /// query's root alias. Its file-type props are scoped to that alias so
    /// they are not confused with the current entry's own type.
    pub fn external_field(field: Field, root_alias: String) -> Expr {
        let props = Self::field_props(&field, Some(&root_alias));
        Self::field_expr(field, Some(root_alias), props)
    }

    fn field_expr(field: Field, root_alias: Option<String>, props: ExprProps) -> Expr {
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
            alias: None,
            weight,
            props,
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
            alias: None,
            weight,
            props: ExprProps::default(),
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
            alias: None,
            weight: weight + left_weight,
            props: ExprProps::default(),
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
            alias: None,
            weight: 0,
            props: ExprProps::default(),
        }
    }
    
    pub const SUBQUERY_SCAN_WEIGHT: i32 = 1024;
    pub const CORRELATED_SUBQUERY_WEIGHT: i32 = 1 << 20;

    pub fn find_external_alias(&self, own: &HashSet<String>) -> Option<String> {
        if let Some(ref alias) = self.root_alias
            && !own.contains(alias) {
                return Some(alias.clone());
            }
        if let Some(ref left) = self.left
            && let Some(alias) = left.find_external_alias(own) { return Some(alias); }
        if let Some(ref right) = self.right
            && let Some(alias) = right.find_external_alias(own) { return Some(alias); }
        if let Some(ref args) = self.args
            && let Some(alias) = args.iter().find_map(|a| a.find_external_alias(own)) { return Some(alias); }
        if let Some(ref sub) = self.subquery {
            let nested_own: HashSet<String> = sub.own_root_aliases()
                .into_iter()
                .chain(own.iter().cloned())
                .collect();
            if let Some(alias) = sub.find_external_alias(&nested_own) {
                return Some(alias);
            }
        }
        None
    }

    pub fn subquery(subquery: Query) -> Expr {
        let mut weight = Self::SUBQUERY_SCAN_WEIGHT + subquery.expr.as_ref().map_or(0, |expr| expr.weight);
        if subquery.is_correlated() {
            weight += Self::CORRELATED_SUBQUERY_WEIGHT;
        }
        
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
            alias: None,
            weight,
            props: ExprProps::default(),
        }
    }
    
    pub fn add_left(&mut self, left: Expr) {
        let old_weight = self.left.as_ref().map_or(0, |l| l.weight);
        let left_weight = left.weight;
        self.left = Some(Box::new(left));
        self.weight = self.weight - old_weight + left_weight;
    }
    
    pub fn set_args(&mut self, args: Vec<Expr>) {
        let old_weight: i32 = self.args.as_ref().map_or(0, |a| a.iter().map(|e| e.weight).sum());
        let mut args_weight = 0;
        for arg in &args {
            args_weight += arg.weight;
        }
        self.args = Some(args);
        self.weight = self.weight - old_weight + args_weight;
    }

    #[allow(unused)]
    pub fn set_prop<K: Into<String>>(&mut self, name: K, value: Variant) -> Option<Variant> {
        self.props.insert(name.into(), value)
    }

    #[allow(unused)]
    pub fn get_prop(&self, name: &str) -> Option<&Variant> {
        self.props.get(name)
    }




    pub fn has_aggregate_function(&self) -> bool {
        if let Some(ref left) = self.left
            && left.has_aggregate_function() {
                return true;
            }

        if let Some(ref right) = self.right
            && right.has_aggregate_function() {
                return true;
            }

        if let Some(ref function) = self.function
            && function.is_aggregate_function() {
                return true;
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

    pub fn get_non_aggregated_fields(&self) -> HashSet<Field> {
        let mut result = HashSet::new();
        Self::collect_non_aggregated_fields(self, &mut result);
        result
    }

    fn collect_non_aggregated_fields(expr: &Expr, result: &mut HashSet<Field>) {
        if let Some(ref function) = expr.function
            && function.is_aggregate_function() {
                return;
            }

        if let Some(field) = expr.field {
            result.insert(field);
        }

        if let Some(ref left) = expr.left {
            Self::collect_non_aggregated_fields(left, result);
        }

        if let Some(ref right) = expr.right {
            Self::collect_non_aggregated_fields(right, result);
        }

        if let Some(ref args) = expr.args {
            for arg in args {
                Self::collect_non_aggregated_fields(arg, result);
            }
        }
    }

    pub fn get_aggregate_exprs(&self) -> Vec<&Expr> {
        let mut result = vec![];
        Self::collect_aggregate_exprs(self, &mut result);
        result
    }

    fn collect_aggregate_exprs<'a>(expr: &'a Expr, result: &mut Vec<&'a Expr>) {
        if let Some(ref function) = expr.function
            && function.is_aggregate_function() {
                result.push(expr);
                return;
            }

        if let Some(ref left) = expr.left {
            Self::collect_aggregate_exprs(left, result);
        }

        if let Some(ref right) = expr.right {
            Self::collect_aggregate_exprs(right, result);
        }

        if let Some(ref args) = expr.args {
            for arg in args {
                Self::collect_aggregate_exprs(arg, result);
            }
        }
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
    
    pub fn get_fields_required_in_subqueries(&self, alias: &str, parent_subquery: bool) -> HashMap<Field, HashSet<String>> {
        let mut result = HashMap::new();

        if let Some(ref subquery) = self.subquery {
            Self::collect_fields_required_from_query(subquery, alias, &mut result);
        }

        for child in self.left.iter().chain(self.right.iter()) {
            Self::merge_required_fields(&mut result, child.get_fields_required_in_subqueries(alias, parent_subquery));
        }
        for arg in self.args.iter().flatten() {
            Self::merge_required_fields(&mut result, arg.get_fields_required_in_subqueries(alias, parent_subquery));
        }

        if let Some(ref expr_alias) = self.root_alias
            && expr_alias == alias
                && let Some(field) = self.field
                    && parent_subquery {
                        let key = self.alias.clone().unwrap_or_else(|| field.to_string());
                        result.entry(field).or_default().insert(key);
                    }

        result
    }

    pub fn get_alias_references_in_subqueries(&self, alias: &str, parent_subquery: bool) -> HashSet<String> {
        let mut result = HashSet::new();
        Self::collect_alias_references(self, alias, parent_subquery, &mut result);
        result
    }

    fn collect_alias_references(expr: &Expr, alias: &str, parent_subquery: bool, result: &mut HashSet<String>) {
        if parent_subquery
            && let Some((root_alias, name)) = expr.alias_reference_key()
            && root_alias == alias
        {
            result.insert(name.to_string());
        }
        if let Some(ref subquery) = expr.subquery {
            Self::collect_alias_references_from_query(subquery, alias, result);
        }
        for child in expr.left.iter().chain(expr.right.iter()) {
            Self::collect_alias_references(child, alias, parent_subquery, result);
        }
        for arg in expr.args.iter().flatten() {
            Self::collect_alias_references(arg, alias, parent_subquery, result);
        }
    }

    fn collect_alias_references_from_query(query: &Query, alias: &str, result: &mut HashSet<String>) {
        let clauses = query
            .fields
            .iter()
            .chain(query.expr.iter())
            .chain(query.ordering_fields.iter())
            .chain(query.grouping_fields.iter())
            .chain(query.having.iter());
        for expr in clauses {
            Self::collect_alias_references(expr, alias, true, result);
        }
        for root in &query.roots {
            if let Some(ref inner) = root.subquery {
                Self::collect_alias_references_from_query(inner, alias, result);
            }
        }
    }

    pub fn merge_required_fields(into: &mut HashMap<Field, HashSet<String>>, from: HashMap<Field, HashSet<String>>) {
        for (field, keys) in from {
            into.entry(field).or_default().extend(keys);
        }
    }

    fn collect_fields_required_from_query(query: &Query, alias: &str, result: &mut HashMap<Field, HashSet<String>>) {
        let clauses = query
            .fields
            .iter()
            .chain(query.expr.iter())
            .chain(query.ordering_fields.iter())
            .chain(query.grouping_fields.iter())
            .chain(query.having.iter());
        for expr in clauses {
            Self::merge_required_fields(result, expr.get_fields_required_in_subqueries(alias, true));
        }
        for root in &query.roots {
            if let Some(ref inner) = root.subquery {
                Self::collect_fields_required_from_query(inner, alias, result);
            }
        }
    }

    const REFERENCED_TYPE: &'static str = "referenced_type";

    /// An alias reference (`t.sz`) is resolved at run time from the row
    /// context, so the type of the column it names is recorded here for the
    /// static type checks.
    pub fn remember_referenced_type(&mut self, column: &Expr) {
        let kind = if column.contains_numeric() {
            "numeric"
        } else if column.contains_datetime() {
            "datetime"
        } else if column.contains_boolean() {
            "boolean"
        } else {
            return;
        };
        self.set_prop(Self::REFERENCED_TYPE, Variant::from_string(&kind.to_string()));
    }

    fn references_type(&self, kind: &str) -> bool {
        self.alias_reference_key().is_some()
            && self.get_prop(Self::REFERENCED_TYPE).is_some_and(|value| value.to_string() == kind)
    }

    pub fn contains_numeric(&self) -> bool {
        Self::contains_numeric_field(self)
    }

    fn contains_numeric_field(expr: &Expr) -> bool {
        if expr.arithmetic_op.is_some() || expr.references_type("numeric") {
            return true;
        }

        if let Some(ref subquery) = expr.subquery
            && let Some(column) = subquery.fields.first()
        {
            return Self::contains_numeric_field(column);
        }

        if let Some(ref function) = expr.function
            && !function.passes_argument_type_through()
        {
            if function.is_numeric_function() {
                return true;
            }
            if function.is_datetime_function() || function.is_string_function() || function.is_boolean_function() {
                return false;
            }
        }

        let field = match expr.field {
            Some(ref field) => field.is_numeric_field(),
            None => false,
        };

        if field {
            return true;
        }

        if let Some(ref left) = expr.left
            && Self::contains_numeric_field(left) {
                return true;
            }

        if let Some(ref right) = expr.right
            && Self::contains_numeric_field(right) {
                return true;
            }

        if let Some(ref args) = expr.args {
            for arg in args {
                if Self::contains_numeric_field(arg) {
                    return true;
                }
            }
        }

        false
    }

    pub fn contains_boolean(&self) -> bool {
        Self::contains_boolean_field(self)
    }

    fn contains_boolean_field(expr: &Expr) -> bool {
        if expr.arithmetic_op.is_some() {
            return false;
        }
        if expr.references_type("boolean") {
            return true;
        }

        if let Some(ref subquery) = expr.subquery
            && let Some(column) = subquery.fields.first()
        {
            return Self::contains_boolean_field(column);
        }

        if let Some(ref function) = expr.function
            && !function.passes_argument_type_through()
        {
            if function.is_boolean_function() {
                return true;
            }
            if function.is_numeric_function() || function.is_string_function() || function.is_datetime_function() {
                return false;
            }
        }

        if expr.field.is_some_and(|field| field.is_boolean_field()) {
            return true;
        }

        expr.left.as_deref().is_some_and(Self::contains_boolean_field)
            || expr.right.as_deref().is_some_and(Self::contains_boolean_field)
            || expr.args.as_ref().is_some_and(|args| args.iter().any(Self::contains_boolean_field))
    }

    pub fn contains_datetime(&self) -> bool {
        Self::contains_datetime_field(self)
    }

    fn contains_datetime_field(expr: &Expr) -> bool {
        if expr.references_type("datetime") {
            return true;
        }

        if let Some(ref subquery) = expr.subquery
            && let Some(column) = subquery.fields.first()
        {
            return Self::contains_datetime_field(column);
        }

        if let Some(ref function) = expr.function
            && !function.passes_argument_type_through()
        {
            if function.is_datetime_function() {
                return true;
            }
            if function.is_numeric_function() || function.is_string_function() || function.is_boolean_function() {
                return false;
            }
        }

        let field = match expr.field {
            Some(ref field) => field.is_datetime_field(),
            None => false,
        };

        if field {
            return true;
        }

        if let Some(ref left) = expr.left
            && Self::contains_datetime_field(left) {
                return true;
            }

        if let Some(ref right) = expr.right
            && Self::contains_datetime_field(right) {
                return true;
            }

        if let Some(ref args) = expr.args {
            for arg in args {
                if Self::contains_datetime_field(arg) {
                    return true;
                }
            }
        }

        false
    }

    /// Walks the expression tree and validates string literals that are
    /// compared against datetime fields. Returns `Err` with the first
    /// unparseable literal so the parser can fail the query once, up front,
    /// rather than emitting one parse error per file scanned.
    ///
    /// Only literals adjacent to a datetime *field* are checked — comparisons
    /// against datetime-returning function results are not validated here
    /// because that would require tracking function return types. Per-file
    /// degradation in `conforms()` still catches those silently.
    pub fn validate_boolean_literals(&self) -> Result<(), String> {
        if self.op.as_ref().is_some_and(|op| !op.checks_null()) {
            let boolean_side = |side: &Option<Box<Expr>>| side.as_deref().and_then(|e| e.field).filter(Field::is_boolean_field);
            if let Some(field) = boolean_side(&self.left)
                && let Some(ref right) = self.right
            {
                Self::check_bool_value_side(right, &field)?;
            }
            if let Some(field) = boolean_side(&self.right)
                && let Some(ref left) = self.left
            {
                Self::check_bool_value_side(left, &field)?;
            }
        }
        for child in self.left.iter().chain(self.right.iter()) {
            child.validate_boolean_literals()?;
        }
        for arg in self.args.iter().flatten() {
            arg.validate_boolean_literals()?;
        }
        if let Some(ref subquery) = self.subquery
            && let Some(ref inner) = subquery.expr
        {
            inner.validate_boolean_literals()?;
        }
        Ok(())
    }

    fn check_bool_value_side(expr: &Expr, field: &Field) -> Result<(), String> {
        if expr.function.is_some() {
            // a function result is data-dependent; its arguments are not
            // literals compared with the field (`concat('tr', 'ue')`)
            return Ok(());
        }
        for arg in expr.args.iter().flatten() {
            Self::check_bool_value_side(arg, field)?;
        }
        if expr.is_literal()
            && let Some(ref val) = expr.val
            && crate::util::str_to_bool(val).is_none()
        {
            return Err(format!("Can't parse boolean '{}' for field {}", val, field.to_string().to_lowercase()));
        }
        Ok(())
    }

    pub fn validate_datetime_literals(&self) -> Result<(), String> {
        Self::validate_dt_node(self)
    }

    fn validate_dt_node(expr: &Expr) -> Result<(), String> {
        if expr.op.as_ref().is_some_and(|op| !op.checks_null()) {
            let left_is_dt = expr
                .left
                .as_deref()
                .and_then(|l| l.field.as_ref())
                .is_some_and(Field::is_datetime_field);
            let right_is_dt = expr
                .right
                .as_deref()
                .and_then(|r| r.field.as_ref())
                .is_some_and(Field::is_datetime_field);

            if left_is_dt
                && let Some(ref right) = expr.right {
                    Self::check_dt_value_side(right)?;
                }
            if right_is_dt
                && let Some(ref left) = expr.left {
                    Self::check_dt_value_side(left)?;
                }
        }

        if let Some(ref left) = expr.left {
            Self::validate_dt_node(left)?;
        }
        if let Some(ref right) = expr.right {
            Self::validate_dt_node(right)?;
        }
        if let Some(ref args) = expr.args {
            for arg in args {
                Self::validate_dt_node(arg)?;
            }
        }
        if let Some(ref subquery) = expr.subquery
            && let Some(ref inner) = subquery.expr {
                Self::validate_dt_node(inner)?;
            }

        Ok(())
    }

    /// Validate the non-field side of a comparison against a datetime field.
    /// Recurses into IN/NotIn arg lists; tries to parse plain literal `val`s.
    fn check_dt_value_side(expr: &Expr) -> Result<(), String> {
        if expr.function.is_some() {
            // a function result is data-dependent; its arguments are not
            // literals compared with the field (`date_add(created, 1)`)
            return Ok(());
        }
        if let Some(ref args) = expr.args {
            for arg in args {
                Self::check_dt_value_side(arg)?;
            }
        }
        if let Some(ref val) = expr.val
            && expr.field.is_none() && expr.function.is_none() && expr.subquery.is_none()
                && expr.root_alias.is_none()
                && crate::util::parse_datetime(val).is_err() {
                    return Err(format!("Can't parse datetime: {}", val));
                }
        Ok(())
    }

    /// Walk the expression tree and verify every plain literal compared
    /// against a numeric field is a number or a file size (`12`, `0.5`,
    /// `1kb`, `1.5 MiB`). Returns `Err` with the first offending literal so
    /// the query fails at parse time instead of silently matching nothing.
    ///
    /// Rejects calls with the wrong number of arguments anywhere in the
    /// expression tree, so the mistake is reported once, before the search,
    /// instead of per file (or never, when extra arguments were ignored).
    pub fn validate_date_function_literals(&self) -> Result<(), String> {
        if let Some(ref function) = self.function {
            for &position in function.datetime_argument_positions() {
                let argument = match position {
                    0 => self.left.as_deref(),
                    n => self.args.as_ref().and_then(|args| args.get(n - 1)),
                };
                if let Some(argument) = argument
                    && argument.is_literal()
                    && let Some(ref val) = argument.val
                {
                    let text = if argument.minus { format!("-{}", val) } else { val.clone() };
                    if crate::util::parse_datetime(&text).is_err() {
                        return Err(format!("Can't parse datetime: {}", text));
                    }
                }
            }
        }
        for child in self.left.iter().chain(self.right.iter()) {
            child.validate_date_function_literals()?;
        }
        for arg in self.args.iter().flatten() {
            arg.validate_date_function_literals()?;
        }
        if let Some(ref subquery) = self.subquery {
            for expr in subquery.fields.iter().chain(subquery.expr.iter()) {
                expr.validate_date_function_literals()?;
            }
        }
        Ok(())
    }

    pub fn validate_format_size_modifiers(&self) -> Result<(), String> {
        if self.function == Some(Function::FormatSize)
            && let Some(modifier) = self.args.as_ref().and_then(|args| args.first())
            && modifier.is_literal()
            && let Some(ref val) = modifier.val
        {
            crate::util::format_filesize(0, val)?;
        }
        for child in self.left.iter().chain(self.right.iter()) {
            child.validate_format_size_modifiers()?;
        }
        for arg in self.args.iter().flatten() {
            arg.validate_format_size_modifiers()?;
        }
        if let Some(ref subquery) = self.subquery {
            for expr in subquery.fields.iter().chain(subquery.expr.iter()) {
                expr.validate_format_size_modifiers()?;
            }
        }
        Ok(())
    }

    pub fn validate_function_arity(&self) -> Result<(), String> {
        if let Some(ref function) = self.function {
            let given = usize::from(self.left.is_some()) + self.args.as_ref().map_or(0, Vec::len);
            function.check_argument_count(given)?;
        }
        for child in self.left.iter().chain(self.right.iter()) {
            child.validate_function_arity()?;
        }
        for arg in self.args.iter().flatten() {
            arg.validate_function_arity()?;
        }
        Ok(())
    }

    /// Like `validate_datetime_literals`, only literals adjacent to a numeric
    /// *field* are checked; fields, functions and subqueries on the value side
    /// are data-dependent and left to runtime (`compare_variants`).
    pub fn validate_numeric_literals(&self) -> Result<(), String> {
        Self::validate_num_node(self)
    }

    fn validate_num_node(expr: &Expr) -> Result<(), String> {
        if expr.op.as_ref().is_some_and(|op| !op.checks_null()) {
            let numeric_field = |side: &Option<Box<Expr>>| {
                side.as_deref()
                    .and_then(|e| e.field.as_ref())
                    .filter(|f| compares_numerically(f))
                    .cloned()
            };
            if let Some(field) = numeric_field(&expr.left)
                && let Some(ref right) = expr.right {
                    Self::check_num_value_side(right, &field)?;
                }
            if let Some(field) = numeric_field(&expr.right)
                && let Some(ref left) = expr.left {
                    Self::check_num_value_side(left, &field)?;
                }
        }

        if let Some(ref left) = expr.left {
            Self::validate_num_node(left)?;
        }
        if let Some(ref right) = expr.right {
            Self::validate_num_node(right)?;
        }
        if let Some(ref args) = expr.args {
            for arg in args {
                Self::validate_num_node(arg)?;
            }
        }
        if let Some(ref subquery) = expr.subquery
            && let Some(ref inner) = subquery.expr {
                Self::validate_num_node(inner)?;
            }

        Ok(())
    }

    /// Validate the non-field side of a comparison against a numeric field.
    /// Recurses into IN/NotIn arg lists; tries to coerce plain literal `val`s.
    fn check_num_value_side(expr: &Expr, field: &Field) -> Result<(), String> {
        if expr.function.is_some() {
            // a function result is data-dependent; its arguments are not
            // literals compared with the field (`locate(name, 'a')`)
            return Ok(());
        }
        if let Some(ref args) = expr.args {
            // an IN list: only its members matter, the holder's own val is a placeholder
            for arg in args {
                Self::check_num_value_side(arg, field)?;
            }
        } else if let Some(ref val) = expr.val
            && expr.field.is_none() && expr.function.is_none() && expr.subquery.is_none()
                && expr.root_alias.is_none()
                && Variant::from_string(val).to_float_opt().is_none() {
                    return Err(format!(
                        "Can't parse number or file size '{}' for field {}",
                        val,
                        field.to_string().to_lowercase()
                    ));
                }
        Ok(())
    }

    pub fn column_name(&self) -> String {
        match self.alias {
            Some(ref alias) => alias.to_lowercase(),
            None => self.to_string().to_lowercase(),
        }
    }

    pub fn contains_colorized(&self) -> bool {
        Self::contains_colorized_field(self)
    }

    pub fn reads_entry(&self) -> bool {
        self.function.as_ref().is_some_and(Function::reads_entry)
            || self.left.as_ref().is_some_and(|left| left.reads_entry())
            || self.right.as_ref().is_some_and(|right| right.reads_entry())
            || self.args.as_ref().is_some_and(|args| args.iter().any(Expr::reads_entry))
    }

    pub fn has_correlated_subquery(&self) -> bool {
        self.subquery.as_ref().is_some_and(|subquery| subquery.is_correlated())
            || self.left.as_ref().is_some_and(|left| left.has_correlated_subquery())
            || self.right.as_ref().is_some_and(|right| right.has_correlated_subquery())
            || self.args.as_ref().is_some_and(|args| args.iter().any(Expr::has_correlated_subquery))
    }

    pub fn needs_row_context(&self) -> bool {
        self.subquery.is_some()
            || self.root_alias.is_some()
            || self.left.as_ref().is_some_and(|left| left.needs_row_context())
            || self.right.as_ref().is_some_and(|right| right.needs_row_context())
            || self.args.as_ref().is_some_and(|args| args.iter().any(Expr::needs_row_context))
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

        if let Some(ref left) = expr.left
            && Self::contains_colorized_field(left) {
                return true;
            }

        if let Some(ref right) = expr.right
            && Self::contains_colorized_field(right) {
                return true;
            }

        false
    }
}

impl Display for Expr {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        self.render(fmt, Rendering::Display)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Rendering {
    Display,
    CacheKey,
}

impl Expr {
    pub fn cache_key(&self) -> String {
        let mut key = String::new();
        self.render(&mut key, Rendering::CacheKey).expect("writing to a String cannot fail");
        key
    }

    pub fn mark_local_roots(&mut self, own_aliases: &[String]) {
        if self.field.is_some()
            && let Some(ref root_alias) = self.root_alias
            && own_aliases.iter().any(|alias| alias.eq_ignore_ascii_case(root_alias))
        {
            self.props.mark_local_root();
        }
        for child in self.left.iter_mut().chain(self.right.iter_mut()) {
            child.mark_local_roots(own_aliases);
        }
        for arg in self.args.iter_mut().flatten() {
            arg.mark_local_roots(own_aliases);
        }
    }

    pub fn alias_reference(root_alias: String, text: String) -> Expr {
        let mut expr = Expr::value(text);
        expr.root_alias = Some(root_alias);
        expr
    }

    pub fn alias_reference_key(&self) -> Option<(&str, &str)> {
        let root_alias = self.root_alias.as_deref()?;
        if self.field.is_some() || self.function.is_some() || self.subquery.is_some() {
            return None;
        }
        let text = self.val.as_deref()?;
        let name = text.strip_prefix(root_alias)?.strip_prefix('.')?;
        Some((root_alias, name))
    }

    pub fn is_literal(&self) -> bool {
        self.val.is_some()
            && self.root_alias.is_none()
            && self.field.is_none()
            && self.function.is_none()
            && self.subquery.is_none()
            && self.left.is_none()
            && self.right.is_none()
    }

    fn render(&self, out: &mut dyn fmt::Write, mode: Rendering) -> fmt::Result {
        let alias = match mode {
            Rendering::Display => self.alias.as_deref(),
            Rendering::CacheKey => None,
        };

        if let Some(ref subquery) = self.subquery {
            if let Some(alias) = alias {
                if let Some(ref root_alias) = self.root_alias {
                    out.write_str(root_alias)?;
                    out.write_char('.')?;
                }
                out.write_str(alias)?;
            } else {
                let subquery_str = format!("{:?}", subquery);
                let mut s = DefaultHasher::new();
                subquery_str.hash(&mut s);
                write!(out, "subquery_{}", s.finish())?;
            }
        }

        if self.minus {
            out.write_char('-')?;
        }

        // A negated composite must keep its grouping: -(1 Add 2) and
        // (-1) Add 2 would otherwise render identically and share a cache key.
        let negated_composite = self.minus
            && (self.arithmetic_op.is_some() || self.logical_op.is_some() || self.op.is_some());
        if negated_composite {
            out.write_char('(')?;
        }

        if let Some(ref function) = self.function {
            if let Some(alias) = alias {
                out.write_str(alias)?;
            } else {
                write!(out, "{}", function)?;
                out.write_char('(')?;
                if let Some(ref left) = self.left {
                    left.render(out, mode)?;
                }
                if let Some(ref args) = self.args {
                    for (i, arg) in args.iter().enumerate() {
                        if self.left.is_some() || i > 0 {
                            out.write_str(", ")?;
                        }
                        arg.render(out, mode)?;
                    }
                }
                out.write_char(')')?;
            }
        } else if let Some(ref left) = self.left {
            write_operand(out, left, mode)?;
        }

        if let Some(ref op) = self.arithmetic_op {
            write!(out, " {} ", op)?;
        }

        if let Some(ref op) = self.logical_op {
            write!(out, " {:?} ", op)?;
        }

        if let Some(ref op) = self.op {
            write!(out, " {:?} ", op)?;
        }

        if let Some(ref field) = self.field {
            if let Some(ref root_alias) = self.root_alias
                && !(mode == Rendering::CacheKey && self.props.is_local_root())
            {
                out.write_str(root_alias)?;
                out.write_char('.')?;
            }
            if let Some(alias) = alias {
                out.write_str(alias)?;
            } else {
                write!(out, "{}", field)?;
            }
        }

        if let Some(ref val) = self.val {
            if mode == Rendering::CacheKey && self.is_literal() {
                write!(out, "{:?}", val)?;
            } else {
                out.write_str(val)?;
            }
        }

        if let Some(ref right) = self.right {
            write_operand(out, right, mode)?;
        }

        if negated_composite {
            out.write_char(')')?;
        }

        Ok(())
    }
}

/// Write a child operand, parenthesized when it is itself a composite
/// expression. Without this, `(1 + 2) * 3` and `1 + (2 * 3)` render to the
/// same string, and the rendered form is used as the per-file evaluation
/// cache key — colliding columns would silently reuse each other's values.
fn write_operand(out: &mut dyn fmt::Write, operand: &Expr, mode: Rendering) -> fmt::Result {
    let composite = operand.arithmetic_op.is_some()
        || operand.logical_op.is_some()
        || operand.op.is_some();

    if composite {
        out.write_char('(')?;
    }
    operand.render(out, mode)?;
    if composite {
        out.write_char(')')?;
    }

    Ok(())
}

/// Numeric fields whose runtime values are actually strings (`fsize` is
/// "12 KiB", exif ratios are "1/250" or "f/2.8"); the `numeric` data type on
/// them only drives sorting, so literal validation must skip them.
fn compares_numerically(field: &Field) -> bool {
    field.is_numeric_field()
        && !matches!(
            field,
            Field::FormattedSize
                | Field::ExifExposureTime
                | Field::ExifAperture
                | Field::ExifShutterSpeed
                | Field::ExifFNumber
                | Field::ExifFocalLength
                | Field::ExifExposureBias
                | Field::ExifDigitalZoom
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_escapes_quotes_inside_literals() {
        let one_literal = Expr::function_left(Function::Concat, Some(Box::new(Expr::value(String::from("a', 'b")))));
        let mut two_literals = Expr::function_left(Function::Concat, Some(Box::new(Expr::value(String::from("a")))));
        two_literals.set_args(vec![Expr::value(String::from("b"))]);
        assert_ne!(one_literal.cache_key(), two_literals.cache_key(), "cache keys must not");

        let backslash = Expr::value(String::from("a\\', 'b"));
        let plain = Expr::value(String::from("a\\\\', 'b"));
        assert_ne!(backslash.cache_key(), plain.cache_key());
    }

    #[test]
    fn cache_key_separates_literals_fields_and_aliases() {
        let literal = Expr::value(String::from("Name"));
        let field = Expr::field(Field::Name);
        assert_eq!(literal.to_string(), field.to_string());
        assert_eq!(literal.cache_key(), "\"Name\"");
        assert_eq!(field.cache_key(), "Name");

        let mut aliased_field = Expr::field(Field::Size);
        aliased_field.alias = Some(String::from("Name"));
        assert_eq!(aliased_field.to_string(), "Name");
        assert_eq!(aliased_field.cache_key(), "Size");

        let mut aliased_function = Expr::function_left(Function::Upper, Some(Box::new(Expr::field(Field::Name))));
        aliased_function.alias = Some(String::from("Size"));
        assert_eq!(aliased_function.to_string(), "Size");
        assert_eq!(aliased_function.cache_key(), "Upper(Name)");

        let nested = Expr::function_left(Function::Length, Some(Box::new(Expr::value(String::from("n")))));
        assert_eq!(nested.cache_key(), "Length(\"n\")");

        let computed = Expr::arithmetic_op(Expr::field(Field::Size), ArithmeticOp::Multiply, Expr::value(String::from("2")));
        assert_eq!(computed.cache_key(), "Size * \"2\"");

        let external = Expr::field_with_root_alias(Field::Name, Some(String::from("t1")));
        assert_eq!(external.cache_key(), external.to_string());
    }

    #[test]
    fn column_name_is_lowercase_and_honors_aliases() {
        assert_eq!(Expr::field(Field::Extension).column_name(), "extension");
        assert_eq!(
            Expr::field_with_root_alias(Field::Name, Some("t1".to_string())).column_name(),
            "t1.name"
        );

        let doubled = Expr::arithmetic_op(Expr::field(Field::Size), ArithmeticOp::Multiply, Expr::value("2".to_string()));
        assert_eq!(doubled.column_name(), "size * 2");

        let mut aliased = doubled.clone();
        aliased.alias = Some("Dbl".to_string());
        assert_eq!(aliased.column_name(), "dbl", "alias on an arithmetic expression");

        let upper = Expr::function_left(Function::Upper, Some(Box::new(Expr::field(Field::Name))));
        assert_eq!(upper.column_name(), "upper(name)");

        let mut aliased_field = Expr::field(Field::Extension);
        aliased_field.alias = Some("E".to_string());
        assert_eq!(aliased_field.column_name(), "e");
    }
    use crate::field::Field;
    use crate::function::Function;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn display_distinguishes_operand_grouping() {
        // The rendered form is the per-file evaluation cache key: without
        // parentheses, (1+2)*3 and 1+(2*3) collide and reuse each other's
        // cached values.
        let query = "select (1+2)*3, 1+(2*3) from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut parser = Parser::new(&mut lexer);
        let query = parser.parse(false).unwrap();

        assert_ne!(query.fields[0].to_string(), query.fields[1].to_string());
    }

    #[test]
    fn display_distinguishes_negated_composite() {
        // -(1+2) is -3 while -1+2 is 1; without parentheses around the negated
        // composite both would render identically and share a cache key.
        let query = "select -(1+2), -1+2 from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut parser = Parser::new(&mut lexer);
        let query = parser.parse(false).unwrap();

        assert_ne!(query.fields[0].to_string(), query.fields[1].to_string());
    }

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
        ).unwrap();
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
                ).unwrap(),
            ).unwrap(),
            LogicalOp::Or,
            Expr::op(
                Expr::field(Field::Name),
                Op::Eq,
                Expr::value(String::from("xxx")),
            ),
        ).unwrap();
        assert_eq!(expr.weight, 2);
    }

    #[test]
    fn props_default_empty_and_mutable() {
        let mut expr = Expr::value("1".to_string());
        assert!(expr.props.is_empty());

        expr.set_prop("weight_hint", Variant::from_int(5));
        expr.props.insert("name".to_string(), Variant::from_string(&"x".to_string()));

        assert!(expr.props.contains_key("weight_hint"));
        assert_eq!(expr.get_prop("weight_hint").map(|v| v.to_int()), Some(5));
        assert_eq!(expr.props.len(), 2);

        assert_eq!(expr.props.remove("name").map(|v| v.to_string()), Some("x".to_string()));
        assert!(expr.get_prop("name").is_none());
    }

    #[test]
    fn props_are_cloned_but_ignored_by_expr_equality() {
        let mut a = Expr::value("1".to_string());
        a.set_prop("k", Variant::from_bool(true));

        let cloned = a.clone();
        assert_eq!(*cloned.props, *a.props);

        let mut b = Expr::value("1".to_string());
        assert_ne!(*b.props, *a.props);
        assert_eq!(a, b);

        b.set_prop("k", Variant::from_bool(true));
        assert_eq!(*b.props, *a.props);

        b.set_prop("k", Variant::from_bool(false));
        assert_ne!(*b.props, *a.props);
        assert_eq!(a, b);
    }

    #[test]
    fn external_field_scopes_file_type_props_to_its_alias() {
        // Inside a correlated subquery `t1` names the OUTER entry, so its file
        // type is independent of the subquery entry's own type.
        let outer_dir = Expr::external_field(Field::IsDir, "t1".to_string());
        assert_eq!(outer_dir.bool_prop("t1.is_dir"), Some(true));
        assert_eq!(outer_dir.bool_prop("t1.is_file"), Some(false));
        assert_eq!(outer_dir.bool_prop(IS_DIR), None);

        let own_file = Expr::field_with_root_alias(Field::IsFile, Some("t2".to_string()));
        let both = Expr::logical_op(own_file, LogicalOp::And, outer_dir.clone()).unwrap();
        assert_eq!(both.bool_prop(IS_FILE), Some(true));
        assert_eq!(both.bool_prop("t1.is_dir"), Some(true));

        // Comparisons keep the scope, so same-alias contradictions are still caught.
        let outer_not_dir = Expr::op(
            Expr::external_field(Field::IsDir, "t1".to_string()),
            Op::Eq,
            Expr::value("false".to_string()),
        );
        assert_eq!(outer_not_dir.bool_prop("t1.is_dir"), Some(false));
        assert_eq!(outer_not_dir.bool_prop(IS_DIR), None);
        let err = Expr::logical_op(outer_dir, LogicalOp::And, outer_not_dir).unwrap_err();
        assert!(err.contains("t1.is_dir = true") && err.contains("t1.is_dir = false"), "{}", err);
    }

    #[test]
    fn is_file_field_sets_prop() {
        assert_eq!(Expr::field(Field::IsFile).bool_prop(IS_FILE), Some(true));
        assert_eq!(Expr::field_with_root_alias(Field::IsFile, Some("r".to_string())).bool_prop(IS_FILE), Some(true));
        assert_eq!(Expr::field(Field::Name).bool_prop(IS_FILE), None);
        assert_eq!(Expr::field(Field::IsDir).bool_prop(IS_FILE), Some(false));
    }

    #[test]
    fn file_type_fields_are_mutually_exclusive() {
        for (field, key) in [
            (Field::IsFile, IS_FILE),
            (Field::IsDir, IS_DIR),
            (Field::IsPipe, IS_PIPE),
            (Field::IsCharacterDevice, IS_CHAR),
            (Field::IsBlockDevice, IS_BLOCK),
            (Field::IsSocket, IS_SOCKET),
        ] {
            for expr in [
                Expr::field(field),
                Expr::op(Expr::field(field), Op::Ne, Expr::value("false".to_string())),
            ] {
                assert_eq!(expr.props.len(), FILE_TYPE_PROPS.len(), "{}", key);
                for other in FILE_TYPE_PROPS {
                    assert_eq!(expr.bool_prop(other), Some(other == key), "{} -> {}", key, other);
                }
            }

            let negative = Expr::op(Expr::field(field), Op::Eq, Expr::value("false".to_string()));
            assert_eq!(negative.props.len(), 1, "{}", key);
            assert_eq!(negative.bool_prop(key), Some(false), "{}", key);
        }

        assert_eq!(Expr::field(Field::IsDir).bool_prop(IS_FILE), Some(false));
        assert_eq!(Expr::op(Expr::field(Field::IsDir), Op::Ne, Expr::value("false".to_string())).bool_prop(IS_FILE), Some(false));
        assert_eq!(Expr::op(Expr::field(Field::IsDir), Op::Eq, Expr::value("false".to_string())).bool_prop(IS_FILE), None);
        assert_eq!(Expr::field(Field::IsSymlink).props.len(), 0);
    }

    #[test]
    fn is_dir_field_sets_prop() {
        assert_eq!(Expr::field(Field::IsDir).bool_prop(IS_DIR), Some(true));
        assert_eq!(Expr::field(Field::IsFile).bool_prop(IS_DIR), Some(false));
        assert_eq!(Expr::field(Field::Name).bool_prop(IS_DIR), None);

        let cmp = |op: Op, val: &str| Expr::op(Expr::field(Field::IsDir), op, Expr::value(val.to_string()));
        assert_eq!(cmp(Op::Eq, "true").bool_prop(IS_DIR), Some(true));
        assert_eq!(cmp(Op::Eq, "false").bool_prop(IS_DIR), Some(false));
        assert_eq!(cmp(Op::Ne, "true").bool_prop(IS_DIR), Some(false));
        assert_eq!(cmp(Op::Eq, "true").bool_prop(IS_FILE), Some(false));
        assert_eq!(cmp(Op::Eq, "false").bool_prop(IS_FILE), None);

        let both = Expr::logical_op(
            Expr::op(Expr::field(Field::IsFile), Op::Eq, Expr::value("false".to_string())),
            LogicalOp::And,
            Expr::op(Expr::field(Field::IsDir), Op::Eq, Expr::value("true".to_string())),
        ).unwrap();
        assert_eq!(both.bool_prop(IS_FILE), Some(false));
        assert_eq!(both.bool_prop(IS_DIR), Some(true));
    }

    #[test]
    fn is_file_comparison_sets_prop() {
        let cmp = |op: Op, val: &str| Expr::op(Expr::field(Field::IsFile), op, Expr::value(val.to_string()));
        assert_eq!(cmp(Op::Eq, "true").bool_prop(IS_FILE), Some(true));
        assert_eq!(cmp(Op::Eeq, "yes").bool_prop(IS_FILE), Some(true));
        assert_eq!(cmp(Op::Eq, "false").bool_prop(IS_FILE), Some(false));
        assert_eq!(cmp(Op::Ne, "true").bool_prop(IS_FILE), Some(false));
        assert_eq!(cmp(Op::Ne, "false").bool_prop(IS_FILE), Some(true));
        assert_eq!(cmp(Op::Eq, "maybe").bool_prop(IS_FILE), None);
        assert_eq!(cmp(Op::Like, "true").bool_prop(IS_FILE), None);

        let reversed = Expr::op(Expr::value("true".to_string()), Op::Eq, Expr::field(Field::IsFile));
        assert_eq!(reversed.bool_prop(IS_FILE), Some(true));

        let other = Expr::op(Expr::field(Field::Name), Op::Eq, Expr::value("true".to_string()));
        assert_eq!(other.bool_prop(IS_FILE), None);
    }

    #[test]
    fn logical_op_propagates_is_file() {
        let is_file = || Expr::op(Expr::field(Field::IsFile), Op::Eq, Expr::value("true".to_string()));
        let not_file = || Expr::op(Expr::field(Field::IsFile), Op::Eq, Expr::value("false".to_string()));
        let unrelated = || Expr::op(Expr::field(Field::Size), Op::Gt, Expr::value("0".to_string()));

        assert_eq!(Expr::logical_op(is_file(), LogicalOp::And, unrelated()).unwrap().bool_prop(IS_FILE), Some(true));
        assert_eq!(Expr::logical_op(unrelated(), LogicalOp::And, not_file()).unwrap().bool_prop(IS_FILE), Some(false));
        assert_eq!(Expr::logical_op(is_file(), LogicalOp::Or, unrelated()).unwrap().bool_prop(IS_FILE), None);
        assert_eq!(Expr::logical_op(is_file(), LogicalOp::Or, is_file()).unwrap().bool_prop(IS_FILE), Some(true));
        assert_eq!(Expr::logical_op(unrelated(), LogicalOp::And, unrelated()).unwrap().bool_prop(IS_FILE), None);

        let err = Expr::logical_op(is_file(), LogicalOp::And, not_file()).unwrap_err();
        assert!(err.contains("is_file"), "unexpected error: {}", err);
        assert_eq!(Expr::logical_op(not_file(), LogicalOp::Or, is_file()).unwrap().bool_prop(IS_FILE), None);
    }

    #[test]
    fn logical_op_merges_arbitrary_props() {
        let with = |key: &str, value: i64| {
            let mut e = Expr::value("1".to_string());
            e.set_prop(key, Variant::from_int(value));
            e
        };
        let plain = || Expr::value("1".to_string());

        let and = Expr::logical_op(with("a", 1), LogicalOp::And, with("b", 2)).unwrap();
        assert_eq!(and.get_prop("a").map(|v| v.to_int()), Some(1));
        assert_eq!(and.get_prop("b").map(|v| v.to_int()), Some(2));

        let or = Expr::logical_op(with("a", 1), LogicalOp::Or, with("b", 2)).unwrap();
        assert!(or.props.is_empty());

        let or_same = Expr::logical_op(with("a", 1), LogicalOp::Or, with("a", 1)).unwrap();
        assert_eq!(or_same.get_prop("a").map(|v| v.to_int()), Some(1));

        assert_eq!(Expr::logical_op(with("a", 1), LogicalOp::And, plain()).unwrap().get_prop("a").map(|v| v.to_int()), Some(1));
        assert!(Expr::logical_op(plain(), LogicalOp::Or, with("a", 1)).unwrap().props.is_empty());

        let err = Expr::logical_op(with("a", 1), LogicalOp::And, with("a", 2)).unwrap_err();
        assert!(err.contains("a = 1") && err.contains("a = 2"), "unexpected error: {}", err);
        assert!(Expr::logical_op(with("a", 1), LogicalOp::Or, with("a", 2)).unwrap().props.is_empty());
    }

    fn parse_where_expr(sql: &str) -> Expr {
        let mut lexer = Lexer::new(vec![sql.to_string()]);
        let mut parser = Parser::new(&mut lexer);
        let query = parser.parse(false).expect("parse should succeed");
        query.expr.expect("query should have where expr")
    }

    #[test]
    fn no_subqueries_returns_fields_from_top_level_for_alias() {
        let expr = parse_where_expr("select t1.name from /t1 as t1 where t1.size > 10");
        let set = expr.get_fields_required_in_subqueries("t1", false);
        assert!(set.is_empty());
    }

    #[test]
    fn uncorrelated_exists_returns_empty_for_outer_alias() {
        let expr = parse_where_expr(
            "select t1.name from /t1 as t1 where exists(select t2.name from /t2 as t2 where t2.size > 0)"
        );
        let set = expr.get_fields_required_in_subqueries("t1", false);
        assert!(set.is_empty(), "Expected no required fields for t1 in uncorrelated subquery");
    }

    #[test]
    fn correlated_exists_collects_parent_fields() {
        let expr = parse_where_expr(
            "select t1.name from /t1 as t1 where exists(select t2.name from /t2 as t2 where t2.name = t1.name and t2.size > t1.size)"
        );
        let map = expr.get_fields_required_in_subqueries("t1", false);
        assert_eq!(map, HashMap::from([(Field::Name, HashSet::from([String::from("Name")])), (Field::Size, HashSet::from([String::from("Size")]))]));
        let set = expr.right.unwrap().subquery.unwrap().expr.unwrap().get_fields_required_in_subqueries("t2", false);
        assert!(set.is_empty(), "Expected no required fields for t2 in correlated subquery");
    }

    #[test]
    fn correlated_not_exists_collects_parent_fields() {
        let expr = parse_where_expr(
            "select t1.name from /t1 as t1 where not exists(select t2.name from /t2 as t2 where t2.name = t1.name and t2.size > t1.size)"
        );
        let map = expr.get_fields_required_in_subqueries("t1", false);
        assert_eq!(map, HashMap::from([(Field::Name, HashSet::from([String::from("Name")])), (Field::Size, HashSet::from([String::from("Size")]))]));
        let map = expr.right.unwrap().subquery.unwrap().expr.unwrap().get_fields_required_in_subqueries("t2", false);
        assert!(map.is_empty(), "Expected no required fields for t2 in correlated subquery");
    }

    #[test]
    fn arithmetic_over_a_text_field_counts_as_numeric() {
        let expr = Expr::arithmetic_op(Expr::field(Field::Name), ArithmeticOp::Add, Expr::value(String::from("0")));
        assert!(expr.contains_numeric());
        assert!(!Expr::field(Field::Name).contains_numeric());
    }

    #[test]
    fn result_types_are_inferred_for_functions_and_scalar_subqueries() {
        fn parse(query: &str) -> crate::query::Query {
            let mut lexer = crate::lexer::Lexer::new(vec![String::from(query)]);
            crate::parser::Parser::new(&mut lexer).parse(false).unwrap()
        }
        let count = Expr::subquery(parse("select count(*) from /d"));
        assert!(count.contains_numeric(), "count(*) subquery is numeric");
        let latest = Expr::subquery(parse("select max(modified) from /d"));
        assert!(latest.contains_datetime(), "max(modified) subquery is a datetime");
        let names = Expr::subquery(parse("select name from /d"));
        assert!(!names.contains_numeric() && !names.contains_datetime());

        let shifted = parse("select date_add(modified, 30) from /d").fields.remove(0);
        assert!(shifted.contains_datetime());
        let from_epoch = parse("select from_unixtime(mtime) from /d").fields.remove(0);
        assert!(from_epoch.contains_datetime(), "from_unixtime yields a datetime although its argument is numeric");
        let now = parse("select now() from /d").fields.remove(0);
        assert!(now.contains_datetime());
        let fallback = parse("select coalesce(size, 0) from /d").fields.remove(0);
        assert!(fallback.contains_numeric());
    }

    #[test]
    fn string_functions_over_typed_fields_are_strings() {
        let mut lexer = crate::lexer::Lexer::new(vec![String::from(
            "select dayname(modified), concat(size, ''), upper(name), hex(size), coalesce(size, 0), coalesce(modified, ''), length(name), date_add(modified, 1) from /d",
        )]);
        let fields = crate::parser::Parser::new(&mut lexer).parse(false).unwrap().fields;
        let typed: Vec<(bool, bool)> = fields.iter().map(|f| (f.contains_numeric(), f.contains_datetime())).collect();
        assert_eq!(typed[0], (false, false), "dayname(modified) is text");
        assert_eq!(typed[1], (false, false), "concat(size, '') is text");
        assert_eq!(typed[2], (false, false), "upper(name) is text");
        assert_eq!(typed[3], (false, false), "hex(size) is text");
        assert_eq!(typed[4], (true, false), "coalesce passes the numeric type through");
        assert_eq!(typed[5], (false, true), "coalesce passes the datetime type through");
        assert_eq!(typed[6], (true, false), "length(name) is numeric");
        assert_eq!(typed[7], (false, true), "date_add(modified, 1) is a datetime");
    }

    #[test]
    fn contains_numeric_right_side_of_arithmetic() {
        let expr = Expr::arithmetic_op(
            Expr::value("1".to_string()),
            ArithmeticOp::Add,
            Expr::field(Field::Size),
        );
        assert!(expr.contains_numeric());
    }

    #[test]
    fn contains_numeric_right_side_of_op() {
        let expr = Expr::op(
            Expr::value("100".to_string()),
            Op::Gt,
            Expr::field(Field::Size),
        );
        assert!(expr.contains_numeric());
    }

    #[test]
    fn contains_datetime_right_side_of_op() {
        let expr = Expr::op(
            Expr::value("2024-01-01".to_string()),
            Op::Lt,
            Expr::field(Field::Accessed),
        );
        assert!(expr.contains_datetime());
    }

    #[test]
    fn contains_colorized_right_side_of_op() {
        let expr = Expr::op(
            Expr::value("foo".to_string()),
            Op::Eq,
            Expr::field(Field::Name),
        );
        assert!(expr.contains_colorized());
    }

    #[test]
    fn a_string_function_is_text_whatever_its_arguments_are() {
        let mut expr = Expr::function(Function::Concat);
        expr.set_args(vec![Expr::field(Field::Size)]);
        assert!(!expr.contains_numeric());
        let mut expr = Expr::function(Function::Concat);
        expr.set_args(vec![Expr::field(Field::Accessed)]);
        assert!(!expr.contains_datetime());
    }

    #[test]
    fn min_and_max_take_the_type_of_their_argument() {
        let mut latest = Expr::function(Function::Max);
        latest.left = Some(Box::new(Expr::field(Field::Accessed)));
        assert!(latest.contains_datetime() && !latest.contains_numeric());
        let mut smallest = Expr::function(Function::Min);
        smallest.left = Some(Box::new(Expr::field(Field::Size)));
        assert!(smallest.contains_numeric() && !smallest.contains_datetime());
    }

    #[test]
    fn display_arithmetic_op_shows_operator() {
        let expr = Expr::arithmetic_op(
            Expr::value("1".to_string()),
            ArithmeticOp::Add,
            Expr::value("2".to_string()),
        );
        assert_eq!(format!("{}", expr), "1 + 2");
    }

    #[test]
    fn display_logical_op_shows_operator() {
        let expr = Expr::logical_op(
            Expr::op(
                Expr::field(Field::Name),
                Op::Eq,
                Expr::value("foo".to_string()),
            ),
            LogicalOp::And,
            Expr::op(
                Expr::field(Field::Size),
                Op::Gt,
                Expr::value("0".to_string()),
            ),
        ).unwrap();
        let displayed = format!("{}", expr);
        assert!(displayed.contains("And"),
            "Expected 'And' in display, got: {}", displayed);
    }

    #[test]
    fn display_comparison_op_shows_operator() {
        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::Gt,
            Expr::value("100".to_string()),
        );
        let displayed = format!("{}", expr);
        assert!(displayed.contains("Gt"),
            "Expected 'Gt' in display, got: {}", displayed);
    }

    #[test]
    fn display_function_shows_args() {
        let mut expr = Expr::function(Function::Round);
        expr.add_left(Expr::value("3.14".to_string()));
        expr.set_args(vec![Expr::value("2".to_string())]);
        let displayed = format!("{}", expr);
        assert!(displayed.contains("2"),
            "Expected args in display, got: {}", displayed);
    }

    #[test]
    fn add_left_replaces_old_weight() {
        let heavy = Expr::field(Field::Accessed);
        let heavy_weight = heavy.weight;
        let light = Expr::value("x".to_string());
        let light_weight = light.weight;

        let mut expr = Expr::function_left(Function::Lower, Some(Box::new(heavy)));
        let base_weight = expr.weight;
        assert_eq!(base_weight, Function::Lower.get_weight() + heavy_weight);

        expr.add_left(light);
        assert_eq!(expr.weight, Function::Lower.get_weight() + light_weight,
            "weight should reflect only the new left, got: {}", expr.weight);
    }

    #[test]
    fn set_args_replaces_old_weight() {
        let mut expr = Expr::function(Function::Concat);
        let heavy_args = vec![Expr::field(Field::Accessed)];
        let heavy_weight: i32 = heavy_args.iter().map(|a| a.weight).sum();
        expr.set_args(heavy_args);
        let weight_after_first = expr.weight;
        assert_eq!(weight_after_first, Function::Concat.get_weight() + heavy_weight);

        let light_args = vec![Expr::value("x".to_string())];
        let light_weight: i32 = light_args.iter().map(|a| a.weight).sum();
        expr.set_args(light_args);
        assert_eq!(expr.weight, Function::Concat.get_weight() + light_weight,
            "weight should reflect only the new args, got: {}", expr.weight);
    }

    #[test]
    fn get_fields_required_in_subqueries_checks_args() {
        let mut func_expr = Expr::function(Function::ConcatWs);
        func_expr.add_left(Expr::value(",".to_string()));
        func_expr.set_args(vec![
            Expr::field_with_root_alias(Field::Name, Some("t1".to_string())),
            Expr::field_with_root_alias(Field::Name, Some("t2".to_string())),
        ]);
        let map = func_expr.get_fields_required_in_subqueries("t1", true);
        assert_eq!(map.len(), 1, "Expected t1.name in args to be found, got: {:?}", map);
        assert!(map.contains_key(&Field::Name));
    }

    fn size_vs(val: &str, op: Op) -> Expr {
        Expr::op(Expr::field(Field::Size), op, Expr::value(String::from(val)))
    }

    #[test]
    fn validate_numeric_literals_rejects_non_numeric_and_empty() {
        for val in ["abc", "", "1x", "kb", "1..5"] {
            let err = size_vs(val, Op::Gt).validate_numeric_literals().unwrap_err();
            assert!(err.contains(&format!("'{}'", val)) && err.contains("size"), "literal {:?} gave: {}", val, err);
        }
    }

    #[test]
    fn validate_numeric_literals_accepts_numbers_and_file_sizes() {
        for val in ["0", "12", "0.5", "-3", "1k", "1kb", "1.5 MiB", "2gib", "3tb"] {
            assert!(size_vs(val, Op::Gt).validate_numeric_literals().is_ok(), "literal {:?} rejected", val);
        }
        let mut negative = Expr::value(String::from("5"));
        negative.minus = true;
        assert!(Expr::op(Expr::field(Field::Size), Op::Gt, negative).validate_numeric_literals().is_ok());
    }

    #[test]
    fn validate_numeric_literals_checks_literal_on_either_side() {
        let err = Expr::op(Expr::value(String::from("abc")), Op::Lt, Expr::field(Field::Size))
            .validate_numeric_literals()
            .unwrap_err();
        assert!(err.contains("abc"));
    }

    #[test]
    fn validate_numeric_literals_skips_non_numeric_fields() {
        assert!(Expr::op(Expr::field(Field::Name), Op::Eq, Expr::value(String::from("abc"))).validate_numeric_literals().is_ok());
        assert!(Expr::op(Expr::field(Field::Modified), Op::Eq, Expr::value(String::from("yesterday"))).validate_numeric_literals().is_ok());
    }

    #[test]
    fn validate_numeric_literals_skips_numeric_fields_with_string_values() {
        // declared numeric for sorting, but their values are strings
        assert!(Expr::op(Expr::field(Field::FormattedSize), Op::Eq, Expr::value(String::from("12 B"))).validate_numeric_literals().is_ok());
        assert!(Expr::op(Expr::field(Field::ExifExposureTime), Op::Eq, Expr::value(String::from("1/250"))).validate_numeric_literals().is_ok());
        assert!(Expr::op(Expr::field(Field::ExifFNumber), Op::Eq, Expr::value(String::from("f/2.8"))).validate_numeric_literals().is_ok());
    }

    #[test]
    fn validate_numeric_literals_ignores_non_literal_value_side() {
        assert!(parse_where_expr("select name from . where size > name").validate_numeric_literals().is_ok());
        assert!(parse_where_expr("select name from . where size > length(name)").validate_numeric_literals().is_ok());
        assert!(parse_where_expr("select name from . where size > 2 * 4").validate_numeric_literals().is_ok());
        assert!(parse_where_expr("select name from . where size > abs(-5)").validate_numeric_literals().is_ok());
    }

    #[test]
    fn validate_numeric_literals_recurses_into_logical_op_and_in_args() {
        let expr = Expr::logical_op(
            Expr::op(Expr::field(Field::Name), Op::Eq, Expr::value(String::from("x"))),
            LogicalOp::And,
            size_vs("oops", Op::Eq),
        ).unwrap();
        assert!(expr.validate_numeric_literals().unwrap_err().contains("oops"));

        let mut right = Expr::value(String::from(""));
        right.args = Some(vec![Expr::value(String::from("0")), Expr::value(String::from("bad"))]);
        let err = Expr::op(Expr::field(Field::Size), Op::In, right).validate_numeric_literals().unwrap_err();
        assert!(err.contains("bad"));

        let mut right = Expr::value(String::from(""));
        right.args = Some(vec![Expr::value(String::from("0")), Expr::value(String::from("1kb"))]);
        assert!(Expr::op(Expr::field(Field::Size), Op::In, right).validate_numeric_literals().is_ok());
    }

    #[test]
    fn validate_numeric_literals_ignores_function_args() {
        // `locate('a', name)` returns a number; its string argument is not a
        // literal compared with size and must not be validated as one.
        assert!(parse_where_expr("select name from . where size > locate('a', name)").validate_numeric_literals().is_ok());
        assert!(parse_where_expr("select name from . where size > locate(name, 'a')").validate_numeric_literals().is_ok());
        assert!(parse_where_expr("select name from . where size > length(concat(name, 'abc'))").validate_numeric_literals().is_ok());
    }

    #[test]
    fn validate_datetime_literals_ignores_function_args() {
        assert!(parse_where_expr("select name from . where modified > date_add(created, 1)").validate_datetime_literals().is_ok());
        assert!(parse_where_expr("select name from . where modified > coalesce(created, 'not-a-date')").validate_datetime_literals().is_ok());
    }

    #[test]
    fn validate_datetime_literals_accepts_valid_dates() {
        let expr = Expr::op(
            Expr::field(Field::Modified),
            Op::Eq,
            Expr::value(String::from("2026-01-01")),
        );
        assert!(expr.validate_datetime_literals().is_ok());
    }

    #[test]
    fn validate_datetime_literals_accepts_today_keyword() {
        let expr = Expr::op(
            Expr::field(Field::Modified),
            Op::Gt,
            Expr::value(String::from("today")),
        );
        assert!(expr.validate_datetime_literals().is_ok());
    }

    #[test]
    fn validate_datetime_literals_rejects_garbage() {
        let expr = Expr::op(
            Expr::field(Field::Modified),
            Op::Eq,
            Expr::value(String::from("not-a-date")),
        );
        let err = expr.validate_datetime_literals().unwrap_err();
        assert!(err.contains("not-a-date"), "expected error to mention bad literal, got: {}", err);
    }

    #[test]
    fn validate_datetime_literals_skips_non_datetime_fields() {
        // `name = 'not-a-date'` is a perfectly valid string comparison —
        // validation must not flag literals when the field isn't datetime.
        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::Eq,
            Expr::value(String::from("not-a-date")),
        );
        assert!(expr.validate_datetime_literals().is_ok());
    }

    #[test]
    fn validate_datetime_literals_recurses_into_logical_op() {
        // `size > 0 AND modified = 'oops'` — bad literal is in a nested arm.
        let expr = Expr::logical_op(
            Expr::op(
                Expr::field(Field::Size),
                Op::Gt,
                Expr::value(String::from("0")),
            ),
            LogicalOp::And,
            Expr::op(
                Expr::field(Field::Modified),
                Op::Eq,
                Expr::value(String::from("oops")),
            ),
        ).unwrap();
        let err = expr.validate_datetime_literals().unwrap_err();
        assert!(err.contains("oops"));
    }

    #[test]
    fn validate_datetime_literals_recurses_into_in_args() {
        // `modified IN ('2026-01-01', 'bad')` — IN args are validated too.
        let mut right = Expr::value(String::from(""));
        right.args = Some(vec![
            Expr::value(String::from("2026-01-01")),
            Expr::value(String::from("bad")),
        ]);
        let expr = Expr::op(Expr::field(Field::Modified), Op::In, right);
        let err = expr.validate_datetime_literals().unwrap_err();
        assert!(err.contains("bad"));
    }

    #[test]
    fn validate_datetime_literals_ignores_field_on_value_side() {
        // `modified = created` — both sides are fields, no literal to validate.
        let expr = Expr::op(
            Expr::field(Field::Modified),
            Op::Eq,
            Expr::field(Field::Created),
        );
        assert!(expr.validate_datetime_literals().is_ok());
    }

    #[test]
    fn deeply_nested_subquery_can_reference_outer_alias() {
        let expr = parse_where_expr(
            "select t1.name from /t1 as t1 where exists(select t2.name from /t2 as t2 where t2.name in (select t3.name from /t3 as t3 where t3.modified = t1.modified) and t2.size > t1.size)"
        );
        let map = expr.get_fields_required_in_subqueries("t1", false);
        assert_eq!(map, HashMap::from([(Field::Modified, HashSet::from([String::from("Modified")])), (Field::Size, HashSet::from([String::from("Size")]))]));
        let map = expr.clone().right.unwrap().subquery.unwrap().expr.unwrap().get_fields_required_in_subqueries("t2", false);
        assert!(map.is_empty(), "Expected no required fields for t2 in correlated subquery");
        let map = expr.clone().right.unwrap().subquery.unwrap().expr.unwrap().right.unwrap().right.unwrap().subquery.unwrap().expr.unwrap().get_fields_required_in_subqueries("t3", false);
        assert!(map.is_empty(), "Expected no required fields for t3 in correlated subquery");
        let map = expr.right.unwrap().subquery.unwrap().expr.unwrap().right.unwrap().right.unwrap().subquery.unwrap().expr.unwrap().get_fields_required_in_subqueries("t1", false);
        assert!(map.is_empty(), "Expected no required fields for t1 in correlated subquery");
    }

    #[test]
    fn correlated_order_by_in_subquery_collects_parent_fields() {
        // The subquery's ONLY reference to the outer alias `t1` lives in its
        // ORDER BY clause. The dependency walk must still surface `t1.size` so
        // the outer row's value is propagated to the subquery via
        // record_context; otherwise the ordering reads an empty value.
        let expr = parse_where_expr(
            "select t1.name from /t1 as t1 where t1.size = (select t2.size from /t2 as t2 order by abs(t2.size - t1.size) limit 1)"
        );
        let map = expr.get_fields_required_in_subqueries("t1", false);
        assert_eq!(
            map,
            HashMap::from([(Field::Size, HashSet::from([String::from("Size")]))]),
            "ORDER BY correlation on t1.size must be collected, got: {:?}", map
        );
    }

    #[test]
    fn correlated_group_by_in_subquery_collects_parent_fields() {
        // Same as above, but the outer-alias reference lives in GROUP BY.
        let expr = parse_where_expr(
            "select t1.name from /t1 as t1 where t1.size = (select max(t2.size) from /t2 as t2 group by abs(t2.size - t1.size) limit 1)"
        );
        let map = expr.get_fields_required_in_subqueries("t1", false);
        assert_eq!(
            map,
            HashMap::from([(Field::Size, HashSet::from([String::from("Size")]))]),
            "GROUP BY correlation on t1.size must be collected, got: {:?}", map
        );
    }
}
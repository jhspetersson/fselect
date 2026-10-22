//! Handles the parsing of the query string

use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;

#[cfg(feature = "interactive")]
use directories::UserDirs;
use crate::expr::Expr;
use crate::field::Field;
use crate::function::Function;
use crate::lexer::Lexeme;
use crate::lexer::Lexer;
use crate::operators::ArithmeticOp;
use crate::operators::LogicalOp;
use crate::operators::Op;
use crate::query::Query;
use crate::query::Root;
use crate::query::TraversalMode::{Bfs, Dfs};
use crate::query::{OutputFormat, RootOptions};
#[cfg(not(feature = "git"))]
use crate::util::error::error_message;

#[derive(Clone)]
struct AliasScope {
    alias: String,
    columns: Vec<(String, Expr)>,
    own: bool,
}

pub struct Parser<'a> {
    lexer: &'a mut Lexer,
    lexemes: Vec<Lexeme>,
    index: isize,
    roots_parsed: bool,
    where_parsed: bool,
    in_having: bool,
    args_depth: u32,
    debug: bool,
    /// Aliases declared on this query's own roots; any other alias in an
    /// expression refers to an enclosing query's entry.
    root_aliases: Vec<String>,
    /// How many NOTs enclose the condition being parsed.
    negation_depth: usize,
    select_aliases: Vec<Expr>,
    nested: bool,
}

impl <'a> Parser<'a> {
    pub fn new(lexer: &mut Lexer) -> Parser<'_> {
        Parser {
            lexer,
            lexemes: vec![],
            index: -1,
            roots_parsed: false,
            where_parsed: false,
            in_having: false,
            args_depth: 0,
            debug: false,
            root_aliases: vec![],
            negation_depth: 0,
            select_aliases: vec![],
            nested: false,
        }
    }

    fn nested(lexer: &mut Lexer) -> Parser<'_> {
        let mut parser = Parser::new(lexer);
        parser.nested = true;
        parser
    }

    pub fn parse(&mut self, debug: bool) -> Result<Query, String> {
        self.debug = debug;

        if let Some(Lexeme::Select) = self.next_lexeme() {
            // skip the "select" keyword
        } else {
            self.drop_lexeme();
        }

        let mut fields = self.parse_fields()?;
        let mut roots = self.parse_roots()?;
        let root_options = self.parse_root_options()?;
        self.root_aliases = roots.iter().filter_map(|root| root.options.alias.clone()).collect();
        self.roots_parsed = true;
        let mut expr = self.parse_where()?.map(|expr| Self::ensure_boolean_conditions(expr, "WHERE")).transpose()?;
        self.where_parsed = true;
        // Fail once, up front, on literals that can never match: a typo like
        // `modified = 'not-a-date'` or `size > 'abc'` would otherwise be
        // silently coerced per file scanned.
        if let Some(ref where_expr) = expr {
            where_expr.validate_datetime_literals()?;
            where_expr.validate_numeric_literals()?;
            where_expr.validate_boolean_literals()?;
        }
        let mut grouping_fields = self.parse_group_by(&fields)?;
        let mut having = self.parse_having(&fields)?.map(|expr| Self::ensure_boolean_conditions(expr, "HAVING")).transpose()?;
        let (mut ordering_fields, ordering_asc) = self.parse_order_by(&fields)?;
        let (mut limit, limit_offset) = self.parse_limit()?;
        let mut offset = self.parse_offset()?;
        match self.next_lexeme() {
            Some(Lexeme::Error(message)) => return Err(message),
            _ => self.drop_lexeme(),
        }

        if limit_offset > 0 {
            if offset > 0 {
                return Err("Ambiguous offset specified".to_string())
            } else {
                offset = limit_offset;
            }
        }

        let clauses = fields.iter()
            .chain(expr.iter())
            .chain(grouping_fields.iter())
            .chain(having.iter())
            .chain(ordering_fields.iter());
        for clause in clauses {
            clause.validate_function_arity()?;
            clause.validate_date_function_literals()?;
            clause.validate_format_size_modifiers()?;
        }

        // With a single root a qualifier is redundant (`t.size` is `size`),
        // so it is dropped from cache keys; with several roots, named or
        // not, `t.size` and `size` cover different entries and the qualifier
        // must stay.
        if roots.len() == 1 {
            for clause in fields
                .iter_mut()
                .chain(expr.iter_mut())
                .chain(grouping_fields.iter_mut())
                .chain(having.iter_mut())
                .chain(ordering_fields.iter_mut())
            {
                clause.mark_local_roots(&self.root_aliases);
            }
        }

        self.reject_non_grouped_columns(&fields, &grouping_fields, having.as_ref())?;

        if let Some(ref having_expr) = having {
            let aggregate_context = having_expr.has_aggregate_function()
                || !grouping_fields.is_empty()
                || fields.iter().any(|f| f.has_aggregate_function());
            if !aggregate_context {
                return Err("HAVING requires GROUP BY or an aggregate function; use WHERE to filter individual files".to_string());
            }
            if Self::contains_op_unsupported_in_having(having_expr) {
                return Err("EXISTS and NOT EXISTS are not supported in the HAVING clause".to_string());
            }
            let allowed_fields: HashSet<Field> = grouping_fields
                .iter()
                .flat_map(|f| f.get_required_fields())
                .collect();
            for field in having_expr.get_non_aggregated_fields() {
                if !allowed_fields.contains(&field) {
                    return Err(format!(
                        "Cannot use '{}' in HAVING: it is not a grouping column; wrap it in an aggregate function or filter with WHERE",
                        field.to_string().to_lowercase()
                    ));
                }
            }
            let grouping_keys: Vec<String> = grouping_fields.iter().map(Expr::cache_key).collect();
            if let Some(function) = having_expr.non_aggregated_entry_function(&grouping_keys) {
                return Err(format!(
                    "Cannot use '{}()' in HAVING: it reads each file; filter with WHERE or wrap it in an aggregate function",
                    function.canonical_name()
                ));
            }
        }

        let output_format = self.parse_output_format()?;

        if roots.is_empty() {
            roots = self.parse_roots()?;
        }

        if roots.is_empty() {
            roots.push(Root::default(root_options));
        }

        // A SELECT list that touches no file fields (e.g. `select 1+2`) yields
        // the same row for every file, so collapse it to a single row. This
        // must not apply to aggregate or grouped queries: `count(*)` requires
        // no fields either, but grouping produces one row per group.
        if limit == 0
            && grouping_fields.is_empty()
            && !fields.iter().any(|expr| expr.has_aggregate_function())
            && !fields.iter().any(|expr| expr.has_correlated_subquery())
            && !fields.iter().any(Expr::reads_entry)
            && fields
                .iter()
                .all(|expr| expr.get_required_fields().is_empty())
        {
            limit = 1;
        }

        let raw_query = self.lexer.get_input_string();
        let mut query = Query {
            fields,
            roots,
            expr,
            grouping_fields,
            having,
            ordering_fields,
            ordering_asc,
            limit,
            offset,
            output_format,
            raw_query,
        };
        if !self.nested {
            Self::finalize_alias_references(&mut query, &[]);
        }
        Ok(query)
    }

    fn finalize_alias_references(query: &mut Query, enclosing: &[AliasScope]) {
        let mut scope: Vec<AliasScope> = enclosing.iter().map(|entry| AliasScope { own: false, ..entry.clone() }).collect();
        let own_start = scope.len();
        for alias in query.own_root_aliases() {
            scope.push(AliasScope { alias, columns: Self::aliased_columns(&query.fields), own: true });
        }

        // A column may reference an alias whose own column references yet
        // another alias, in any declaration order; each pass over the SELECT
        // list settles one more link, and WHERE is resolved only against the
        // settled columns so their recorded types are complete.
        let passes = query.fields.iter().filter(|column| column.alias.is_some()).count().max(1);
        for _ in 0..passes {
            for expr in query.fields.iter_mut() {
                Self::finalize_expr_references(expr, &scope, false);
            }
            let columns = Self::aliased_columns(&query.fields);
            for entry in &mut scope[own_start..] {
                entry.columns = columns.clone();
            }
        }
        for expr in query.expr.iter_mut() {
            Self::finalize_expr_references(expr, &scope, false);
        }
        for root in query.roots.iter_mut() {
            if let Some(ref mut inner) = root.subquery {
                Self::finalize_alias_references(inner, &scope);
            }
        }
        let clauses = query
            .grouping_fields
            .iter_mut()
            .chain(query.having.iter_mut())
            .chain(query.ordering_fields.iter_mut());
        for expr in clauses {
            Self::finalize_expr_references(expr, &scope, true);
        }
    }

    fn aliased_columns(fields: &[Expr]) -> Vec<(String, Expr)> {
        fields
            .iter()
            .filter_map(|field| field.alias.clone().map(|alias| (alias, field.clone())))
            .collect()
    }

    fn finalize_expr_references(expr: &mut Expr, scope: &[AliasScope], substitute: bool) {
        if let Some((root_alias, name)) = expr.alias_reference_key() {
            let (root_alias, name) = (root_alias.to_string(), name.to_string());
            let entry = scope.iter().rev().find(|entry| entry.alias.eq_ignore_ascii_case(&root_alias));
            let column = entry.and_then(|entry| {
                entry.columns.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(&name)).map(|(_, column)| (entry, column))
            });
            match column {
                Some((entry, column)) if entry.own && substitute => {
                    let minus = expr.minus;
                    *expr = column.clone();
                    if minus {
                        expr.minus = !expr.minus;
                    }
                    return;
                }
                Some((entry, column)) => {
                    let declared = column.alias.clone().unwrap_or(name);
                    expr.val = Some(format!("{}.{}", entry.alias, declared));
                    expr.root_alias = Some(entry.alias.clone());
                    expr.remember_referenced_type(column);
                }
                None => expr.root_alias = None,
            }
        }
        for child in expr.left.iter_mut().chain(expr.right.iter_mut()) {
            Self::finalize_expr_references(child, scope, substitute);
        }
        for arg in expr.args.iter_mut().flatten() {
            Self::finalize_expr_references(arg, scope, substitute);
        }
        if let Some(ref mut subquery) = expr.subquery {
            Self::finalize_alias_references(subquery, scope);
        }
    }

    fn parse_fields(&mut self) -> Result<Vec<Expr>, String> {
        let mut fields = vec![];

        loop {
            let lexeme = self.next_lexeme();
            match lexeme {
                Some(Lexeme::Comma) => {
                    // skip
                }
                Some(Lexeme::String(ref s))
                | Some(Lexeme::RawString(ref s))
                | Some(Lexeme::ArithmeticOperator(ref s)) => {
                    let is_quoted = matches!(lexeme, Some(Lexeme::String(_)));
                    if s == "*" {
                        #[cfg(unix)]
                        {
                            fields.push(Expr::field(Field::Mode));
                            #[cfg(feature = "users")]
                            fields.push(Expr::field(Field::User));
                            #[cfg(feature = "users")]
                            fields.push(Expr::field(Field::Group));
                        }

                        fields.push(Expr::field(Field::Size));
                        fields.push(Expr::field(Field::Modified));
                        fields.push(Expr::field(Field::Path));
                    } else {
                        self.drop_lexeme();

                        // Root option keywords terminate the field list, but
                        // prefix matching makes e.g. "git" shadow the git_*
                        // fields, so anything that parses as a field stays one.
                        if !is_quoted
                            && Self::is_root_option_keyword(s)
                            && (Field::parse_field(s).is_err() || self.numeric_argument_follows())
                        {
                            break;
                        }

                        match self.parse_expr() {
                            Ok(Some(field)) => fields.push(field),
                            Err(e) => return Err(e),
                            _ => {}
                        }
                    }
                }
                Some(Lexeme::Open) | Some(Lexeme::CurlyOpen) | Some(Lexeme::Not) => {
                    self.drop_lexeme();
                    match self.parse_expr() {
                        Ok(Some(field)) => fields.push(field),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
                Some(Lexeme::Error(ref message)) => return Err(message.clone()),
                _ => {
                    self.drop_lexeme();
                    break;
                }
            }
        }

        if fields.is_empty() {
            return Err(String::from("Error parsing fields, no selector found"));
        }

        Ok(fields)
    }

    fn parse_roots(&mut self) -> Result<Vec<Root>, String> {
        enum RootParsingMode {
            Unknown,
            From,
            Root,
            Comma,
        }

        let mut roots: Vec<Root> = Vec::new();
        let mut mode = RootParsingMode::Unknown;

        if let Some(ref lexeme) = self.next_lexeme() {
            match lexeme {
                &Lexeme::From => {
                    mode = RootParsingMode::From;
                }
                _ => {
                    self.drop_lexeme();
                }
            }
        }

        if let RootParsingMode::From = mode {
            let mut path: String = String::from("");
            let mut root_options = RootOptions::new();
            let mut subquery_root: Option<Query> = None;

            loop {
                let lexeme = self.next_lexeme();
                match lexeme {
                    Some(ref lexeme) => match lexeme {
                        Lexeme::Open | Lexeme::CurlyOpen
                            if matches!(mode, RootParsingMode::From | RootParsingMode::Comma) =>
                        {
                            let curly = matches!(lexeme, Lexeme::CurlyOpen);
                            // Look ahead: only treat parenthesised group as a
                            // subselect when it starts with SELECT. Otherwise,
                            // surrender the paren back and stop parsing roots.
                            match self.next_lexeme() {
                                Some(Lexeme::Select) => {
                                    self.lexer.push_state();
                                    let mut sub_parser = Parser::nested(self.lexer);
                                    let sub_query = sub_parser.parse(self.debug)?;
                                    self.lexer.pop_state();
                                    if curly {
                                        self.push_lexeme(Lexeme::CurlyClose);
                                    } else {
                                        self.push_lexeme(Lexeme::Close);
                                    }
                                    let close_lexeme = self.next_lexeme();
                                    let expected_close = if curly { Lexeme::CurlyClose } else { Lexeme::Close };
                                    if close_lexeme != Some(expected_close) {
                                        return Err(String::from("Unmatched parenthesis in FROM subselect"));
                                    }
                                    subquery_root = Some(sub_query);
                                    mode = RootParsingMode::Root;
                                }
                                _ => {
                                    self.drop_lexeme();
                                    self.drop_lexeme();
                                    break;
                                }
                            }
                        }
                        Lexeme::String(s) | Lexeme::RawString(s) => match mode {
                            RootParsingMode::From | RootParsingMode::Comma => {
                                path = s.to_string();
                                #[cfg(feature = "interactive")]
                                if path.starts_with("~")
                                    && let Some(ud) = UserDirs::new() {
                                        let mut pb = PathBuf::from(path.clone());
                                        pb = pb.components().skip(1).collect();
                                        pb = ud.home_dir().to_path_buf().join(pb);
                                        path = pb.to_string_lossy().to_string();
                                    }
                                mode = RootParsingMode::Root;
                            }
                            RootParsingMode::Root => {
                                if s.to_lowercase() == "group"
                                    && let Some(Lexeme::By) = self.next_lexeme() {
                                        self.drop_lexeme();
                                        self.drop_lexeme();

                                        if let Some(sub) = subquery_root.take() {
                                            roots.push(Root::from_subquery(sub, root_options));
                                        } else if !path.is_empty() {
                                            roots.push(Root::new(path, root_options));
                                        }
                                        break;
                                    }

                                self.drop_lexeme();
                                match self.parse_root_options()? {
                                    Some(options) => root_options = options,
                                    None => {
                                        if let Some(sub) = subquery_root.take() {
                                            roots.push(Root::from_subquery(sub, root_options));
                                        } else {
                                            roots.push(Root::new(path, root_options));
                                        }
                                        break
                                    }
                                }
                            }
                            _ => {}
                        },
                        Lexeme::Comma => {
                            if subquery_root.is_some() {
                                let sub = subquery_root.take().unwrap();
                                roots.push(Root::from_subquery(sub, root_options));
                                path = String::from("");
                                root_options = RootOptions::new();
                                mode = RootParsingMode::Comma;
                            } else if !path.is_empty() {
                                roots.push(Root::new(path, root_options));

                                path = String::from("");
                                root_options = RootOptions::new();

                                mode = RootParsingMode::Comma;
                            } else {
                                break;
                            }
                        }
                        _ => {
                            if let Some(sub) = subquery_root.take() {
                                roots.push(Root::from_subquery(sub, root_options));
                            } else if !path.is_empty() {
                                roots.push(Root::new(path, root_options));
                            }

                            self.drop_lexeme();
                            break;
                        }
                    },
                    None => {
                        if let Some(sub) = subquery_root.take() {
                            roots.push(Root::from_subquery(sub, root_options));
                        } else if !path.is_empty() {
                            roots.push(Root::new(path, root_options));
                        }
                        break;
                    }
                }
            }
        }

        Ok(roots)
    }

    fn parse_root_options(&mut self) -> Result<Option<RootOptions>, String> {
        #[derive(Debug, PartialEq)]
        enum RootParsingMode {
            Unknown,
            Options,
            MinDepth,
            Depth,
            Alias,
        }

        let mut mode = RootParsingMode::Unknown;

        let mut min_depth: u32 = 0;
        let mut max_depth: u32 = 0;
        let mut archives = false;
        let mut symlinks = false;
        let mut gitignore = None;
        let mut hgignore = None;
        let mut dockerignore = None;
        let mut traversal = Bfs;
        let mut regexp = false;
        let mut alias: Option<String> = None;

        loop {
            let lexeme = self.next_lexeme();
            match lexeme {
                Some(ref lexeme) => match lexeme {
                    Lexeme::String(s) | Lexeme::RawString(s) => match mode {
                        RootParsingMode::Unknown | RootParsingMode::Options => {
                            let s = s.to_ascii_lowercase();
                            if s == "mindepth" {
                                mode = RootParsingMode::MinDepth;
                            } else if s == "maxdepth" || s == "depth" {
                                mode = RootParsingMode::Depth;
                            } else if s.starts_with("arc") {
                                archives = true;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("sym") {
                                symlinks = true;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("git") {
                                #[cfg(feature = "git")]
                                {
                                    gitignore = Some(true);
                                    mode = RootParsingMode::Options;
                                }
                                #[cfg(not(feature = "git"))]
                                {
                                    error_message("parser", "git support is not enabled");
                                    self.drop_lexeme();
                                    break;
                                }
                            } else if s.starts_with("hg") {
                                hgignore = Some(true);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("dock") {
                                dockerignore = Some(true);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("nogit") {
                                gitignore = Some(false);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("nohg") {
                                hgignore = Some(false);
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("nodock") {
                                dockerignore = Some(false);
                                mode = RootParsingMode::Options;
                            } else if s == "bfs" {
                                traversal = Bfs;
                                mode = RootParsingMode::Options;
                            } else if s == "dfs" {
                                traversal = Dfs;
                                mode = RootParsingMode::Options;
                            } else if s.starts_with("regex") || s == "rx" {
                                regexp = true;
                                mode = RootParsingMode::Options;
                            } else if s == "as" {
                                mode = RootParsingMode::Alias;
                            } else {
                                self.drop_lexeme();
                                break;
                            }
                        }
                        RootParsingMode::MinDepth => {
                            let d: Result<u32, _> = s.parse();
                            match d {
                                Ok(d) => {
                                    min_depth = d;
                                    mode = RootParsingMode::Options;
                                }
                                _ => {
                                    self.drop_lexeme();
                                    break;
                                }
                            }
                        }
                        RootParsingMode::Depth => {
                            let d: Result<u32, _> = s.parse();
                            match d {
                                Ok(d) => {
                                    max_depth = d;
                                    mode = RootParsingMode::Options;
                                }
                                _ => {
                                    self.drop_lexeme();
                                    break;
                                }
                            }
                        }
                        RootParsingMode::Alias => {
                            // Field::parse_field lowercases `T1.size`, so the
                            // declared alias must match case-insensitively.
                            alias = Some(s.to_lowercase());
                            mode = RootParsingMode::Options;
                        }
                    },
                    _ => {
                        self.drop_lexeme();
                        if mode != RootParsingMode::Unknown && mode != RootParsingMode::Options {
                            return Err(String::from("Error parsing root options"));
                        }
                        break;
                    }
                },
                None => {
                    if mode != RootParsingMode::Unknown && mode != RootParsingMode::Options {
                        return Err(String::from("Error parsing root options"));
                    }
                    break;
                }
            }
        }

        match mode {
            RootParsingMode::Unknown => Ok(None),
            _ => Ok(Some(RootOptions {
                min_depth,
                max_depth,
                archives,
                symlinks,
                gitignore,
                hgignore,
                dockerignore,
                traversal,
                regexp,
                alias,
            })),
        }
    }

    fn numeric_argument_follows(&mut self) -> bool {
        self.next_lexeme();
        let follows = matches!(self.next_lexeme(), Some(Lexeme::RawString(ref s)) if s.parse::<u32>().is_ok());
        self.drop_lexeme();
        self.drop_lexeme();
        follows
    }

    fn is_root_option_keyword(s: &str) -> bool {
        let s = s.to_ascii_lowercase();
        s == "depth"
            || s == "mindepth"
            || s == "maxdepth"
            || s.starts_with("arc")
            || s.starts_with("sym")
            || s.starts_with("git")
            || s.starts_with("hg")
            || s.starts_with("dock")
            || s.starts_with("nogit")
            || s.starts_with("nohg")
            || s.starts_with("nodock")
            || s == "bfs"
            || s == "dfs"
            || s.starts_with("regex")
            || s == "rx"
            || s == "as"
    }

    /*

    expr        := and (OR and)*
    and         := cond (AND cond)*
    cond        := add_sub (OP add_sub)*
    add_sub     := mul_div (PLUS mul_div)* | mul_div (MINUS mul_div)*
    mul_div     := paren (MUL paren)* | paren (DIV paren)*
    paren       := ( expr ) | func_scalar
    func_scalar := function paren | field | scalar

    */

    fn parse_where(&mut self) -> Result<Option<Expr>, String> {
        match self.next_lexeme() {
            Some(Lexeme::Where) => self.parse_expr(),
            _ => {
                self.drop_lexeme();
                Ok(None)
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_and()?;

        let mut operands: Vec<Expr> = vec![];
        loop {
            let lexeme = self.next_lexeme();
            match lexeme {
                Some(Lexeme::Or) => {
                    let expr = self.parse_and()?;
                    operands.push(expr.ok_or_else(|| "Expected expression after OR".to_string())?);
                }
                _ => {
                    self.drop_lexeme();

                    let mut result = if operands.is_empty() {
                        Ok(left)
                    } else {
                        let left = left.ok_or_else(|| "Expected expression before OR".to_string())?;
                        operands.insert(0, left);
                        self.fold_by_weight(operands, LogicalOp::Or).map(Some)
                    };

                    if let Ok(Some(ref mut result)) = result {
                        if let Some(Lexeme::RawString(ref s)) = self.next_lexeme() {
                            if s.to_lowercase() == "as" {
                                match self.next_lexeme() {
                                    Some(Lexeme::String(s)) | Some(Lexeme::RawString(s)) => {
                                        result.alias = Some(s);
                                    },
                                    _ => {
                                        self.drop_lexeme();
                                        self.drop_lexeme();
                                    }
                                }
                            } else {
                                self.drop_lexeme();
                            }
                        } else {
                            self.drop_lexeme();
                        }
                    }

                    return result;
                }
            }
        }
    }

    fn parse_and(&mut self) -> Result<Option<Expr>, String> {
        let left = self.parse_cond()?;

        let mut operands: Vec<Expr> = vec![];
        loop {
            let lexeme = self.next_lexeme();
            match lexeme {
                Some(Lexeme::And) => {
                    let expr = self.parse_cond()?;
                    operands.push(expr.ok_or_else(|| "Expected expression after AND".to_string())?);
                }
                _ => {
                    self.drop_lexeme();

                    if operands.is_empty() {
                        return Ok(left);
                    }
                    let left = left.ok_or_else(|| "Expected expression before AND".to_string())?;
                    operands.insert(0, left);
                    return self.fold_by_weight(operands, LogicalOp::And).map(Some);
                }
            }
        }
    }

    fn fold_by_weight(&self, operands: Vec<Expr>, logical_op: LogicalOp) -> Result<Expr, String> {
        let mut operands: Vec<Expr> = operands.into_iter().map(|operand| self.as_condition(operand)).collect();
        operands.sort_by_key(|operand| operand.weight);
        let mut operands = operands.into_iter();
        let mut result = operands.next().expect("a chain has at least one operand");
        for operand in operands {
            result = self.logical_op(result, logical_op.clone(), operand)?;
        }
        Ok(result)
    }

    fn parse_cond(&mut self) -> Result<Option<Expr>, String> {
        let mut negate = false;

        loop {
            if let Some(Lexeme::Not) = self.next_lexeme() {
                negate = !negate;
            } else {
                self.drop_lexeme();
                break;
            }
        }

        if !negate {
            return self.parse_comparison();
        }

        // Under NOT a contradiction such as `not (is_file and is_dir)` is a
        // tautology, so the file-type conflict check must stay quiet.
        self.negation_depth += 1;
        let result = self.parse_comparison();
        self.negation_depth -= 1;

        match result {
            Ok(Some(expr)) => Ok(Some(self.as_condition(expr).negated())),
            other => other,
        }
    }

    fn as_condition(&self, expr: Expr) -> Expr {
        if self.wraps_bare_booleans() || expr.op.is_some() || expr.logical_op.is_some() {
            return expr;
        }
        Expr::op(expr, Op::Eq, Expr::value(String::from("true")))
    }

    fn parse_comparison(&mut self) -> Result<Option<Expr>, String> {
        let mut exists_present = false;

        let left = self.parse_add_sub();

        let mut not = false;

        let lexeme = self.next_lexeme();
        match lexeme {
            Some(Lexeme::Not) => {
                not = true;
            }
            _ => {
                self.drop_lexeme();
            }
        };

        let lexeme = self.next_lexeme();

        if left.is_err()
            && let Some(Lexeme::Operator(s)) = lexeme
                && matches!(s.to_lowercase().as_str(), "exists" | "notexists") {
                    exists_present = true;
                }
        self.drop_lexeme();
        let left = match exists_present {
            true => {
                Some(Expr::field(Field::Name))
            },
            false => {
                left?
            }
        };

        let lexeme = self.next_lexeme();
        let mut result = match lexeme {
            Some(Lexeme::Operator(s)) if s.as_str() == "between" || s.as_str() == "notbetween" => {
                let is_negated = not ^ (s.as_str() == "notbetween");

                let left_between = self.parse_add_sub()?;

                let and_lexeme = self.next_lexeme();
                if and_lexeme.is_none() || and_lexeme.unwrap() != Lexeme::And {
                    return Err(String::from("Error parsing BETWEEN operator"));
                }

                let right_between = self.parse_add_sub()?;

                let left_val = left.ok_or_else(|| "Expected expression before BETWEEN".to_string())?;
                let left_between = left_between.ok_or_else(|| "Expected expression in BETWEEN range".to_string())?;
                let right_between = right_between.ok_or_else(|| "Expected expression in BETWEEN range".to_string())?;
                let left_expr = Expr::op(
                    left_val.clone(),
                    match is_negated {
                        false => Op::Gte,
                        true => Op::Lt,
                    },
                    left_between,
                );
                let right_expr = Expr::op(
                    left_val,
                    match is_negated {
                        false => Op::Lte,
                        true => Op::Gt,
                    },
                    right_between,
                );

                Ok(Some(self.logical_op(
                    left_expr,
                    match is_negated {
                        false => LogicalOp::And,
                        true => LogicalOp::Or,
                    },
                    right_expr,
                )?))
            }
            Some(Lexeme::Operator(s)) if s.as_str() == "in" || s.as_str() == "exists" || s.as_str() == "notin" || s.as_str() == "notexists" => {
                let list = self.parse_list()?;
                // Like SQL: IN requires a single-column subquery, while
                // EXISTS accepts any column list.
                if s.as_str() == "in" || s.as_str() == "notin" {
                    Self::ensure_single_column_subquery(&list, "IN")?;
                }
                let op = Op::from_with_not(&s, not)
                    .ok_or_else(|| format!("Unknown operator: {}", s))?;
                let left = left.ok_or_else(|| "Expected expression before operator".to_string())?;
                Ok(Some(Expr::op(
                    left,
                    op,
                    list,
                )))
            }
            Some(Lexeme::Operator(s)) if s.as_str() == "is" => {
                let mut negated = not;
                let mut word = self.next_lexeme();
                if matches!(word, Some(Lexeme::RawString(ref w)) if w.eq_ignore_ascii_case("not")) {
                    negated = !negated;
                    word = self.next_lexeme();
                }
                if !matches!(word, Some(Lexeme::RawString(ref w)) if w.eq_ignore_ascii_case("null")) {
                    return Err(String::from("Expected NULL after IS"));
                }
                let left = left.ok_or_else(|| "Expected expression before IS NULL".to_string())?;
                let op = if negated { Op::IsNotNull } else { Op::IsNull };
                Ok(Some(Expr::op(left, op, Expr::value(String::new()))))
            }
            Some(Lexeme::Operator(s)) => {
                let right = self.parse_add_sub()?;
                let op = Op::from_with_not(&s, not);
                match op {
                    Some(op) => {
                        let left = left.ok_or_else(|| "Expected expression before operator".to_string())?;
                        let right = right.ok_or_else(|| "Expected expression after operator".to_string())?;
                        Ok(Some(Expr::op(left, op, right)))
                    }
                    None => Err(format!("Unknown operator: {}", s)),
                }
            }
            _ => {
                self.drop_lexeme();
                Ok(left)
            }
        };

        if let Ok(Some(ref expr)) = result {
            let should_wrap = if let Some(ref field) = expr.field {
                expr.left.is_none()
                    && expr.right.is_none()
                    && field.is_boolean_field()
                    && self.wraps_bare_booleans()
            } else { false };
            let should_wrap_func = if let Some(ref function) = expr.function {
                expr.right.is_none()
                    && (expr.args.is_none() || expr.args.as_ref().is_some_and(|a| a.is_empty()))
                    && function.is_boolean_function()
                    && self.wraps_bare_booleans()
            } else { false };
            let should_wrap_literal = expr.is_literal()
                && !expr.minus
                && expr.val.as_ref().is_some_and(|val| val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("false"))
                && self.wraps_bare_booleans();
            if should_wrap {
                // Reuse the parsed field as-is: it already carries its root
                // alias and alias-scoped props.
                let expr = result.unwrap().unwrap();
                result = Ok(Some(Expr::op(expr, Op::Eq, Expr::value(String::from("true")))));
            } else if should_wrap_func {
                let expr = result.unwrap().unwrap();
                let func_expr = Expr::function_left(expr.function.unwrap(), expr.left);
                result = Ok(Some(Expr::op(
                    func_expr,
                    Op::Eq,
                    Expr::value(String::from("true")),
                )));
            } else if should_wrap_literal {
                let expr = result.unwrap().unwrap();
                let literal = Expr::value(expr.val.unwrap().to_ascii_lowercase());
                result = Ok(Some(Expr::op(literal, Op::Eq, Expr::value(String::from("true")))));
            }
        }

        result
    }

    /// A bare boolean in WHERE or HAVING is a condition (`where is_file`),
    /// but inside a function's argument list or an IN list it is a value.
    fn wraps_bare_booleans(&self) -> bool {
        self.args_depth == 0 && self.roots_parsed && (!self.where_parsed || self.in_having)
    }

    fn ensure_boolean_conditions(mut expr: Expr, clause: &str) -> Result<Expr, String> {
        if expr.logical_op.is_some() {
            if let Some(left) = expr.left.take() {
                expr.left = Some(Box::new(Self::ensure_boolean_conditions(*left, clause)?));
            }
            if let Some(right) = expr.right.take() {
                expr.right = Some(Box::new(Self::ensure_boolean_conditions(*right, clause)?));
            }
            return Ok(expr);
        }
        if expr.op.is_some() {
            return Ok(expr);
        }
        Err(format!("Expected a comparison or boolean condition in {clause}, found '{}'", expr.to_string().to_lowercase()))
    }

    fn logical_op(&self, left: Expr, logical_op: LogicalOp, right: Expr) -> Result<Expr, String> {
        if self.negation_depth > 0 {
            Ok(Expr::negated_logical_op(left, logical_op, right))
        } else {
            Expr::logical_op(left, logical_op, right)
        }
    }

    /// A root alias that is not one of this query's own is a correlated
    /// reference to an enclosing query. Before the roots are known (the
    /// SELECT list) every alias is taken to be local.
    fn call_follows(&mut self) -> bool {
        let next = self.next_lexeme();
        self.drop_lexeme();
        matches!(next, Some(Lexeme::Open) | Some(Lexeme::CurlyOpen))
    }

    fn is_local_alias(&self, root_alias: Option<&str>) -> bool {
        match root_alias {
            Some(alias) => !self.roots_parsed || self.root_aliases.iter().any(|local| local == alias),
            None => true,
        }
    }

    fn parse_add_sub(&mut self) -> Result<Option<Expr>, String> {
        let mut left = self.parse_mul_div()?;

        let mut op = None;
        loop {
            let lexeme = self.next_lexeme();
            if let Some(Lexeme::ArithmeticOperator(s)) = lexeme {
                let new_op = ArithmeticOp::from(&s);
                match new_op {
                    Some(ArithmeticOp::Add) | Some(ArithmeticOp::Subtract) => {
                        let expr = self.parse_mul_div()?;
                        if op.is_none() {
                            op = new_op.clone();
                        }

                        left = match left {
                            Some(left) => {
                                let expr = expr.ok_or_else(|| "Expected expression after arithmetic operator".to_string())?;
                                Some(Expr::arithmetic_op(left, new_op.unwrap(), expr))
                            }
                            None => expr,
                        };
                    }
                    _ => {
                        self.drop_lexeme();

                        return Ok(left);
                    }
                }
            } else {
                self.drop_lexeme();

                return Ok(left);
            }
        }
    }

    fn parse_mul_div(&mut self) -> Result<Option<Expr>, String> {
        let mut left = self.parse_paren()?;

        loop {
            let lexeme = self.next_lexeme();
            if let Some(Lexeme::ArithmeticOperator(s)) = lexeme {
                let new_op = ArithmeticOp::from(&s);
                match new_op {
                    Some(ArithmeticOp::Multiply)
                    | Some(ArithmeticOp::Divide)
                    | Some(ArithmeticOp::Modulo) => {
                        let expr = self.parse_paren()?;

                        left = match left {
                            Some(left) => {
                                let expr = expr.ok_or_else(|| "Expected expression after arithmetic operator".to_string())?;
                                Some(Expr::arithmetic_op(left, new_op.unwrap(), expr))
                            }
                            None => expr,
                        };
                    }
                    _ => {
                        self.drop_lexeme();

                        return Ok(left);
                    }
                }
            } else {
                self.drop_lexeme();

                return Ok(left);
            }
        }
    }

    /// SQL allows a subquery in a value position (and in IN lists) only when
    /// it selects exactly one column; cf. Postgres' "subquery has too many
    /// columns" / "subquery must return only one column" errors.
    fn ensure_single_column_subquery(expr: &Expr, context: &str) -> Result<(), String> {
        if let Some(ref subquery) = expr.subquery
            && subquery.fields.len() > 1 {
                return Err(format!(
                    "{} subquery must return a single column, but it returns {} columns",
                    context,
                    subquery.fields.len()
                ));
            }
        Ok(())
    }

    fn parse_paren(&mut self) -> Result<Option<Expr>, String> {
        match self.next_lexeme() {
            Some(Lexeme::Open) => {
                match self.next_lexeme() {
                    Some(Lexeme::Select) => {
                        self.drop_lexeme();
                        self.drop_lexeme();
                        let list = self.parse_list()?;
                        Self::ensure_single_column_subquery(&list, "Scalar")?;
                        Ok(Some(list))
                    },
                    _ => {
                        self.drop_lexeme();
                        let result = self.parse_expr();
                        if let Some(Lexeme::Close) = self.next_lexeme() {
                            result
                        } else {
                            Err("Unmatched parenthesis".to_string())
                        }
                    }
                }
            }
            Some(Lexeme::CurlyOpen) => {
                match self.next_lexeme() {
                    Some(Lexeme::Select) => {
                        self.drop_lexeme();
                        self.drop_lexeme();
                        let list = self.parse_list()?;
                        Self::ensure_single_column_subquery(&list, "Scalar")?;
                        Ok(Some(list))
                    },
                    _ => {
                        self.drop_lexeme();
                        let result = self.parse_expr();
                        if let Some(Lexeme::CurlyClose) = self.next_lexeme() {
                            result
                        } else {
                            Err("Unmatched parenthesis".to_string())
                        }
                    }
                }
            }
            _ => {
                self.drop_lexeme();
                self.parse_func_scalar()
            }
        }
    }

    fn parse_list(&mut self) -> Result<Expr, String> {
        match self.next_lexeme() {
            Some(Lexeme::Open) => {
                let result = {
                    if let Some(Lexeme::Select) = self.next_lexeme() {
                        self.lexer.push_state();
                        let mut parser = Parser::nested(self.lexer);
                        let query = parser.parse(self.debug)?;
                        self.lexer.pop_state();
                        self.push_lexeme(Lexeme::Close);
                        Expr::subquery(query)
                    } else {
                        self.drop_lexeme();
                        let mut result = Expr::new();
                        result.set_args(self.parse_args()?);
                        result
                    }
                };

                if let Some(Lexeme::Close) = self.next_lexeme() {
                    Ok(result)
                } else {
                    self.drop_lexeme();
                    Err("Unmatched parenthesis".to_string())
                }
            }
            Some(Lexeme::CurlyOpen) => {
                let result = {
                    if let Some(Lexeme::Select) = self.next_lexeme() {
                        self.lexer.push_state();
                        let mut parser = Parser::nested(self.lexer);
                        let query = parser.parse(self.debug)?;
                        self.lexer.pop_state();
                        self.push_lexeme(Lexeme::CurlyClose);
                        Expr::subquery(query)
                    } else {
                        self.drop_lexeme();
                        let mut result = Expr::new();
                        result.set_args(self.parse_args()?);
                        result
                    }
                };

                if let Some(Lexeme::CurlyClose) = self.next_lexeme() {
                    Ok(result)
                } else {
                    self.drop_lexeme();
                    Err("Unmatched parenthesis".to_string())
                }
            }
            _ => {
                self.drop_lexeme();
                Err("Error parsing list".to_string())
            }
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, String> {
        self.args_depth += 1;
        let args = self.parse_args_inner();
        self.args_depth -= 1;
        args
    }

    fn parse_args_inner(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = vec![];

        loop {
            let lexeme = self.next_lexeme();
            self.drop_lexeme();
            match lexeme {
                Some(Lexeme::Close) | Some(Lexeme::CurlyClose) | None if args.is_empty() => {
                    return Err("IN list cannot be empty".to_string());
                }
                Some(Lexeme::Close) | Some(Lexeme::CurlyClose) | None => {
                    return Err("Missing value after ',' in IN list".to_string());
                }
                Some(Lexeme::Comma) if args.is_empty() => {
                    return Err("Missing value before ',' in IN list".to_string());
                }
                Some(Lexeme::Comma) => {
                    return Err("Missing value after ',' in IN list".to_string());
                }
                _ => {}
            }

            match self.parse_expr()? {
                Some(expr) => args.push(expr),
                None => return Err("Expected a value in IN list".to_string()),
            }

            match self.next_lexeme() {
                Some(Lexeme::Comma) => {}
                Some(Lexeme::Close) | Some(Lexeme::CurlyClose) | None => {
                    self.drop_lexeme();
                    return Ok(args);
                }
                Some(_) => {
                    self.drop_lexeme();
                    return Err("Expected ',' or closing parenthesis in IN list".to_string());
                }
            }
        }
    }

    fn parse_func_scalar(&mut self) -> Result<Option<Expr>, String> {
        let mut lexeme = self.next_lexeme();
        let mut minus = false;

        if let Some(Lexeme::ArithmeticOperator(ref s)) = lexeme {
            if s == "-" {
                minus = true;
                lexeme = self.next_lexeme();
            } else if s == "+" {
                lexeme = self.next_lexeme();
            } else {
                self.drop_lexeme();
                return Err("Error parsing expression, expecting string".to_string());
            }
        }

        // Unary minus before a parenthesized expression: -(1+2). The toggle
        // makes -(-1) cancel out instead of silently staying negative.
        if minus && matches!(lexeme, Some(Lexeme::Open) | Some(Lexeme::CurlyOpen)) {
            self.drop_lexeme();
            return match self.parse_paren()? {
                Some(mut expr) => {
                    expr.minus = !expr.minus;
                    Ok(Some(expr))
                }
                None => Err("Error parsing expression, expecting string".to_string()),
            };
        }

        match lexeme {
            Some(Lexeme::Error(ref msg)) => {
                Err(msg.clone())
            }
            Some(Lexeme::String(ref s)) => {
                // Quoted strings are always literals, never column references:
                // `where name = 'size'` must compare against the string "size",
                // not against the size field.
                let mut expr = Expr::value(s.to_string());
                expr.minus = minus;

                Ok(Some(expr))
            }
            Some(Lexeme::RawString(ref s)) => {
                if let Some(aliased) = self
                    .select_aliases
                    .iter()
                    .find(|f| f.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(s)))
                    .cloned()
                    && !self.call_follows()
                {
                    let mut expr = aliased;
                    if minus {
                        expr.minus = !expr.minus;
                    }
                    return Ok(Some(expr));
                }

                if let Ok((field, root_alias)) = Field::parse_field(s) {
                    let mut expr = match root_alias {
                        Some(alias) if !self.is_local_alias(Some(&alias)) => Expr::external_field(field, alias),
                        root_alias => Expr::field_with_root_alias(field, root_alias),
                    };
                    expr.minus = minus;
                    return Ok(Some(expr));
                }

                // Once a function name is recognised, argument errors must
                // propagate: silently falling back to the literal "upper"
                // would leave the already-consumed lexemes unrewound and
                // turn `where length(name > 3` into a constant condition.
                if let Ok(function) = Function::from_str(s)
                    && (function.arity().min == 0 || self.call_follows())
                {
                    let mut expr = self.parse_function(function)?;
                    expr.minus = minus;
                    return Ok(Some(expr));
                }

                // After a comparison operator the lexer emits a bare "-" as a
                // RawString (start of a negative literal), so `size gt -(1+2)`
                // arrives here rather than as an arithmetic operator.
                if s == "-" {
                    let next = self.next_lexeme();
                    if matches!(next, Some(Lexeme::Open) | Some(Lexeme::CurlyOpen)) {
                        self.drop_lexeme();
                        return match self.parse_paren()? {
                            Some(mut expr) => {
                                expr.minus = !expr.minus;
                                if minus {
                                    expr.minus = !expr.minus;
                                }
                                Ok(Some(expr))
                            }
                            None => Err("Error parsing expression, expecting string".to_string()),
                        };
                    }
                    self.drop_lexeme();
                }

                let is_identifier = |part: &str| {
                    part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && part.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                };
                if let Some((prefix, name)) = s.split_once('.')
                    && is_identifier(prefix)
                    && is_identifier(name)
                {
                    let mut expr = Expr::alias_reference(prefix.to_string(), s.to_string());
                    expr.minus = minus;
                    return Ok(Some(expr));
                }

                let mut expr = Expr::value(s.to_string());
                expr.minus = minus;

                Ok(Some(expr))
            }
            _ => {
                self.drop_lexeme();
                Err("Error parsing expression, expecting string".to_string())
            }
        }
    }

    fn parse_function(&mut self, function: Function) -> Result<Expr, String> {
        self.args_depth += 1;
        let parsed = self.parse_function_inner(function);
        self.args_depth -= 1;
        parsed
    }

    fn parse_function_inner(&mut self, function: Function) -> Result<Expr, String> {
        let mut function_expr = Expr::function(function);

        let mut curly_mode = false;
        if let Some(lexeme) = self.next_lexeme() {
            if lexeme != Lexeme::Open && lexeme != Lexeme::CurlyOpen {
                self.drop_lexeme();
                return Ok(function_expr);
            }

            if lexeme == Lexeme::CurlyOpen {
                curly_mode = true;
            }
        } else {
            return Ok(function_expr);
        }

        let is_close = |lexeme: &Lexeme| {
            (*lexeme == Lexeme::Close && !curly_mode) || (*lexeme == Lexeme::CurlyClose && curly_mode)
        };

        // Empty argument list, e.g. `curdate()`
        match self.next_lexeme() {
            Some(ref lexeme) if is_close(lexeme) => return Ok(function_expr),
            _ => self.drop_lexeme(),
        }

        match self.parse_expr()? {
            Some(function_arg) => function_expr.add_left(function_arg),
            None => return Err(format!("Error in function expression: {}", function_expr)),
        }

        let mut args = vec![];

        loop {
            match self.next_lexeme() {
                Some(Lexeme::Comma) => match self.parse_expr()? {
                    Some(expr) => args.push(expr),
                    None => {
                        return Err(format!("Error in function expression: {}", function_expr));
                    }
                },
                Some(ref lexeme) if is_close(lexeme) => {
                    function_expr.set_args(args);
                    return Ok(function_expr);
                }
                _ => {
                    return Err(format!("Missing closing parenthesis in function expression: {}", function_expr));
                }
            }
        }
    }

    /// Resolve a bare ORDER BY / GROUP BY identifier against SELECT-list
    /// aliases: `select size + 1 as s ... order by s` must use the aliased
    /// expression, not a constant string "s" (which would silently not sort).
    fn resolve_select_alias(&mut self, fields: &[Expr], name: &str) -> Option<Expr> {
        let aliased = fields
            .iter()
            .find(|f| f.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name)))?;

        // Only a bare alias is substituted; if the identifier is part of a
        // larger expression, leave it to the regular expression parser.
        let next = self.next_lexeme();
        self.drop_lexeme();
        if matches!(
            next,
            Some(Lexeme::ArithmeticOperator(_))
                | Some(Lexeme::Operator(_))
                | Some(Lexeme::And)
                | Some(Lexeme::Or)
                | Some(Lexeme::Open)
                | Some(Lexeme::CurlyOpen)
        ) {
            return None;
        }

        Some(aliased.clone())
    }

    fn followed_by_arithmetic_operator(&mut self) -> bool {
        let next = self.next_lexeme();
        self.drop_lexeme();
        matches!(next, Some(Lexeme::ArithmeticOperator(_)))
    }

    fn parse_group_by(&mut self, fields: &[Expr]) -> Result<Vec<Expr>, String> {
        self.select_aliases = fields.to_vec();
        let grouping = self.parse_group_by_clause(fields);
        self.select_aliases.clear();
        let grouping = grouping?;
        Self::reject_unknown_columns(&grouping, "GROUP BY")?;
        Ok(grouping)
    }

    fn reject_non_grouped_columns(&self, fields: &[Expr], grouping_fields: &[Expr], having: Option<&Expr>) -> Result<(), String> {
        let aggregated = !grouping_fields.is_empty()
            || fields.iter().any(Expr::has_aggregate_function)
            || having.is_some_and(Expr::has_aggregate_function);
        if !aggregated {
            return Ok(());
        }
        let grouping_keys: Vec<String> = grouping_fields.iter().map(Expr::cache_key).collect();
        match fields.iter().find_map(|column| self.non_grouped_field(column, &grouping_keys)) {
            Some(field) => {
                let prefix = field.root_alias.as_deref().map(|alias| format!("{alias}.")).unwrap_or_default();
                let name = match (field.field, &field.function) {
                    (Some(field), _) => field.to_string().to_lowercase(),
                    (None, Some(function)) => format!("{}()", function.canonical_name()),
                    (None, None) => String::new(),
                };
                Err(format!(
                    "Column '{prefix}{name}' must appear in GROUP BY or be used inside an aggregate function"
                ))
            }
            None => Ok(()),
        }
    }

    fn non_grouped_field<'e>(&self, expr: &'e Expr, grouping_keys: &[String]) -> Option<&'e Expr> {
        if expr.is_literal() || expr.subquery.is_some() || grouping_keys.contains(&expr.cache_key()) {
            return None;
        }
        if expr.function.as_ref().is_some_and(Function::is_aggregate_function) {
            return None;
        }
        if !self.is_local_alias(expr.root_alias.as_deref()) {
            return None;
        }
        if expr.function.as_ref().is_some_and(Function::reads_entry) {
            return Some(expr);
        }
        if expr.field.is_some() {
            return Some(expr);
        }
        expr.left
            .iter()
            .chain(expr.right.iter())
            .map(Box::as_ref)
            .chain(expr.args.iter().flatten())
            .find_map(|child| self.non_grouped_field(child, grouping_keys))
    }

    fn reject_unknown_columns(exprs: &[Expr], clause: &str) -> Result<(), String> {
        match exprs.iter().find(|expr| expr.is_literal()) {
            Some(literal) => Err(format!("Unknown column '{}' in {}", literal.val.as_deref().unwrap_or_default(), clause)),
            None => Ok(()),
        }
    }

    fn parse_group_by_clause(&mut self, fields: &[Expr]) -> Result<Vec<Expr>, String> {
        let mut group_by_fields: Vec<Expr> = vec![];

        if let Some(Lexeme::Group) = self.next_lexeme() {
            if let Some(Lexeme::By) = self.next_lexeme() {
                loop {
                    match self.next_lexeme() {
                        Some(Lexeme::Comma) => {}
                        Some(Lexeme::RawString(ref grouping_field)) => {
                            let actual_field = match grouping_field.parse::<usize>() {
                                Ok(_) if self.followed_by_arithmetic_operator() => {
                                    self.drop_lexeme();
                                    match self.parse_expr()? {
                                        Some(expr) => expr,
                                        None => break,
                                    }
                                }
                                Ok(idx) if idx >= 1 && idx <= fields.len() => fields[idx - 1].clone(),
                                Ok(_) => return Err(String::from("Group by field index is out of range")),
                                _ => match self.resolve_select_alias(fields, grouping_field) {
                                    Some(expr) => expr,
                                    None => {
                                        self.drop_lexeme();
                                        match self.parse_expr()? {
                                            Some(expr) => expr,
                                            None => break,
                                        }
                                    }
                                },
                            };
                            group_by_fields.push(actual_field);
                        }
                        Some(Lexeme::ArithmeticOperator(ref op)) if op == "-" => {
                            self.drop_lexeme();
                            match self.parse_expr()? {
                                Some(expr) => group_by_fields.push(expr),
                                None => break,
                            }
                        }
                        Some(Lexeme::String(ref grouping_field)) => {
                            // Grouping by a string literal is meaningless, so a
                            // quoted string here keeps its historical meaning
                            // as a field name (or a SELECT-list alias).
                            if let Ok((field, root_alias)) = Field::parse_field(grouping_field) {
                                group_by_fields.push(Expr::field_with_root_alias(field, root_alias));
                            } else if let Some(expr) = self.resolve_select_alias(fields, grouping_field) {
                                group_by_fields.push(expr);
                            } else {
                                self.drop_lexeme();
                                match self.parse_expr()? {
                                    Some(group_field) => group_by_fields.push(group_field),
                                    None => break,
                                }
                            }
                        }
                        Some(Lexeme::Open) | Some(Lexeme::CurlyOpen) | Some(Lexeme::Not) => {
                            self.drop_lexeme();
                            match self.parse_expr()? {
                                Some(group_field) => group_by_fields.push(group_field),
                                None => break,
                            }
                        }
                        _ => {
                            self.drop_lexeme();
                            break;
                        }
                    }
                }
            } else {
                self.drop_lexeme();
            }
        } else {
            self.drop_lexeme();
        }

        Ok(group_by_fields)
    }

    fn parse_having(&mut self, fields: &[Expr]) -> Result<Option<Expr>, String> {
        match self.next_lexeme() {
            Some(Lexeme::Having) => {
                self.select_aliases = fields.to_vec();
                self.in_having = true;
                let having = self.parse_expr();
                self.in_having = false;
                self.select_aliases.clear();
                match having? {
                    Some(having) => Ok(Some(having)),
                    None => Err("Expected expression after HAVING".to_string()),
                }
            }
            _ => {
                self.drop_lexeme();
                Ok(None)
            }
        }
    }

    fn contains_op_unsupported_in_having(expr: &Expr) -> bool {
        if matches!(expr.op, Some(Op::Exists) | Some(Op::NotExists)) {
            return true;
        }
        expr.left.as_ref().is_some_and(|e| Self::contains_op_unsupported_in_having(e))
            || expr.right.as_ref().is_some_and(|e| Self::contains_op_unsupported_in_having(e))
    }

    fn parse_order_by(&mut self, fields: &[Expr]) -> Result<(Vec<Expr>, Vec<bool>), String> {
        self.select_aliases = fields.to_vec();
        let ordering = self.parse_order_by_clause(fields);
        self.select_aliases.clear();
        let ordering = ordering?;
        Self::reject_unknown_columns(&ordering.0, "ORDER BY")?;
        Ok(ordering)
    }

    fn parse_order_by_clause(&mut self, fields: &[Expr]) -> Result<(Vec<Expr>, Vec<bool>), String> {
        let mut order_by_fields: Vec<Expr> = vec![];
        let mut order_by_directions: Vec<bool> = vec![];

        if let Some(Lexeme::Order) = self.next_lexeme() {
            if let Some(Lexeme::By) = self.next_lexeme() {
                loop {
                    match self.next_lexeme() {
                        Some(Lexeme::Comma) => {}
                        Some(Lexeme::RawString(ref ordering_field)) => {
                            let actual_field = match ordering_field.parse::<usize>() {
                                Ok(_) if self.followed_by_arithmetic_operator() => {
                                    self.drop_lexeme();
                                    match self.parse_expr()? {
                                        Some(expr) => expr,
                                        None => break,
                                    }
                                }
                                Ok(idx) if idx >= 1 && idx <= fields.len() => fields[idx - 1].clone(),
                                Ok(_) => return Err(String::from("Order by field index is out of range")),
                                _ => match self.resolve_select_alias(fields, ordering_field) {
                                    Some(expr) => expr,
                                    None => {
                                        self.drop_lexeme();
                                        match self.parse_expr()? {
                                            Some(expr) => expr,
                                            None => break,
                                        }
                                    }
                                },
                            };
                            order_by_fields.push(actual_field);
                            order_by_directions.push(true);
                        }
                        Some(Lexeme::ArithmeticOperator(ref op)) if op == "-" => {
                            self.drop_lexeme();
                            match self.parse_expr()? {
                                Some(expr) => {
                                    order_by_fields.push(expr);
                                    order_by_directions.push(true);
                                }
                                None => break,
                            }
                        }
                        Some(Lexeme::String(ref ordering_field)) => {
                            // Ordering by a string literal is meaningless, so a
                            // quoted string here keeps its historical meaning
                            // as a field name (or a SELECT-list alias).
                            if let Ok((field, root_alias)) = Field::parse_field(ordering_field) {
                                order_by_fields.push(Expr::field_with_root_alias(field, root_alias));
                                order_by_directions.push(true);
                            } else if let Some(expr) = self.resolve_select_alias(fields, ordering_field) {
                                order_by_fields.push(expr);
                                order_by_directions.push(true);
                            } else {
                                self.drop_lexeme();
                                match self.parse_expr()? {
                                    Some(expr) => {
                                        order_by_fields.push(expr);
                                        order_by_directions.push(true);
                                    }
                                    None => break,
                                }
                            }
                        }
                        Some(Lexeme::Open) | Some(Lexeme::CurlyOpen) | Some(Lexeme::Not) => {
                            self.drop_lexeme();
                            match self.parse_expr()? {
                                Some(expr) => {
                                    order_by_fields.push(expr);
                                    order_by_directions.push(true);
                                }
                                None => break,
                            }
                        }
                        Some(Lexeme::DescendingOrder) => {
                            if let Some(last) = order_by_directions.last_mut() {
                                *last = false;
                            }
                        }
                        _ => {
                            self.drop_lexeme();
                            break;
                        }
                    }
                }
            } else {
                self.drop_lexeme();
            }
        } else {
            self.drop_lexeme();
        }

        Ok((order_by_fields, order_by_directions))
    }

    fn parse_limit(&mut self) -> Result<(u32, u32), &str> {
        let lexeme = self.next_lexeme();
        match lexeme {
            Some(Lexeme::Limit) => {
                let lexeme = self.next_lexeme();
                return match lexeme {
                    Some(Lexeme::RawString(s)) | Some(Lexeme::String(s)) => {
                        // Parse the first number (could be limit alone or offset in offset,limit format)
                        if let Ok(first_number) = s.parse::<u32>() {
                            if let Some(Lexeme::Comma) = self.next_lexeme() {
                                // This is offset,limit format (first_number=offset, second_number=limit)
                                let second_lexeme = self.next_lexeme();
                                match second_lexeme {
                                    Some(Lexeme::RawString(s2)) | Some(Lexeme::String(s2)) => {
                                        if let Ok(second_number) = s2.parse::<u32>() {
                                            Ok((second_number, first_number))
                                        } else {
                                            self.drop_lexeme();
                                            Err("Error parsing limit value in offset,limit format")
                                        }
                                    }
                                    _ => {
                                        self.drop_lexeme();
                                        Err("Error parsing limit value in offset,limit format")
                                    }
                                }
                            } else {
                                self.drop_lexeme();
                                Ok((first_number, 0))
                            }
                        } else {
                            self.drop_lexeme();
                            Err("Error parsing limit")
                        }
                    }
                    _ => {
                        self.drop_lexeme();
                        Err("Error parsing limit, limit value not found")
                    }
                }
            }
            _ => {
                self.drop_lexeme();
            }
        }

        Ok((0, 0))
    }

    fn parse_offset(&mut self) -> Result<u32, &str> {
        let lexeme = self.next_lexeme();
        match lexeme {
            Some(Lexeme::Offset) => {
                let lexeme = self.next_lexeme();
                return match lexeme {
                    Some(Lexeme::RawString(s)) | Some(Lexeme::String(s)) => {
                        if let Ok(offset) = s.parse() {
                            Ok(offset)
                        } else {
                            Err("Error parsing offset")
                        }
                    }
                    _ => {
                        self.drop_lexeme();
                        Err("Error parsing offset, offset value not found")
                    }
                }
            }
            _ => {
                self.drop_lexeme();
            }
        }

        Ok(0)
    }

    fn parse_output_format(&mut self) -> Result<OutputFormat, &str> {
        let lexeme = self.next_lexeme();
        match lexeme {
            Some(Lexeme::Into) => {
                let lexeme = self.next_lexeme();
                match lexeme {
                    Some(Lexeme::RawString(s)) | Some(Lexeme::String(s)) => {
                        return match OutputFormat::from(&s) {
                            Some(output_format) => Ok(output_format),
                            None => Err("Unknown output format"),
                        };
                    }
                    _ => {
                        self.drop_lexeme();
                        return Err("Error parsing output format");
                    }
                }
            }
            _ => {
                self.drop_lexeme();
            }
        }

        Ok(OutputFormat::Tabs)
    }

    pub(crate) fn there_are_remaining_lexemes(&mut self) -> bool {
        let result = self.next_lexeme().is_some();
        if result {
            self.drop_lexeme();
        }

        result
    }

    fn next_lexeme(&mut self) -> Option<Lexeme> {
        self.index += 1;

        match self.lexemes.get(self.index as usize) {
            Some(lexeme) => {
                Some(lexeme.clone())
            }
            None => {
                let lexeme = self.lexer.next_lexeme();
                match lexeme {
                    Some(ref lexeme) => {
                        if self.debug {
                            dbg!(&lexeme);
                        }

                        self.lexemes.push(lexeme.clone());

                        Some(lexeme.clone())
                    }
                    _ => {
                        None
                    }
                }
            }
        }
    }

    fn push_lexeme(&mut self, lexeme: Lexeme) {
        self.lexemes.push(lexeme);
    }

    fn drop_lexeme(&mut self) {
        self.index -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{IS_DIR, IS_FILE, IS_PIPE, IS_SOCKET};

    fn parse_where(query: &str) -> Expr {
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes(), "leftover lexemes in {:?}", query);
        query.expr.unwrap()
    }

    fn parse_err(query: &str) -> String {
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        p.parse(false).err().unwrap_or_else(|| panic!("{} should not parse", query))
    }

    #[test]
    fn bare_booleans_under_not_and_or_inside_arguments_become_conditions() {
        let expr = parse_where("select name from /test where coalesce(not is_file, 0) = false");
        let negated = expr.left.as_ref().unwrap().left.as_ref().unwrap();
        assert_eq!(negated.op, Some(Op::Ne));
        assert_eq!(negated.left.as_ref().unwrap().field, Some(Field::IsFile));

        let expr = parse_where("select name from /test where coalesce(is_file and size > 2, 0) = true");
        let conjunction = expr.left.as_ref().unwrap().left.as_ref().unwrap();
        assert_eq!(conjunction.logical_op, Some(LogicalOp::And));
        for side in [&conjunction.left, &conjunction.right] {
            assert!(side.as_ref().unwrap().op.is_some());
        }
    }

    #[test]
    fn select_columns_accept_not_and_or_conditions() {
        let query = parse_query("select not is_file, coalesce(not is_file, 0), is_file and size > 2 as big from /test").unwrap();
        assert_eq!(query.fields.len(), 3);
        assert_eq!(query.fields[0].op, Some(Op::Ne));
        assert_eq!(query.fields[0].left.as_ref().unwrap().field, Some(Field::IsFile));
        assert_eq!(query.fields[1].left.as_ref().unwrap().op, Some(Op::Ne));
        assert_eq!(query.fields[2].logical_op, Some(LogicalOp::And));
        assert_eq!(query.fields[2].alias.as_deref(), Some("big"));
    }

    #[test]
    fn not_flips_only_the_outer_comparison_of_a_nested_one() {
        let expr = parse_where("select name from /test where not ((size > 2) = false)");
        assert_eq!(expr.op, Some(Op::Ne));
        assert_eq!(expr.left.as_ref().unwrap().op, Some(Op::Gt));
    }

    #[test]
    fn operator_less_conditions_are_rejected() {
        for (query, found) in [
            ("select name from . where name", "name"),
            ("select name from . where 1", "1"),
            ("select name from . where and", "and"),
            ("select name from . where (size)", "size"),
            ("select name from . where size + 1", "size + 1"),
            ("select name from . where lower(name)", "lower(name)"),
            ("select name from . where not name", "name"),
            ("select name from . where size > 100 and name", "name"),
            ("select name from . where name = 'a' or (size)", "size"),
            ("select name from . where size > 100 and (name)", "name"),
            ("select name from . where name in (select name from . where size)", "size"),
            ("select name from . where (select count(*) from .)", "subquery_"),
        ] {
            let err = parse_err(query);
            assert!(
                err.starts_with("Expected a comparison or boolean condition in WHERE, found '") && err.contains(found),
                "{}: unexpected error text {:?}", query, err
            );
        }
        let err = parse_err("select ext, count(*) from . group by ext having count(*)");
        assert_eq!(err, "Expected a comparison or boolean condition in HAVING, found 'count(*)'");
    }

    #[test]
    fn bare_boolean_fields_functions_and_literals_are_conditions() {
        for query in [
            "select name from . where is_dir",
            "select name from . where not is_dir",
            "select name from . where is_dir and size > 0",
            "select name from . where contains('x')",
            "select name from . where (size) > 100",
            "select name from . where (size > 100)",
            "select name from . where exists (select name from .)",
            "select name from . where true",
            "select name from . where TRUE or size > 0",
            "select ext, count(*) from . group by ext having count(*) > 1",
            "select is_dir, count(*) from . group by is_dir having is_dir",
        ] {
            parse_query(query).expect(query);
        }

        let expr = parse_where("select name from . where true");
        assert_eq!(expr.op, Some(Op::Eq));
        assert_eq!(expr.left.as_ref().unwrap().val.as_deref(), Some("true"));
        assert_eq!(expr.right.as_ref().unwrap().val.as_deref(), Some("true"));

        let expr = parse_where("select name from . where not False");
        assert_eq!(expr.op, Some(Op::Ne));
        assert_eq!(expr.left.as_ref().unwrap().val.as_deref(), Some("false"));
    }

    #[test]
    fn non_numeric_literal_against_numeric_field_is_a_parse_error() {
        for query in [
            "select name from . where size = ''",
            "select name from . where size > 'abc'",
            "select name from . where size > abc",
            "select name from . where size != 'abc'",
            "select name from . where 'abc' < size",
            "select name from . where size in ('abc')",
            "select name from . where size in (0, 'abc')",
            "select name from . where size not in ('')",
            "select name from . where name = 'x' and width > 'wide'",
            "select name from . where size > 0 or line_count = 'many'",
        ] {
            let err = parse_err(query);
            assert!(
                err.to_lowercase().contains("can't parse number") || err.to_lowercase().contains("cannot"),
                "{}: unexpected error text {:?}", query, err
            );
        }
        assert!(parse_err("select name from . where size > 'abc'").contains("'abc'"));
        assert!(parse_err("select name from . where width > 'wide'").contains("width"));
    }

    #[test]
    fn unparseable_datetime_literal_is_a_parse_error() {
        for query in [
            "select name from . where modified = 'not-a-date'",
            "select name from . where modified > oops",
            "select name from . where created in ('2026-01-01', 'bad')",
            "select name from . where size > 0 and modified = 'oops'",
            "select name from . where 'oops' < modified",
            "select name from (select name from . where modified = 'oops')",
        ] {
            let err = parse_err(query);
            assert!(err.contains("Can't parse datetime"), "{}: unexpected error text {:?}", query, err);
        }
        assert!(parse_err("select name from . where modified = 'not-a-date'").contains("not-a-date"));
    }

    #[test]
    fn valid_datetime_literals_still_parse() {
        for query in [
            "select name from . where modified = '2026-01-01'",
            "select name from . where modified > 2026-01-01",
            "select name from . where modified > '2026-01-01 10:00:00'",
            "select name from . where modified > today",
            "select name from . where modified > 'yesterday'",
            "select name from . where modified in ('2026-01-01', '2026-01-02')",
            "select name from . where modified = created",
            "select name from . where modified > date_add(created, 1)",
            "select name from . where name = 'not-a-date'",
        ] {
            parse_where(query);
        }
    }

    #[test]
    fn numeric_field_comparisons_that_coerce_still_parse() {
        for query in [
            "select name from . where size > '1kb'",
            "select name from . where size > 1kb",
            "select name from . where size > '0.5'",
            "select name from . where size > -5",
            "select name from . where size > name",
            "select name from . where size > length(name)",
            "select name from . where size > 2*4",
            "select name from . where size in (0, '1kb', 2mb)",
            "select name from . where fsize = '12 B'",
            "select name from . where exif_exposure_time = '1/250'",
            "select name from . where modified > 'yesterday'",
            "select name from . where name = 'abc'",
        ] {
            parse_where(query);
        }
        // subquery WHERE clauses are validated too, with no outer WHERE to unwrap
        let mut lexer = Lexer::new(vec!["select name from (select name from . where size > '1kb')".to_string()]);
        assert!(Parser::new(&mut lexer).parse(false).is_ok());
        assert!(parse_err("select name from (select name from . where size > 'abc')").contains("'abc'"));
    }

    #[test]
    fn unspaced_arithmetic_on_comparison_rhs_parses_like_spaced() {
        assert_eq!(
            parse_where("select name from . where size > 2*4"),
            parse_where("select name from . where size > 2 * 4")
        );
        assert_eq!(
            parse_where("select name from . where size > 10-5"),
            parse_where("select name from . where size > 10 - 5")
        );
        assert_eq!(
            parse_where("select name from . where size > 1+(2*2)"),
            parse_where("select name from . where size > 1 + (2 * 2)")
        );
    }

    #[test]
    fn simple_query() {
        let query = "select name, path ,size , fsize from /";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![
                Expr::field(Field::Name),
                Expr::field(Field::Path),
                Expr::field(Field::Size),
                Expr::field(Field::FormattedSize),
            ]
        );
    }

    #[test]
    fn query() {
        let query = "select name, path ,size , fsize from /test depth 2, /test2 archives,/test3 depth 3 archives , /test4 ,'/test5' gitignore , /test6 mindepth 3, /test7 archives DFS, /test8 dfs where name != 123 AND ( size gt 456 or fsize lte 758) or name = 'xxx' order by 2, size desc limit 50";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![
                Expr::field(Field::Name),
                Expr::field(Field::Path),
                Expr::field(Field::Size),
                Expr::field(Field::FormattedSize)
            ]
        );

        assert_eq!(
            query.roots,
            vec![
                Root::new(
                    String::from("/test"),
                    RootOptions::from(0, 2, false, false,  None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test2"),
                    RootOptions::from(0, 0, true, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test3"),
                    RootOptions::from(0, 3, true, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test4"),
                    RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test5"),
                    RootOptions::from(0, 0, false, false, Some(true), None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test6"),
                    RootOptions::from(3, 0, false, false, None, None, None, Bfs, false, None)
                ),
                Root::new(
                    String::from("/test7"),
                    RootOptions::from(0, 0, true, false, None, None, None, Dfs, false, None)
                ),
                Root::new(
                    String::from("/test8"),
                    RootOptions::from(0, 0, false, false, None, None, None, Dfs, false, None)
                ),
            ]
        );

        let expr = Expr::logical_op(
            Expr::op(
                Expr::field(Field::Name),
                Op::Eq,
                Expr::value(String::from("xxx")),
            ),
            LogicalOp::Or,
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
        ).unwrap();
        // Query expression must be reordered due to the weight difference of its branches
        assert_eq!(query.expr, Some(expr));
        assert_eq!(
            query.ordering_fields,
            vec![Expr::field(Field::Path), Expr::field(Field::Size)]
        );
        assert_eq!(query.ordering_asc, vec![true, false]);
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn query_with_not() {
        let query = "select name from /test where name not like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_single_not() {
        let query = "select name from /test where not name like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_multiple_not() {
        let query = "select name from /test where not name like '%.tmp' and not name like '%.tst'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let left = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );
        let right = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tst")),
        );
        let expr = Expr::logical_op(left, LogicalOp::And, right).unwrap();

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_multiple_not_paren() {
        let query =
            "select name from /test where (not name like '%.tmp') and (not name like '%.tst')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let left = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );
        let right = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tst")),
        );
        let expr = Expr::logical_op(left, LogicalOp::And, right).unwrap();

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_double_not() {
        let query = "select name from /test where not not name like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::Like,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_triple_not() {
        let query = "select name from /test where not not not name like '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn broken_query() {
        let query = "select name, path ,size , fsize from / where name != 'foobar' order by size desc limit 10 into csv this is unexpected";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false);

        assert!(query.is_ok());
        assert!(p.there_are_remaining_lexemes());
    }

    #[test]
    fn path_with_spaces() {
        let query = "select name from '/opt/Some Cool Dir/Test This'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/opt/Some Cool Dir/Test This"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
            ),]
        );
    }

    #[test]
    fn simple_boolean_syntax() {
        let query = "select name from /home/user where is_audio or is_video";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name from /home/user where is_audio = true or is_video = true";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn simple_boolean_function_syntax() {
        let query = "select name from /home/user where CONTAINS('foobar') or CONTAINS('bazz')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name from /home/user where CONTAINS('foobar') = true or CONTAINS('bazz') = true";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn simple_function_without_args_syntax() {
        let query = "select CURDATE()";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select CURDATE";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn from_at_the_end_of_the_query() {
        let query = "select name where not name like '%.tmp' from /test gitignore mindepth 2";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(2, 0, false, false, Some(true), None, None, Bfs, false, None)
            ),]
        );

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotLike,
            Expr::value(String::from("%.tmp")),
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_implicit_root() {
        let query = "select name, size";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.roots,
            vec![Root::new(String::from("."), RootOptions::new()),]
        );
    }

    #[test]
    fn query_with_implicit_root_and_root_options() {
        let query = "select name, size depth 2";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("."),
                RootOptions::from(0, 2, false, false, None, None, None, Bfs, false, None)
            ),]
        );
    }

    #[test]
    fn use_curly_braces() {
        let query = "select name, (1 + 2) from /home/user limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name, {1 + 2} from /home/user limit 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_group_by() {
        let query = "select AVG(size) from /test group by mime";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![Expr::function_left(
                Function::Avg,
                Some(Box::new(Expr::field(Field::Size)))
            )]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        assert_eq!(
            query.grouping_fields,
            vec![Expr::field(Field::Mime)]
        );
    }

    #[test]
    fn having_parses_expression() {
        let query = "select ext, count(*) from /test group by ext having count(*) > 5";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());
        let having = query.having.expect("HAVING expression not parsed");
        assert_eq!(having.op, Some(Op::Gt));
        assert_eq!(having.left.as_ref().unwrap().function, Some(Function::Count));
        assert_eq!(having.right.as_ref().unwrap().val, Some(String::from("5")));
    }

    #[test]
    fn having_with_logical_ops() {
        let query = "select ext from /test group by ext having count(*) > 1 and count(*) < 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        let having = query.having.expect("HAVING expression not parsed");
        assert!(having.logical_op.is_some());
    }

    #[test]
    fn having_before_order_by() {
        let query = "select ext, count(*) from /test group by ext having count(*) > 1 order by 2 desc limit 3";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());
        assert!(query.having.is_some());
        assert_eq!(query.ordering_fields.len(), 1);
        assert_eq!(query.ordering_asc, vec![false]);
        assert_eq!(query.limit, 3);
    }

    #[test]
    fn having_resolves_select_alias() {
        let query = "select ext, count(*) as c from /test group by ext having c > 5";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        let having = query.having.expect("HAVING expression not parsed");
        assert_eq!(
            having.left.as_ref().unwrap().function,
            Some(Function::Count),
            "alias 'c' must resolve to the aliased count(*) expression"
        );
    }

    #[test]
    fn quoted_having_literals_are_never_aliases() {
        let quoted = parse_query("select ext, count(*) as txt from /test group by ext having ext = 'txt'").unwrap();
        let right = quoted.having.unwrap().right.unwrap();
        assert!(right.is_literal() && right.val.as_deref() == Some("txt"), "quoted 'txt' must stay a literal, got {:?}", right);

        let bare = parse_query("select ext, count(*) as txt from /test group by ext having txt > 1").unwrap();
        let left = bare.having.unwrap().left.unwrap();
        assert_eq!(left.function, Some(Function::Count), "the bare identifier resolves to the alias");

        let function_named = parse_query("select ext, count(*) as len from /test group by ext having len > 1").unwrap();
        let left = function_named.having.unwrap().left.unwrap();
        assert_eq!(left.function, Some(Function::Count), "an alias wins over a function of the same name");
    }

    #[test]
    fn bare_dotted_names_are_alias_references_only_inside_their_scope() {
        let query = parse_query("select (select t.sz from /o), 't.sz', readme.md, t.sz, 1.5, size as sz from /d as t").unwrap();
        let inner = &query.fields[0].subquery.as_ref().unwrap().fields[0];
        assert_eq!(inner.root_alias.as_deref(), Some("t"), "bare t.sz inside the subquery references the outer alias");
        assert!(inner.field.is_none());
        assert!(query.fields[1].root_alias.is_none() && query.fields[1].val.as_deref() == Some("t.sz"), "quoted stays a literal");
        assert!(query.fields[2].root_alias.is_none() && query.fields[2].val.as_deref() == Some("readme.md"), "readme is no alias");
        assert_eq!(query.fields[3].root_alias.as_deref(), Some("t"));
        assert!(query.fields[4].root_alias.is_none() && query.fields[4].val.as_deref() == Some("1.5"));

        let unknown = parse_query("select name from /d where x.sz = 1").unwrap();
        let left = unknown.expr.unwrap().left.unwrap();
        assert!(left.root_alias.is_none() && left.val.as_deref() == Some("x.sz"), "x is not an alias anywhere, so x.sz is text");

        let nested = parse_query("select name, size as sz from /d as t where exists (select name, size as sz from /o as u where u.sz = t.sz)").unwrap();
        let inner_where = nested.expr.unwrap().right.unwrap().subquery.unwrap().expr.unwrap();
        assert_eq!(inner_where.left.unwrap().root_alias.as_deref(), Some("u"));
        assert_eq!(inner_where.right.unwrap().root_alias.as_deref(), Some("t"), "the enclosing alias is in scope");
    }

    #[test]
    fn select_aliases_resolve_inside_order_by_and_group_by_expressions() {
        let ordered = parse_query("select name, size as sz from /d order by sz + 0 desc, abs(sz)").unwrap();
        let first = &ordered.ordering_fields[0];
        assert_eq!(first.left.as_ref().unwrap().field, Some(Field::Size), "sz inside arithmetic is the aliased size");
        let second = &ordered.ordering_fields[1];
        assert_eq!(second.left.as_ref().unwrap().field, Some(Field::Size), "sz as a function argument is the aliased size");
        assert_eq!(ordered.ordering_asc, vec![false, true]);

        let grouped = parse_query("select length(name) as len, count(*) from /d group by len, len % 2").unwrap();
        assert_eq!(grouped.grouping_fields[0].function, Some(Function::Length), "bare alias is the aliased expression");
        assert_eq!(grouped.grouping_fields[1].left.as_ref().unwrap().function, Some(Function::Length), "alias inside arithmetic is the aliased expression");
    }

    #[test]
    fn order_by_accepts_comparisons_like_group_by() {
        let compact = parse_query("select name from /d order by size>2 desc, name").unwrap();
        assert_eq!(compact.ordering_fields.len(), 2);
        assert_eq!(compact.ordering_fields[0].op, Some(Op::Gt));
        assert_eq!(compact.ordering_fields[0].left.as_ref().unwrap().field, Some(Field::Size));
        assert_eq!(compact.ordering_asc, vec![false, true]);

        let spaced = parse_query("select name from /d order by modified > '2000-01-01' limit 1").unwrap();
        assert_eq!(spaced.ordering_fields[0].op, Some(Op::Gt));
        assert_eq!(spaced.ordering_fields[0].left.as_ref().unwrap().field, Some(Field::Modified));

        let arithmetic = parse_query("select name from /d order by (size>2)+0 desc, name").unwrap();
        let wrapped = &arithmetic.ordering_fields[0];
        assert!(wrapped.arithmetic_op.is_some(), "the comparison is the left operand of + 0");
        assert_eq!(wrapped.left.as_ref().unwrap().op, Some(Op::Gt));

        let logical = parse_query("select name from /d order by not size > 2, size > 2 and is_file desc").unwrap();
        assert_eq!(logical.ordering_fields.len(), 2);
        assert!(logical.ordering_fields[1].logical_op.is_some());
        assert_eq!(logical.ordering_asc, vec![true, false]);

        let aggregated = parse_query("select ext, count(*) from /d group by ext order by count(*) > 1 desc").unwrap();
        assert_eq!(aggregated.ordering_fields[0].op, Some(Op::Gt));
        assert_eq!(aggregated.ordering_fields[0].left.as_ref().unwrap().function, Some(Function::Count));
    }

    #[test]
    fn comparisons_may_start_with_a_select_alias_in_order_by_and_group_by() {
        let compact = parse_query("select name, size as s from /d order by s>1000 desc, name").unwrap();
        assert_eq!(compact.ordering_fields.len(), 2);
        assert_eq!(compact.ordering_fields[0].op, Some(Op::Gt));
        assert_eq!(compact.ordering_fields[0].left.as_ref().unwrap().field, Some(Field::Size), "s is the aliased size");
        assert_eq!(compact.ordering_asc, vec![false, true]);

        let spaced = parse_query("select name, size as s from /d order by s > 2 desc, name").unwrap();
        assert_eq!(spaced.ordering_fields[0].op, Some(Op::Gt));
        assert_eq!(spaced.ordering_fields[0].left.as_ref().unwrap().field, Some(Field::Size));

        let bare = parse_query("select name, size as s from /d order by s desc, name").unwrap();
        assert_eq!(bare.ordering_fields[0].field, Some(Field::Size), "a bare alias still resolves to its column");
        assert!(bare.ordering_fields[0].op.is_none());

        let grouped = parse_query("select size as s, count(*) from /d group by s, s>2 order by s > 2 desc, s").unwrap();
        assert_eq!(grouped.grouping_fields.len(), 2);
        assert_eq!(grouped.grouping_fields[0].field, Some(Field::Size));
        assert_eq!(grouped.grouping_fields[1].op, Some(Op::Gt));
        assert_eq!(grouped.grouping_fields[1].left.as_ref().unwrap().field, Some(Field::Size));
        assert_eq!(grouped.ordering_fields[0].op, Some(Op::Gt));
        assert_eq!(grouped.ordering_asc, vec![false, true]);
    }

    #[test]
    fn logical_conditions_may_start_with_a_select_alias_in_order_by() {
        let conjunction = parse_query("select name, is_file as f from /d order by f and size>1000 desc, name").unwrap();
        assert_eq!(conjunction.ordering_fields.len(), 2);
        let first = &conjunction.ordering_fields[0];
        assert!(first.logical_op.is_some(), "f and size>1000 is one logical sort key");
        let condition = first.left.as_ref().unwrap();
        assert_eq!(condition.op, Some(Op::Eq), "a bare boolean alias is wrapped as a condition");
        assert_eq!(condition.left.as_ref().unwrap().field, Some(Field::IsFile), "f is the aliased is_file");
        assert_eq!(conjunction.ordering_asc, vec![false, true]);

        let disjunction = parse_query("select name, is_file as f from /d order by f or size>2 desc, name").unwrap();
        assert!(disjunction.ordering_fields[0].logical_op.is_some());
        assert_eq!(disjunction.ordering_asc, vec![false, true]);

        let grouped = parse_query("select is_file as f, count(*) from /d group by f, f and size>2").unwrap();
        assert_eq!(grouped.grouping_fields.len(), 2);
        assert!(grouped.grouping_fields[1].logical_op.is_some());
    }

    #[test]
    fn an_alias_named_like_a_function_does_not_shadow_calls() {
        let counted = parse_query("select ext, count(*) as count from /d group by ext having count(*) > 1").unwrap();
        let having = counted.having.unwrap();
        assert_eq!(having.left.as_ref().unwrap().function, Some(Function::Count));
        assert!(having.left.as_ref().unwrap().alias.is_none(), "count(*) in HAVING is a fresh call, not the aliased column");

        let bare = parse_query("select ext, count(*) as count from /d group by ext having count > 1").unwrap();
        assert_eq!(bare.having.unwrap().left.unwrap().alias.as_deref(), Some("count"), "bare count is the alias");

        let max = parse_query("select ext, max(size) as max from /d group by ext having max(size) >= 0 order by max(size), ext").unwrap();
        assert_eq!(max.having.unwrap().left.unwrap().function, Some(Function::Max));
        assert_eq!(max.ordering_fields.len(), 2, "max(size) is one ordering expression, not the alias followed by (size)");
        assert_eq!(max.ordering_fields[0].function, Some(Function::Max));
        assert!(max.ordering_fields[0].alias.is_none());
        assert_eq!(max.ordering_fields[1].field, Some(Field::Extension));
    }

    #[test]
    fn correlated_column_alias_references_are_not_validated_as_literals() {
        for query in [
            "select name, size as sz from /d as t where exists (select name from /o where size > t.sz)",
            "select name, size as sz from /d as t where exists (select name from /o where t.sz < size)",
            "select name, modified as m from /d as t where exists (select name from /o where modified >= t.m)",
            "select name, size as sz from /d as t where exists (select name from /o where size in (t.sz, 5))",
        ] {
            assert!(parse_query(query).is_ok(), "{}: {:?}", query, parse_query(query).err());
        }
        for query in ["select name from /d where size > abc", "select name from /d where modified = 'not a date'"] {
            assert!(parse_query(query).is_err(), "{} must still be rejected", query);
        }
    }

    #[test]
    fn alias_references_are_normalised_and_checked_against_the_owning_select_list() {
        let upper = parse_query("select name, size as sz from /d as t where exists (select name from /o where size = T.sz)").unwrap();
        let inner = upper.expr.unwrap().right.unwrap().subquery.unwrap().expr.unwrap();
        let reference = inner.right.unwrap();
        assert_eq!(reference.root_alias.as_deref(), Some("t"), "the prefix takes the declared alias's spelling");
        assert_eq!(reference.val.as_deref(), Some("t.sz"));

        let file_name = parse_query("select name from /d as t where name = t.txt").unwrap();
        let literal = file_name.expr.unwrap().right.unwrap();
        assert!(literal.root_alias.is_none() && literal.val.as_deref() == Some("t.txt"), "txt is no column of t, so t.txt is text");

        let unknown = parse_query("select name, size as sz from /d as t where exists (select name from /o where t.nothing = 1)").unwrap();
        let inner = unknown.expr.unwrap().right.unwrap().subquery.unwrap().expr.unwrap();
        assert!(inner.left.unwrap().root_alias.is_none(), "an unknown column name reverts to text");

        let ordered = parse_query("select name, size as sz from /d as t order by t.sz desc").unwrap();
        assert_eq!(ordered.ordering_fields[0].field, Some(Field::Size), "an own-query reference in ORDER BY is the aliased expression");
    }

    #[test]
    fn non_boolean_literals_against_boolean_fields_are_rejected() {
        for query in [
            "select name from /d where is_dir = 2",
            "select name from /d where is_dir = 'maybe'",
            "select name from /d where is_dir = 0.5",
            "select name from /d where 2 = is_dir",
            "select name from /d where is_dir != 'maybe'",
            "select name from /d where is_file in (1, 'maybe')",
        ] {
            let err = parse_query(query).expect_err(query);
            assert!(err.contains("Can't parse boolean") && (err.contains("isdir") || err.contains("isfile")), "{}: {}", query, err);
        }
        for query in [
            "select name from /d where is_dir = true",
            "select name from /d where is_dir = 1 and is_file != 'yes'",
            "select name from /d where is_dir = yes or is_dir = off",
            "select name from /d where is_dir",
            "select name from /d where is_dir = is_file",
            "select name from /d where is_dir = contains('x')",
            "select name from /d where is_file = concat('tr', 'ue')",
            "select name from /d where is_file = replace('xtrue', 'x', '')",
            "select name from /d where is_file = substring('true', 1, 4)",
            "select name from /d where is_file in (concat('tr', 'ue'))",
            "select name from /d where is_file = coalesce(is_file, 'nope')",
        ] {
            assert!(parse_query(query).is_ok(), "{}: {:?}", query, parse_query(query).err());
        }
    }

    #[test]
    fn unknown_words_in_order_by_and_group_by_are_rejected() {
        for (query, clause, word) in [
            ("select name from /d order by size des", "ORDER BY", "des"),
            ("select name from /d order by size dsc", "ORDER BY", "dsc"),
            ("select name from /d order by bogus", "ORDER BY", "bogus"),
            ("select name from /d order by size, bogus desc", "ORDER BY", "bogus"),
            ("select name, count(*) from /d group by bogus", "GROUP BY", "bogus"),
        ] {
            let err = parse_query(query).expect_err(query);
            assert!(err.contains(clause) && err.contains(word), "{}: {}", query, err);
        }
        for query in [
            "select name from /d order by size desc",
            "select name, size from /d order by 2 desc, 1",
            "select name from /d order by 'size' desc",
            "select name from /d order by size + 0, length(name)",
            "select name, size as sz from /d order by sz desc",
            "select ext, count(*) from /d group by ext",
            "select length(name) as len, count(*) from /d group by len, len % 2",
        ] {
            assert!(parse_query(query).is_ok(), "{}: {:?}", query, parse_query(query).err());
        }
    }

    #[test]
    fn select_columns_outside_the_grouping_are_rejected() {
        for (query, column) in [
            ("select length(name), count(*) from /d group by length(name) % 2", "name"),
            ("select name, count(*) from /d group by ext", "name"),
            ("select ext, size, count(*) from /d group by ext", "size"),
            ("select ext, count(*) from /d group by lower(ext)", "extension"),
            ("select ext, size * 2 as dbl, count(*) from /d group by ext", "size"),
            ("select ext, upper(name) from /d group by ext", "name"),
            ("select ext, max(size), length(name) from /d group by ext", "name"),
            ("select name, count(*) from /d", "name"),
            ("select min(size), path from /d", "path"),
            ("select name from /d having count(*) > 0", "name"),
            ("select contains('x') from /d having count(*) > 0", "contains()"),
            ("select t.name, count(*) from /d as t group by t.ext", "t.name"),
        ] {
            let err = parse_query(query).expect_err(query);
            assert!(
                err.contains(&format!("Column '{column}' must appear in GROUP BY")),
                "{}: {}", query, err
            );
        }
        for query in [
            "select ext, count(*) from /d group by ext",
            "select ext, count(*) from /d group by 1",
            "select ext as e, count(*) from /d group by e",
            "select upper(ext), count(*) from /d group by ext",
            "select length(name) % 2, count(*) from /d group by length(name) % 2",
            "select length(name) as len, count(*) from /d group by len, len % 2",
            "select ext, 'label', 1 + 2, count(*) from /d group by ext",
            "select ext, max(size) + min(size), abs(sum(size * 2) - 1) from /d group by ext",
            "select ext, count(*) from /d group by ext, size",
            "select t.ext, count(*) from /d as t group by t.ext",
            "select ext, (select count(*) from /e where ext = t.ext) from /d as t group by ext",
            "select name from /d group by name order by max(size) desc",
            "select count(*) from /d",
            "select name, size, length(name) from /d",
            "select name, (select count(*) + t.size from /o) from /d as t",
            "select name, (select count(*) + t.size from /o group by ext) from /d as t",
            "select name, (select max(size) - t.size from /o where size > t.size) from /d as t",
        ] {
            assert!(parse_query(query).is_ok(), "{}: {:?}", query, parse_query(query).err());
        }
        let err = parse_query("select name, (select o.name, count(*) from /o as o) from /d as t").expect_err("own alias");
        assert!(err.contains("Column 'o.name' must appear in GROUP BY"), "{err}");
    }

    #[test]
    fn group_by_accepts_arithmetic_without_parentheses() {
        let negated = parse_query("select -size, count(*) from /d group by -size").unwrap();
        assert_eq!(negated.grouping_fields.len(), 1);
        assert_eq!(negated.grouping_fields[0].field, Some(Field::Size));
        assert!(negated.grouping_fields[0].minus);

        let sum = parse_query("select 1 + size, count(*) from /d group by 1 + size").unwrap();
        assert_eq!(sum.grouping_fields.len(), 1);
        assert_eq!(sum.grouping_fields[0].arithmetic_op, Some(ArithmeticOp::Add));
        assert_eq!(sum.grouping_fields, parse_query("select 1 + size, count(*) from /d group by (1 + size)").unwrap().grouping_fields);

        let two_keys = parse_query("select ext, 2 * size, count(*) from /d group by ext, 2 * size").unwrap();
        assert_eq!(two_keys.grouping_fields.len(), 2);
        assert_eq!(two_keys.grouping_fields[1].arithmetic_op, Some(ArithmeticOp::Multiply));

        let positional = parse_query("select ext, size, count(*) from /d group by 1, 2").unwrap();
        assert_eq!(positional.grouping_fields, vec![Expr::field(Field::Extension), Expr::field(Field::Size)]);
    }

    #[test]
    fn a_field_qualified_with_the_own_root_alias_is_the_same_grouping_column() {
        for query in [
            "select ext, count(*) from /d as t group by t.ext",
            "select t.ext, count(*) from /d as t group by ext",
            "select upper(t.ext), count(*) from /d as t group by ext",
            "select ext, count(*) from /d as t group by t.ext order by ext",
            "select t.ext, count(*) from /d as t group by t.ext having ext = 'txt' order by t.ext",
        ] {
            assert!(parse_query(query).is_ok(), "{}: {:?}", query, parse_query(query).err());
        }
        let err = parse_query("select name, (select ext, count(*) + t.size from /o group by t.ext) from /d as t").expect_err("outer alias");
        assert!(err.contains("Column 'extension' must appear in GROUP BY"), "{err}");
    }

    #[test]
    fn an_unterminated_quote_is_reported_wherever_it_occurs() {
        for query in [
            "select 'abc from /d",
            "select name, 'abc from /d",
            "select upper('abc) from /d",
            "select name from '/d",
            "select name from /d where name = 'abc",
            "select name from /d group by 'x",
            "select name from /d order by 'x",
            "select name from /d limit 1 'x",
        ] {
            let err = parse_query(query).expect_err(query);
            assert!(err.starts_with("Unterminated quoted string"), "{}: {}", query, err);
        }
        assert!(parse_query("select name from /d where name = 'it''s'").is_ok());
    }

    #[test]
    fn an_unknown_format_size_modifier_literal_is_rejected_at_parse_time() {
        for query in [
            "select format_size(size, 'short') from /d",
            "select name from /d where format_size(size, 'xyz') = ''",
            "select name from /d order by format_size(size, '%.1f')",
        ] {
            let err = parse_query(query).expect_err(query);
            assert!(err.contains("Invalid file size format modifier"), "{}: {}", query, err);
        }
        for query in [
            "select format_size(size) from /d",
            "select format_size(size, '%.1 mb') from /d",
            "select format_size(size, 'kbs') from /d",
            "select format_size(size, ' k') from /d",
            "select format_size(size, ext) from /d",
        ] {
            assert!(parse_query(query).is_ok(), "{}: {:?}", query, parse_query(query).err());
        }
    }

    #[test]
    fn is_null_and_is_not_null_parse_as_operators() {
        let is_null = parse_query("select name from /d where width is null").unwrap();
        let expr = is_null.expr.unwrap();
        assert_eq!(expr.op, Some(Op::IsNull));
        assert_eq!(expr.left.unwrap().field, Some(Field::Width));

        let is_not_null = parse_query("select name from /d where width is not null").unwrap();
        assert_eq!(is_not_null.expr.unwrap().op, Some(Op::IsNotNull));

        let negated = parse_query("select name from /d where not width is null").unwrap();
        assert_eq!(negated.expr.unwrap().op, Some(Op::IsNotNull));

        let combined = parse_query("select name from /d where ext is null and size > 0 or upper(name) is not null").unwrap();
        assert!(combined.expr.unwrap().logical_op.is_some());

        let err = parse_query("select name from /d where width is nul").expect_err("IS needs NULL");
        assert!(err.contains("NULL"), "{}", err);
    }

    #[test]
    fn having_without_aggregation_is_rejected() {
        let query = "select name from /test having name = foo";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let err = p.parse(false).unwrap_err();
        assert!(
            err.contains("HAVING requires GROUP BY or an aggregate function"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn having_with_aggregate_but_no_group_by_is_accepted() {
        let query = "select count(*) from /test having count(*) > 5";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(query.having.is_some());
        assert!(query.is_aggregated());
    }

    #[test]
    fn having_with_non_grouping_field_is_rejected() {
        let query = "select ext, count(*) from /test group by ext having size > 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let err = p.parse(false).unwrap_err();
        assert!(
            err.contains("not a grouping column"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn having_with_grouping_field_is_accepted() {
        let query = "select ext, count(*) from /test group by ext having ext = rs";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(query.having.is_some());
    }

    #[test]
    fn track_and_disc_accept_slash_formats() {
        assert!(parse_query("select name from /d where track = '4/9'").is_ok());
        assert!(parse_query("select name from /d where disc in ('1/2', '2/2')").is_ok());
        assert!(parse_query("select name from /d where track like '4/%'").is_ok());
    }

    #[test]
    fn file_reading_functions_need_grouping_or_an_aggregate() {
        let selected = parse_query("select contains('x'), count(*) from /d").unwrap_err();
        assert!(selected.contains("contains()"), "unexpected error: {selected}");
        let grouped = parse_query("select ext, count(*) from /d group by ext having contains('x')").unwrap_err();
        assert!(grouped.contains("contains()") && grouped.contains("HAVING"), "unexpected error: {grouped}");
        let has_xattr = parse_query("select ext, has_xattr('a'), count(*) from /d group by ext").unwrap_err();
        assert!(has_xattr.contains("has_xattr()"), "unexpected error: {has_xattr}");

        assert!(parse_query("select count(*) from /d where contains('x')").is_ok(), "WHERE is per file");
        assert!(parse_query("select ext, count(*) from /d group by ext having max(size) > 1").is_ok());
        assert!(parse_query("select ext, count(*) from /d group by ext, contains('x') having contains('x')").is_ok(), "a grouping key is allowed");
    }

    #[test]
    fn having_with_exists_is_rejected() {
        let query = "select ext, count(*) from /test group by ext having exists (select name from /test)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let err = p.parse(false).unwrap_err();
        assert!(
            err.contains("not supported in the HAVING clause"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn multicolumn_in_subquery_is_rejected() {
        let query = "select name from /test where name in (select name, size from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let err = p.parse(false).unwrap_err();
        assert!(
            err.contains("single column"),
            "expected single-column error, got: {}",
            err
        );
    }

    #[test]
    fn multicolumn_not_in_subquery_is_rejected() {
        let query = "select name from /test where name not in (select name, size from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        assert!(p.parse(false).is_err());
    }

    #[test]
    fn single_column_in_subquery_is_accepted() {
        let query = "select name from /test where name in (select name from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());
        assert!(query.expr.is_some());
    }

    #[test]
    fn multicolumn_exists_subquery_is_accepted() {
        // SQL places no column-count restriction on EXISTS.
        let query = "select name from /test where exists (select name, size from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(query.expr.is_some());
    }

    #[test]
    fn multicolumn_scalar_subquery_is_rejected() {
        let query = "select name from /test where size gt (select size, name from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let err = p.parse(false).unwrap_err();
        assert!(
            err.contains("single column"),
            "expected single-column error, got: {}",
            err
        );
    }

    #[test]
    fn single_column_scalar_subquery_is_accepted() {
        let query = "select name from /test where size gt (select size from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(query.expr.is_some());
    }

    #[test]
    fn implicit_limit_applies_to_constant_select() {
        let query = "select 123 from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert_eq!(query.limit, 1, "field-less constant SELECT collapses to one row");
    }

    #[test]
    fn no_implicit_limit_when_a_correlated_subquery_reads_the_outer_row() {
        for query in [
            "select (select t1.size from /other) from /test as t1",
            "select (select name from /other where size > t1.size) from /test as t1",
            "select upper((select t1.name from /other)) from /test as t1",
        ] {
            let parsed = parse_query(query).unwrap();
            assert_eq!(parsed.limit, 0, "{}: a correlated SELECT differs per outer row", query);
        }
        for query in [
            "select 1 from /test",
            "select (select count(*) from /other) from /test",
            "select (select name from /other where size > 1) from /test as t1",
        ] {
            let parsed = parse_query(query).unwrap();
            assert_eq!(parsed.limit, 1, "{}: a constant SELECT still collapses", query);
        }
    }

    #[test]
    fn no_implicit_limit_for_functions_that_read_the_file() {
        for query in [
            "select contains('x') from /test",
            "select upper(contains('x')) from /test",
            "select 1, contains('x') from /test",
        ] {
            let parsed = parse_query(query).unwrap();
            assert_eq!(parsed.limit, 0, "{}: the value differs per file", query);
        }
        for query in ["select upper('a') from /test", "select 1 + 2, length('abc') from /test"] {
            let parsed = parse_query(query).unwrap();
            assert_eq!(parsed.limit, 1, "{}: a constant SELECT still collapses", query);
        }
    }

    #[test]
    fn no_implicit_limit_for_grouped_aggregate_query() {
        // `count(*)` requires no file fields, but grouping produces one row
        // per group — the implicit LIMIT 1 must not truncate the result.
        let query = "select count(*) from /test group by ext";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert_eq!(query.limit, 0);
    }

    #[test]
    fn no_implicit_limit_for_plain_aggregate_query() {
        let query = "select count(*) from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert_eq!(query.limit, 0);
    }

    #[test]
    fn query_with_between() {
        let query = "select name, size from /test where size between 5mb and 6mb";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name, size from /test where size gte 5mb and size lte 6mb";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_dfs() {
        let query = "select mime from /test dfs group by mime";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Dfs, false, None)
            ),]
        );
    }

    #[test]
    fn reordered_expr_branches_with_different_weights() {
        let query = "select name from /test where CONTAINS('foobar') or name like 'foobar'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name from /test where name like 'foobar' or CONTAINS('foobar')";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn query_with_value_in_string_args() {
        let query = "select name from /test where name in ('foo', 'bar')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("foo")),
            Expr::value(String::from("bar")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::In,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));

        let query = "select name from /test where name not in (foo, bar)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("foo")),
            Expr::value(String::from("bar")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Name),
            Op::NotIn,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_value_in_int_args() {
        let query = "select name from /test where size in (100, 200)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("100")),
            Expr::value(String::from("200")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::In,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_value_in_float_args() {
        let query = "select name from /test where size in (100.0, 200.0)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![
            Expr::value(String::from("100.0")),
            Expr::value(String::from("200.0")),
        ]);

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::In,
            list_expr,
        );

        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn simple_subquery() {
        let query = "select name from /test where size > 100 and size in (select size from /test2 where size > 50)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        assert!(query.expr.unwrap().left.unwrap().right.is_some());
    }

    #[test]
    fn complex_subquery() {
        let query = "select name from /test1 where size > 100 and size in (select size from /test2 where name in (select name from /test3 where modified in (select modified from /test4 where size < 200)))";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test1"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None)
            ),]
        );

        // Verify the main query expression exists
        assert!(query.expr.is_some());

        // Function to recursively find subqueries in an expression
        fn find_subqueries(expr: &Expr, count: &mut usize) {
            // Check if this expression has a subquery
            if expr.subquery.is_some() {
                *count += 1;
                // Check if the subquery has an expression
                if let Some(subquery) = &expr.subquery
                    && let Some(subquery_expr) = &subquery.expr {
                        // Continue searching in the subquery's expression
                        find_subqueries(subquery_expr, count);
                    }
            }

            // Check the left branch
            if let Some(left) = &expr.left {
                find_subqueries(left, count);
            }

            // Check the right branch
            if let Some(right) = &expr.right {
                find_subqueries(right, count);
            }
        }

        // Count the number of subqueries
        let mut subquery_count = 0;
        find_subqueries(&query.expr.unwrap(), &mut subquery_count);

        // We expect 3 levels of nested subqueries
        assert_eq!(subquery_count, 3);
    }

    #[test]
    fn root_with_alias() {
        let query = "select name from /test as test_alias";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, Some(String::from("test_alias")))
            ),]
        );
    }

    #[test]
    fn root_and_field_with_alias() {
        let query = "select test_alias.name from /test as test_alias";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![Expr::field_with_root_alias(Field::Name, Some(String::from("test_alias")))]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, Some(String::from("test_alias")))
            ),]
        );
    }

    #[test]
    fn broken_root_path() {
        let query = "select name from ,";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.fields,
            vec![Expr::field(Field::Name)]
        );

        assert_eq!(
            query.roots,
            vec![Root::new(String::from("."), RootOptions::new())]
        );
    }

    #[test]
    fn parse_root_options_fails_on_incomplete_option() {
        // "mindepth" requires a number, omitting it must produce an error
        let query = "select name from /test mindepth";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn query_with_limit_and_offset() {
        let query = "select name from /test limit 3,5";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.limit, 5);
        assert_eq!(query.offset, 3);
    }

    #[test]
    fn group_by_quoted_field() {
        let query = "select count(*), mime from /test group by 'mime'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.grouping_fields, vec![Expr::field(Field::Mime)]);
    }

    #[test]
    fn order_by_quoted_field() {
        let query = "select name, size from /test order by 'size'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields, vec![Expr::field(Field::Size)]);
    }

    #[test]
    fn group_by_quoted_same_as_unquoted() {
        let query1 = "select count(*), mime from /test group by mime";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();

        let query2 = "select count(*), mime from /test group by 'mime'";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();

        assert_eq!(q1.grouping_fields, q2.grouping_fields);
    }

    #[test]
    fn order_by_quoted_same_as_unquoted() {
        let query1 = "select name, size from /test order by size";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();

        let query2 = "select name, size from /test order by 'size'";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();

        assert_eq!(q1.ordering_fields, q2.ordering_fields);
    }

    #[test]
    fn quoted_string_in_where_is_literal_not_field() {
        // A file literally named "size" must be findable: the quoted string is
        // a literal, not a reference to the size field.
        let query = "select name from /test where name = 'size'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        let expr = query.expr.unwrap();
        let right = expr.right.unwrap();
        assert_eq!(right.val, Some(String::from("size")));
        assert_eq!(right.field, None);
    }

    #[test]
    fn quoted_string_in_select_list_is_literal_not_field() {
        let query = "select name, 'size' from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(query.fields[1].val, Some(String::from("size")));
        assert_eq!(query.fields[1].field, None);
    }

    #[test]
    fn order_by_alias_of_computed_column_resolves_to_expression() {
        let query = "select size + 1 as s from /test order by s desc";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields, query.fields);
        assert_eq!(query.ordering_asc, vec![false]);
    }

    #[test]
    fn group_by_alias_of_computed_column_resolves_to_expression() {
        let query = "select size + 1 as s, count(*) from /test group by s";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.grouping_fields, vec![query.fields[0].clone()]);
    }

    #[test]
    fn order_by_alias_of_plain_field_resolves_to_expression() {
        let query = "select ext as e from /test order by e";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields, query.fields);
    }

    #[test]
    fn trailing_comma_in_select_list_does_not_swallow_from() {
        // A trailing comma must not demote FROM to a value column, or the
        // query would silently search the current directory instead.
        let query = "select name, from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(query.fields.len(), 1);
        assert_eq!(query.roots.len(), 1);
        assert_eq!(query.roots[0].path, "/test");
    }

    #[test]
    fn unary_minus_on_parenthesized_expression_parses() {
        let query = "select -(1+2) from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.fields.len(), 1);
        assert!(query.fields[0].minus);
        assert!(query.fields[0].arithmetic_op.is_some());
    }

    #[test]
    fn unary_minus_on_parenthesized_expression_in_where_parses() {
        let query = "select name from /test where size gt -(1+2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        let right = expr.right.unwrap();
        assert!(right.minus);
    }

    #[test]
    fn double_unary_minus_cancels() {
        let query = "select -(-1) from /test";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert!(!query.fields[0].minus);
    }

    #[test]
    fn order_by_non_boolean_function_without_parens_should_not_panic() {
        let query = "select name, size from /test order by upper desc";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err() || !result.unwrap().ordering_fields.is_empty());
    }

    #[test]
    fn group_by_non_boolean_function_without_parens_should_not_panic() {
        let query = "select mime from /test group by upper, mime";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err() || !result.unwrap().grouping_fields.is_empty());
    }

    #[test]
    fn query_with_exists_operator() {
        let query = "select t.name from /test as t where exists('user.test')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![Expr::value(String::from("user.test"))]);

        let expr = Expr::op(Expr::field(Field::Name), Op::Exists, list_expr);
        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn query_with_notexists_operator_and_no_left_operand() {
        // The no-LHS recovery must accept `notexists` just like `exists`.
        let query = "select name from /test where notexists (select name from /test2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::NotExists));
    }

    #[test]
    fn query_with_not_exists_operator() {
        let query = "select t.name from /test as t where not exists('user.test')";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let mut list_expr = Expr::new();
        list_expr.set_args(vec![Expr::value(String::from("user.test"))]);

        let expr = Expr::op(Expr::field(Field::Name), Op::NotExists, list_expr);
        assert_eq!(query.expr, Some(expr));
    }

    fn count_subqueries(expr: &Expr) -> usize {
        let mut count = 0;
        fn walk(e: &Expr, c: &mut usize) {
            if e.subquery.is_some() {
                *c += 1;
                if let Some(subq) = &e.subquery
                    && let Some(subexpr) = &subq.expr {
                        walk(subexpr, c);
                    }
            }
            if let Some(left) = &e.left { walk(left, c); }
            if let Some(right) = &e.right { walk(right, c); }
        }
        walk(expr, &mut count);
        count
    }

    #[test]
    fn exists_with_simple_subquery() {
        let query = "select t1.name from /test as t1 where exists(select t2.name from /t2 as t2 where t2.size > t1.size and t2.name = t1.name)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.expect("query.expr should be present");
        assert_eq!(expr.op, Some(Op::Exists));
        let right = expr.right.as_ref().expect("EXISTS should have right side");
        assert!(right.subquery.is_some(), "Right side should be a subquery");

        let total = count_subqueries(&expr);
        assert_eq!(total, 1);
    }

    #[test]
    fn not_exists_with_simple_subquery() {
        let query = "select t1.name from /test as t1 where not exists(select t2.name from /t2 as t2 where t2.size > 0)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.expect("query.expr should be present");
        assert_eq!(expr.op, Some(Op::NotExists));
        let right = expr.right.as_ref().expect("NOT EXISTS should have right side");
        assert!(right.subquery.is_some(), "Right side should be a subquery");

        let total = count_subqueries(&expr);
        assert_eq!(total, 1);
    }

    #[test]
    fn exists_with_nested_subqueries() {
        let query = "select t1.name from /t1 as t1 where exists (select t2.name from /t2 as t2 where t2.name in (select t3.name from /t3 as t3 where t3.size > 0))";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.expect("expr should be present");
        assert_eq!(expr.op, Some(Op::Exists));
        let right = expr.right.as_ref().expect("EXISTS should have right side");
        assert!(right.subquery.is_some(), "Right side should be a subquery");

        let total = count_subqueries(&expr);
        assert_eq!(total, 2);
    }

    #[test]
    fn not_exists_with_double_nested_subqueries() {
        let query = "select t1.name from /t1 as t1 where not exists (select t2.name from /t2 as t2 where t2.name in (select t3.name from /t3 as t3 where t3.modified in (select t4.modified from /t4 as t4 where t4.size < 200)))";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.expect("expr should be present");
        assert_eq!(expr.op, Some(Op::NotExists));
        let right = expr.right.as_ref().expect("NOT EXISTS should have right side");
        assert!(right.subquery.is_some(), "Right side should be a subquery");

        let total = count_subqueries(&expr);
        assert_eq!(total, 3);
    }

    #[test]
    fn exists_with_binding_subqueries() {
        let query = "select t1.name from /t1 as t1 where exists(select t2.name from /t2 as t2 where t2.name = t1.name and t2.size > t1.size and t2.name in (select t3.name from /t3 as t3 where t3.name = t2.name and t3.size > t2.size))";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        fn contains_field_with_alias(expr: &Expr, field: Field, alias: &str) -> bool {
            let mut found = false;
            fn walk(e: &Expr, field: Field, alias: &str, found: &mut bool) {
                if *found { return; }
                if let Some(f) = &e.field
                    && *f == field
                        && let Some(a) = &e.root_alias
                            && a == alias { *found = true; }
                if let Some(left) = &e.left { walk(left, field, alias, found); }
                if let Some(right) = &e.right { walk(right, field, alias, found); }
                if let Some(args) = &e.args {
                    for a in args { walk(a, field, alias, found); }
                }
                if let Some(subq) = &e.subquery
                    && let Some(subexpr) = &subq.expr {
                        walk(subexpr, field, alias, found);
                    }
            }
            walk(expr, field, alias, &mut found);
            found
        }

        let expr = query.expr.expect("expr should be present");
        assert_eq!(expr.op, Some(Op::Exists));
        let right = expr.right.as_ref().expect("EXISTS should have right side");
        assert!(right.subquery.is_some(), "Right side should be a subquery");

        let sub1 = right.subquery.as_ref().unwrap();
        let sub1_expr = sub1.expr.as_ref().expect("sub1 expr should exist");

        assert!(contains_field_with_alias(sub1_expr, Field::Name, "t2"));
        assert!(contains_field_with_alias(sub1_expr, Field::Name, "t1"));
        assert!(contains_field_with_alias(sub1_expr, Field::Size, "t2"));
        assert!(contains_field_with_alias(sub1_expr, Field::Size, "t1"));

        fn find_first_subquery(e: &Expr) -> Option<&Expr> {
            if e.subquery.is_some() { return Some(e); }
            if let Some(left) = &e.left && let Some(f) = find_first_subquery(left) { return Some(f); }
            if let Some(right) = &e.right && let Some(f) = find_first_subquery(right) { return Some(f); }
            if let Some(args) = &e.args { for a in args { if let Some(f) = find_first_subquery(a) { return Some(f); } } }
            None
        }
        let subnode = find_first_subquery(sub1_expr).expect("second-level subquery node should be present");
        let sub2 = subnode.subquery.as_ref().unwrap();
        let sub2_expr = sub2.expr.as_ref().expect("sub2 expr should exist");

        assert!(contains_field_with_alias(sub2_expr, Field::Name, "t3"));
        assert!(contains_field_with_alias(sub2_expr, Field::Name, "t2"));
        assert!(contains_field_with_alias(sub2_expr, Field::Size, "t3"));
        assert!(contains_field_with_alias(sub2_expr, Field::Size, "t2"));

        let total = count_subqueries(&expr);
        assert_eq!(total, 2);
    }

    #[test]
    fn not_gt_should_produce_lte() {
        let query = "select name from /test where not size > 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::Lte,
            Expr::value(String::from("100")),
        );
        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn not_lt_should_produce_gte() {
        let query = "select name from /test where not size < 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::Gte,
            Expr::value(String::from("100")),
        );
        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn not_gte_should_produce_lt() {
        let query = "select name from /test where not size gte 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::Lt,
            Expr::value(String::from("100")),
        );
        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn not_lte_should_produce_gt() {
        let query = "select name from /test where not size lte 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Size),
            Op::Gt,
            Expr::value(String::from("100")),
        );
        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn order_by_zero_index_should_not_panic() {
        let query = "select name, size from /test order by 0";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn order_by_out_of_bounds_index_should_not_panic() {
        let query = "select name from /test order by 5";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn order_by_expression_with_leading_minus() {
        let query = "select name, size from /test order by -size, name desc";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields.len(), 2);
        assert_eq!(query.ordering_asc, vec![true, false]);
        let negated = &query.ordering_fields[0];
        assert!(negated.minus);
        assert_eq!(negated.field, Some(Field::Size));
        assert_eq!(query.ordering_fields[1].field, Some(Field::Name));
    }

    #[test]
    fn order_by_expression_starting_with_a_number_is_not_a_positional_index() {
        let query = "select name, size as sz from /test order by 0 - sz desc, 2 * size";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields.len(), 2);
        assert_eq!(query.ordering_asc, vec![false, true]);
        let zero_minus_alias = &query.ordering_fields[0];
        assert_eq!(zero_minus_alias.arithmetic_op, Some(ArithmeticOp::Subtract));
        assert_eq!(zero_minus_alias.left.as_ref().unwrap().val.as_deref(), Some("0"));
        assert_eq!(zero_minus_alias.right.as_ref().unwrap().field, Some(Field::Size));
        let twice_size = &query.ordering_fields[1];
        assert_eq!(twice_size.arithmetic_op, Some(ArithmeticOp::Multiply));
        assert_eq!(twice_size.right.as_ref().unwrap().field, Some(Field::Size));
    }

    #[test]
    fn order_by_bare_number_is_still_a_positional_index() {
        let query = "select name, size from /test order by 2 desc, 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields.len(), 2);
        assert_eq!(query.ordering_asc, vec![false, true]);
        assert_eq!(query.ordering_fields[0].field, Some(Field::Size));
        assert_eq!(query.ordering_fields[1].field, Some(Field::Name));
    }

    #[test]
    fn not_between_produces_correct_operators() {
        let query = "select name from /test where size not between 5 and 10";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name from /test where size < 5 or size > 10";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn not_parenthesized_and_applies_de_morgan() {
        let query = "select name from /test where not (size > 100 and name = foo)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.logical_op, Some(LogicalOp::Or));
        let left = expr.left.unwrap();
        assert_eq!(left.op, Some(Op::Ne));
        let right = expr.right.unwrap();
        assert_eq!(right.op, Some(Op::Lte));
    }

    #[test]
    fn not_parenthesized_or_applies_de_morgan() {
        let query = "select name from /test where not (size > 100 or name = foo)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.logical_op, Some(LogicalOp::And));
        let left = expr.left.unwrap();
        assert_eq!(left.op, Some(Op::Ne));
        let right = expr.right.unwrap();
        assert_eq!(right.op, Some(Op::Lte));
    }

    #[test]
    fn order_by_desc_without_field_should_not_panic() {
        let query = "select name from /test where size > 0 order by desc";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err() || result.unwrap().ordering_fields.is_empty());
    }

    #[test]
    fn boolean_function_without_parens_does_not_eat_next_token() {
        // The bare name is a string literal, not a call, so it is rejected as
        // an operator-less condition instead of swallowing the next tokens.
        let err = parse_query("select name from /test where CONTAINS and size > 100").unwrap_err();
        assert_eq!(err, "Expected a comparison or boolean condition in WHERE, found 'contains'");

        let query = parse_query("select name from /test where contains('x') and size > 100").unwrap();
        let expr = query.expr.unwrap();
        assert_eq!(expr.logical_op, Some(LogicalOp::And));
        assert!(expr.right.is_some());
    }

    #[test]
    fn unary_plus_in_field_expression() {
        let query = "select (+size) from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select size from /test limit 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.fields, query2.fields);
    }

    #[test]
    fn rx_root_option_is_a_synonym_of_regexp() {
        for query in [
            "select name from /te.*/sub rx",
            "select name from /te.*/sub RX",
            "select name from /te.*/sub rx where name = 'x'",
            "select name from /te.*/sub depth 2 rx symlinks",
            "select name from /te.*/sub rx, /other regexp",
        ] {
            let mut lexer = Lexer::new(vec![query.to_string()]);
            let mut p = Parser::new(&mut lexer);
            let parsed = p.parse(false).unwrap_or_else(|e| panic!("{}: {}", query, e));
            assert!(!p.there_are_remaining_lexemes(), "{}: leftover lexemes", query);
            assert_eq!(parsed.roots[0].path, "/te.*/sub", "{}", query);
            assert!(parsed.roots[0].options.regexp, "{}: rx should enable regexp roots", query);
        }
        let mut lexer = Lexer::new(vec!["select name from /te.*/sub depth 2 rx symlinks".to_string()]);
        let parsed = Parser::new(&mut lexer).parse(false).unwrap();
        assert_eq!(parsed.roots[0].options.max_depth, 2);
        assert!(parsed.roots[0].options.symlinks);
    }

    #[test]
    fn rx_root_option_does_not_shadow_a_field_named_rx() {
        // `rx` is only a root option; a SELECT list ending right before FROM
        // and a WHERE using the rx operator keep working.
        parse_where("select name from /test rx where name rx '^a'");
    }

    #[test]
    fn root_options_preserved_when_followed_by_unknown_string() {
        let query = "select name from /test depth 2";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();

        assert_eq!(
            query.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 2, false, false, None, None, None, Bfs, false, None)
            )]
        );

        let query2 = "select name from /test depth 2 /other";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();

        assert_eq!(query2.roots[0].options.max_depth, 2);
    }

    #[test]
    fn invalid_operator_should_return_error() {
        let query = "select name from /test where name => foo";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn notlike_word_operator() {
        let query = "select name from /test where name notlike '%.tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::NotLike));
    }

    #[test]
    fn notrx_word_operator() {
        let query = "select name from /test where name notrx '.*tmp'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::NotRx));
    }

    #[test]
    fn eeq_word_operator() {
        let query = "select name from /test where name eeq 'test.txt'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::Eeq));
    }

    #[test]
    fn ene_word_operator() {
        let query = "select name from /test where name ene 'test.txt'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::Ene));
    }

    #[test]
    fn quoted_text_column_starting_with_root_option_prefix() {
        let query = "select 'gitignore_status', name from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_ok());
        let query = result.unwrap();
        assert_eq!(query.fields.len(), 2);
    }

    #[test]
    fn non_boolean_function_without_parens_in_select_errors() {
        let query = "select UPPER, name from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err() || result.as_ref().unwrap().fields.len() == 2);
    }

    #[test]
    fn quoted_function_name_as_value_in_where() {
        let query1 = "select name from /test where ext = 'upper' and size > 0";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let result1 = p1.parse(false);
        assert!(result1.is_ok());

        let query2 = "select name from /test where ext = 'lower' and size > 0";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let result2 = p2.parse(false);
        assert!(result2.is_ok());
    }

    #[test]
    fn quoted_function_name_as_value_produces_value_expr() {
        let query = "select name from /test where ext = 'upper'";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = Expr::op(
            Expr::field(Field::Extension),
            Op::Eq,
            Expr::value(String::from("upper")),
        );
        assert_eq!(query.expr, Some(expr));
    }

    #[test]
    fn quoted_function_name_in_function_arg() {
        let query = "select CONCAT(name, 'count') from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_ok());
        let query = result.unwrap();
        assert_eq!(query.fields.len(), 1);
    }

    #[test]
    fn and_reorders_by_weight_like_or() {
        let query1 = "select name from /test where CONTAINS('foobar') and name like 'foobar'";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();

        let query2 = "select name from /test where name like 'foobar' and CONTAINS('foobar')";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();

        assert_eq!(q1.expr, q2.expr);
    }

    #[test]
    fn depth_without_value_followed_by_keyword_should_error() {
        let query = "select name from /test depth where size > 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn mindepth_without_value_followed_by_keyword_should_error() {
        let query = "select name from /test mindepth where size > 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn word_arithmetic_operator_does_not_double_drop() {
        // `mul` after a comparison operator is a value, not the arithmetic
        // keyword (a string field: against `size` the word is now rejected
        // at parse time as a non-numeric literal).
        let query = "select name from /test where name > mul";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_ok());
        let query = result.unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::Gt));
        let right = expr.right.unwrap();
        assert_eq!(right.val, Some(String::from("mul")));
    }

    #[test]
    fn group_by_parenthesized_expression() {
        let query = "select count(*), (size) from /test group by (size)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.grouping_fields, vec![Expr::field(Field::Size)]);
    }

    #[test]
    fn group_by_parenthesized_arithmetic() {
        let query1 = "select count(*), size + 1 from /test group by size + 1";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();
        assert!(!p1.there_are_remaining_lexemes());

        let query2 = "select count(*), size + 1 from /test group by (size + 1)";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(q1.grouping_fields, q2.grouping_fields);
    }

    #[test]
    fn order_by_parenthesized_expression() {
        let query1 = "select name, size from /test order by size";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();
        assert!(!p1.there_are_remaining_lexemes());

        let query2 = "select name, size from /test order by (size)";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(q1.ordering_fields, q2.ordering_fields);
    }

    #[test]
    fn order_by_parenthesized_arithmetic_desc() {
        let query = "select name, size from /test order by (size + 1) desc";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.ordering_fields.len(), 1);
        assert_eq!(query.ordering_asc, vec![false]);
        let expr = &query.ordering_fields[0];
        assert!(expr.arithmetic_op.is_some());
    }

    #[test]
    fn order_by_curly_braces_expression() {
        let query1 = "select name, size from /test order by (size)";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();
        assert!(!p1.there_are_remaining_lexemes());

        let query2 = "select name, size from /test order by {size}";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(q1.ordering_fields, q2.ordering_fields);
    }

    #[test]
    fn unquoted_function_name_parsed_as_function() {
        // A bare zero-argument function name is a call; the name of a function
        // that needs arguments is an ordinary string literal without parentheses.
        let query = parse_query("select name from /test where name = curdate").unwrap();
        let expr = query.expr.unwrap();
        assert_eq!(expr.op, Some(Op::Eq));
        assert_eq!(expr.right.unwrap().function, Some(Function::CurrentDate));

        let query = parse_query("select name from /test where name = upper").unwrap();
        let right = query.expr.unwrap().right.unwrap();
        assert_eq!(right.function, None);
        assert_eq!(right.val.as_deref(), Some("upper"));
    }

    #[test]
    fn function_names_without_parentheses_are_literals_unless_zero_argument() {
        for (query, literal) in [
            ("select name from /test where ext = log", "log"),
            ("select name from /test where ext = LEN", "LEN"),
            ("select name from /test where ext like year", "year"),
            ("select name from /test where ext = max", "max"),
        ] {
            let right = parse_query(query).expect(query).expr.unwrap().right.unwrap();
            assert_eq!(right.function, None, "{}", query);
            assert_eq!(right.val.as_deref(), Some(literal), "{}", query);
        }

        let query = parse_query("select name from /test where ext in (log, txt)").unwrap();
        let list = query.expr.unwrap().right.unwrap();
        let values: Vec<_> = list.args.unwrap().iter().map(|a| a.val.clone().unwrap()).collect();
        assert_eq!(values, vec!["log", "txt"]);

        let query = parse_query("select upper from /test").unwrap();
        assert_eq!(query.fields[0].function, None);
        assert_eq!(query.fields[0].val.as_deref(), Some("upper"));

        let query = parse_query("select name from /test where log(size) > 1").unwrap();
        assert_eq!(query.expr.unwrap().left.unwrap().function, Some(Function::Log));

        let query = parse_query("select name from /test where ext = now").unwrap();
        assert_eq!(query.expr.unwrap().right.unwrap().function, Some(Function::CurrentTimestamp));

        let err = parse_query("select name from /test where ext = log()").unwrap_err();
        assert_eq!(err, "Function LOG expects 1 to 2 arguments, got 0");
    }

    #[test]
    fn zero_arg_function_without_parens_as_function_arg() {
        let query = "select dayname(curdate()) from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select dayname(curdate) from /test limit 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.fields, query2.fields);
    }

    #[test]
    fn zero_arg_function_without_parens_in_where() {
        let query = "select name from /test where name = curdate()";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select name from /test where name = curdate";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.expr, query2.expr);
    }

    #[test]
    fn zero_arg_function_without_parens_does_not_eat_next_token() {
        let query = "select name from /test where name = curdate and size > 100";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let expr = query.expr.unwrap();
        assert_eq!(expr.logical_op, Some(LogicalOp::And));
        assert!(expr.right.is_some());
    }

    #[test]
    fn multiple_zero_arg_functions_without_parens() {
        let query = "select curdate(), curtime() from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select curdate, curtime from /test limit 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.fields, query2.fields);
    }

    #[test]
    fn notbetween_operator() {
        let query = "select name from /test where size notbetween 100 and 200";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_ok());
        let query = result.unwrap();
        assert!(query.expr.is_some());
    }

    #[test]
    fn not_notbetween_double_negation() {
        // "not notbetween" = double negation = "between"
        let q1 = "select name from /test where size not notbetween 100 and 200";
        let mut lexer1 = Lexer::new(vec![q1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let result1 = p1.parse(false).unwrap();

        let q2 = "select name from /test where size between 100 and 200";
        let mut lexer2 = Lexer::new(vec![q2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let result2 = p2.parse(false).unwrap();

        // Both should produce the same expression tree
        assert_eq!(format!("{:?}", result1.expr), format!("{:?}", result2.expr));
    }

    #[test]
    fn notin_operator() {
        let query = "select name from /test where name notin (a, b, c)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_ok());
        let query = result.unwrap();
        assert!(query.expr.is_some());
    }

    #[test]
    fn nested_functions_without_parens_on_inner() {
        let query = "select upper(dayname(now())) from /test limit 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let query2 = "select upper(dayname(now)) from /test limit 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let query2 = p2.parse(false).unwrap();
        assert!(!p2.there_are_remaining_lexemes());

        assert_eq!(query.fields, query2.fields);
    }

    #[test]
    fn malformed_or_missing_rhs_no_panic() {
        // parse_expr: right side of OR has no expression
        let query = "select name from . where name = test or ";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        // Should return Err, not panic
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn malformed_and_missing_rhs_no_panic() {
        // parse_and: right side of AND has no expression
        let query = "select name from . where name = test and ";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn malformed_comparison_missing_rhs_no_panic() {
        // parse_cond: operator with no right side
        let query = "select name from . where name = ";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
    }

    #[test]
    fn unterminated_quote_returns_parse_error() {
        let query = "select name from . where name = 'unterminated";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unterminated quoted string"));
    }

    #[test]
    fn group_by_positional_index() {
        let query = "select mime, count(*) from /test group by 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.grouping_fields, vec![Expr::field(Field::Mime)]);
    }

    #[test]
    fn group_by_multiple_positional_indices() {
        let query = "select mime, name, count(*) from /test group by 1, 2";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(
            query.grouping_fields,
            vec![Expr::field(Field::Mime), Expr::field(Field::Name)]
        );
    }

    #[test]
    fn group_by_positional_same_as_named() {
        let query1 = "select mime, count(*) from /test group by mime";
        let mut lexer1 = Lexer::new(vec![query1.to_string()]);
        let mut p1 = Parser::new(&mut lexer1);
        let q1 = p1.parse(false).unwrap();

        let query2 = "select mime, count(*) from /test group by 1";
        let mut lexer2 = Lexer::new(vec![query2.to_string()]);
        let mut p2 = Parser::new(&mut lexer2);
        let q2 = p2.parse(false).unwrap();

        assert_eq!(q1.grouping_fields, q2.grouping_fields);
    }

    #[test]
    fn group_by_positional_index_out_of_range() {
        let query = "select mime from /test group by 5";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let result = p.parse(false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of range"));
    }

    #[test]
    fn group_by_positional_with_expression_field() {
        let query = "select size + 1, count(*) from /test group by 1";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.grouping_fields.len(), 1);
        assert!(query.grouping_fields[0].arithmetic_op.is_some());
    }

    #[test]
    fn from_subselect_basic() {
        let query = "select name from (select path from /test depth 2)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.fields, vec![Expr::field(Field::Name)]);
        assert_eq!(query.roots.len(), 1);

        let root = &query.roots[0];
        assert!(root.is_subquery(), "FROM (select ...) must produce a subquery root");
        assert_eq!(root.path, "", "subquery root must carry no filesystem path");

        let inner = root.subquery.as_ref().unwrap();
        assert_eq!(inner.fields, vec![Expr::field(Field::Path)]);
        assert_eq!(
            inner.roots,
            vec![Root::new(
                String::from("/test"),
                RootOptions::from(0, 2, false, false, None, None, None, Bfs, false, None),
            )]
        );
    }

    #[test]
    fn from_subselect_with_outer_where_and_limit() {
        let query = "select name from (select path from /a depth 1) where name like '%.rs' limit 10";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.limit, 10);
        let outer_expr = query.expr.expect("outer WHERE must survive a FROM subselect");
        assert_eq!(outer_expr.op, Some(Op::Like));

        assert!(query.roots[0].is_subquery());
        let inner = query.roots[0].subquery.as_ref().unwrap();
        assert_eq!(inner.fields, vec![Expr::field(Field::Path)]);
    }

    #[test]
    fn from_subselect_nested() {
        let query = "select name from (select path from (select path from /a) where size > 0)";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let outer_root = &query.roots[0];
        assert!(outer_root.is_subquery());
        let middle = outer_root.subquery.as_ref().unwrap();
        assert_eq!(middle.roots.len(), 1);
        assert!(
            middle.roots[0].is_subquery(),
            "the middle query's FROM clause must itself be a subselect root"
        );
        let innermost = middle.roots[0].subquery.as_ref().unwrap();
        assert_eq!(
            innermost.roots,
            vec![Root::new(
                String::from("/a"),
                RootOptions::from(0, 0, false, false, None, None, None, Bfs, false, None),
            )]
        );
    }

    #[test]
    fn depth_field_is_distinct_from_the_depth_root_option() {
        let query = "select name, depth from /test depth 2 where depth > 1 order by depth";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let parsed = p.parse(false).unwrap_or_else(|e| panic!("{}: {}", query, e));
        assert!(!p.there_are_remaining_lexemes(), "{}: leftover lexemes", query);
        assert_eq!(parsed.fields[1].field, Some(Field::Depth));
        assert_eq!(parsed.roots[0].options.max_depth, 2);
        assert_eq!(parsed.expr.unwrap().left.unwrap().field, Some(Field::Depth));
        assert_eq!(parsed.ordering_fields[0].field, Some(Field::Depth));

        let parsed = parse_query("select depth from /test").unwrap();
        assert_eq!(parsed.fields[0].field, Some(Field::Depth));
        assert_eq!(parsed.roots[0].options.max_depth, 0);

        let parsed = parse_query("select name, depth depth 3").unwrap();
        assert_eq!(parsed.fields[1].field, Some(Field::Depth));
        assert_eq!(parsed.roots[0].options.max_depth, 3);
    }

    #[test]
    fn bare_aliased_boolean_field_keeps_its_root_alias() {
        let expr = parse_where("select name from /test as t1 where t1.is_file");
        let left = expr.left.as_ref().unwrap();
        assert_eq!(left.field, Some(Field::IsFile));
        assert_eq!(left.root_alias.as_deref(), Some("t1"), "root alias dropped on implicit `= true`");
        assert_eq!(expr.op, Some(Op::Eq));
        assert_eq!(expr.right.as_ref().unwrap().val.as_deref(), Some("true"));

        let negated = parse_where("select name from /test as t1 where not t1.is_dir");
        let left = negated.left.as_ref().unwrap();
        assert_eq!(left.field, Some(Field::IsDir));
        assert_eq!(left.root_alias.as_deref(), Some("t1"), "root alias dropped under NOT");
        assert_eq!(negated.op, Some(Op::Ne));
    }

    #[test]
    fn select_alias_named_group_is_not_the_group_by_keyword() {
        let query = "select name as group from /test order by group";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let parsed = p.parse(false).unwrap_or_else(|e| panic!("{}: {}", query, e));
        assert!(!p.there_are_remaining_lexemes(), "{}: leftover lexemes", query);
        assert_eq!(parsed.fields[0].alias.as_deref(), Some("group"));
        assert_eq!(parsed.ordering_fields[0].field, Some(Field::Name), "alias not resolved in ORDER BY");
    }

    #[cfg(all(unix, feature = "users"))]
    #[test]
    fn group_field_is_selectable_everywhere() {
        let query = "select group, name from /test where group = 'wheel' group by group, name order by group";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let parsed = p.parse(false).unwrap_or_else(|e| panic!("{}: {}", query, e));
        assert!(!p.there_are_remaining_lexemes(), "{}: leftover lexemes", query);
        assert_eq!(parsed.fields[0].field, Some(Field::Group));
        assert_eq!(parsed.fields[1].field, Some(Field::Name));
        assert_eq!(parsed.expr.unwrap().left.unwrap().field, Some(Field::Group));
        assert_eq!(parsed.grouping_fields[0].field, Some(Field::Group));
        assert_eq!(parsed.ordering_fields[0].field, Some(Field::Group));
    }

    #[test]
    fn root_alias_is_case_insensitive() {
        // `T1.size` is lowercased by Field::parse_field, so the declared alias
        // must be stored lowercased too or `as T1` can never be referenced.
        for query in [
            "select T1.name from /test as T1 where T1.size > 0",
            "select t1.name from /test as T1",
            "select Src.name from (select path from /a) as SRC",
        ] {
            let mut lexer = Lexer::new(vec![query.to_string()]);
            let mut p = Parser::new(&mut lexer);
            let parsed = p.parse(false).unwrap_or_else(|e| panic!("{}: {}", query, e));
            assert!(!p.there_are_remaining_lexemes(), "{}: leftover lexemes", query);
            let alias = parsed.roots[0].options.alias.as_deref().unwrap();
            assert_eq!(alias, alias.to_lowercase(), "{}: alias stored verbatim", query);
            assert_eq!(parsed.fields[0].root_alias.as_deref(), Some(alias), "{}: field alias != root alias", query);
        }
    }

    #[test]
    fn from_subselect_with_alias_after_close() {
        let query = "select src.name from (select path from /a) as src";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let root = &query.roots[0];
        assert!(root.is_subquery());
        assert_eq!(
            root.options.alias.as_deref(),
            Some("src"),
            "alias after a FROM subselect should attach to the subquery root"
        );
    }

    #[test]
    fn from_subselect_curly_braces() {
        let query = "select name from {select path from /a}";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        let root = &query.roots[0];
        assert!(root.is_subquery());
        let inner = root.subquery.as_ref().unwrap();
        assert_eq!(inner.fields, vec![Expr::field(Field::Path)]);
    }

    #[test]
    fn from_subselect_mixed_with_plain_root() {
        let query = "select name from (select path from /a), /b";
        let mut lexer = Lexer::new(vec![query.to_string()]);
        let mut p = Parser::new(&mut lexer);
        let query = p.parse(false).unwrap();
        assert!(!p.there_are_remaining_lexemes());

        assert_eq!(query.roots.len(), 2);
        assert!(query.roots[0].is_subquery());
        assert!(!query.roots[1].is_subquery());
        assert_eq!(query.roots[1].path, "/b");
    }

    fn parse_query(sql: &str) -> Result<Query, String> {
        let mut lexer = Lexer::new(vec![sql.to_string()]);
        let mut p = Parser::new(&mut lexer);
        p.parse(false)
    }

    #[test]
    fn where_is_file_sets_prop() {
        let query = parse_query("select name from /test where is_file and size > 0").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), Some(true));

        let query = parse_query("select name from /test where size > 0 and is_file = true").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), Some(true));

        let query = parse_query("select name from /test where is_file = false and size > 0").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), Some(false));

        let query = parse_query("select name from /test where is_file or size > 0").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), None);

        let query = parse_query("select is_file from /test").unwrap();
        assert_eq!(query.fields[0].bool_prop(IS_FILE), Some(true));
    }

    #[test]
    fn where_is_dir_sets_prop() {
        let query = parse_query("select name from /test where is_dir and size > 0").unwrap();
        let expr = query.expr.unwrap();
        assert_eq!(expr.bool_prop(IS_DIR), Some(true));
        assert_eq!(expr.bool_prop(IS_FILE), Some(false));

        let query = parse_query("select name from /test where not is_dir").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_DIR), None);

        let err = parse_query("select name from /test where is_dir and is_dir = false").unwrap_err();
        assert!(err.contains("is_dir"), "unexpected error: {}", err);

        let err = parse_query("select name from /test where is_file and is_dir").unwrap_err();
        assert!(err.contains("requires is_dir = false, but 'IsDir Eq true' requires is_dir = true"), "unexpected error: {}", err);

        let err = parse_query("select name from /test where is_dir and size > 0 and is_pipe").unwrap_err();
        assert!(err.contains("is_dir"), "unexpected error: {}", err);

        let query = parse_query("select name from /test where is_file or is_pipe").unwrap();
        let expr = query.expr.unwrap();
        assert_eq!(expr.bool_prop(IS_DIR), Some(false));
        assert_eq!(expr.bool_prop(IS_FILE), None);
        assert_eq!(expr.bool_prop(IS_PIPE), None);
        assert_eq!(expr.bool_prop(IS_SOCKET), Some(false));

        let err = parse_query("select name from /test where is_pipe and is_socket").unwrap_err();
        assert!(err.contains("is_pipe") || err.contains("is_socket"), "unexpected error: {}", err);
    }

    #[test]
    fn function_argument_counts_are_checked_at_parse_time() {
        let err = parse_query("select lower(name, name) from /test").unwrap_err();
        assert_eq!(err, "Function LOWER expects 1 argument, got 2");

        let err = parse_query("select name from /test where substring(name, 1, 2, 3) = 'ab'").unwrap_err();
        assert!(err.contains("SUBSTRING expects 2 to 3 arguments, got 4"), "{}", err);

        let err = parse_query("select name from /test order by length(name, 1)").unwrap_err();
        assert!(err.contains("LENGTH"), "{}", err);

        let err = parse_query("select count(*) from /test group by replace(name, 'a')").unwrap_err();
        assert!(err.contains("REPLACE expects 3 arguments, got 2"), "{}", err);

        let err = parse_query("select ext, count(*) from /test group by ext having min(size, 1) > 0").unwrap_err();
        assert!(err.contains("MIN"), "{}", err);

        let err = parse_query("select name from /test where size in (select upper(size, 1) from /other)").unwrap_err();
        assert!(err.contains("UPPER"), "{}", err);

        // Without parentheses the name of a function that needs arguments is
        // a string literal, not a call.
        let query = parse_query("select length from /test").unwrap();
        assert_eq!(query.fields[0].function, None);
        assert_eq!(query.fields[0].val.as_deref(), Some("length"));

        let err = parse_query("select pi(1) from /test").unwrap_err();
        assert_eq!(err, "Function PI expects no arguments, got 1");

        for ok in [
            "select coalesce(name) from /test",
            "select coalesce(name, ext, 'x'), concat_ws('-', name, ext) from /test",
            "select count(*), count, count(name) from /test",
            "select curdate(), now, pi(), rand(1, 10) from /test",
            "select substring(name, 1), substring(name, 1, 2), round(size), round(size, 2) from /test",
            "select name from /test where contains('x') and lower(name) = 'a'",
            "select min(size * 2), max(size) from /test group by ext order by count(*)",
        ] {
            parse_query(ok).unwrap_or_else(|e| panic!("{}: {}", ok, e));
        }
    }

    #[test]
    fn file_type_conditions_on_different_root_aliases_do_not_conflict() {
        // `t2` is the subquery's own root, `t1` the outer entry: a directory
        // in the outer query can be paired with files in the inner one.
        let query = parse_query(
            "select name from /outer as t1 where exists (select name from /inner as t2 where t2.is_file and t1.is_dir)",
        )
        .unwrap_or_else(|e| panic!("cross-alias file-type conditions rejected: {}", e));
        assert!(query.expr.is_some());

        // The subquery's own alias is the plain entry, so this is still a contradiction.
        let err = parse_query(
            "select name from /outer as t1 where exists (select name from /inner as t2 where t2.is_file and t2.is_dir)",
        )
        .unwrap_err();
        assert!(err.contains("is_dir"), "unexpected error: {}", err);

        // So is mixing the local alias with the bare field in the outer query ...
        let err = parse_query("select name from /test as t1 where t1.is_file and is_dir").unwrap_err();
        assert!(err.contains("is_dir"), "unexpected error: {}", err);

        // ... and asking two different things of the same outer entry.
        let err = parse_query(
            "select name from /outer as t1 where exists (select name from /inner as t2 where t1.is_file and t1.is_dir)",
        )
        .unwrap_err();
        assert!(err.contains("t1.is_dir"), "unexpected error: {}", err);
    }

    #[test]
    fn negated_contradiction_is_a_tautology_not_an_error() {
        // `not (is_file and is_dir)` is true for every entry; only the
        // un-negated form can never match.
        let query = parse_query("select name from /test where not (is_file and is_dir)")
            .unwrap_or_else(|e| panic!("negated contradiction rejected: {}", e));
        let expr = query.expr.unwrap();
        assert_eq!(expr.bool_prop(IS_FILE), None);
        assert_eq!(expr.bool_prop(IS_DIR), None);

        assert!(parse_query("select name from /test where not (size > 0 and (is_file and is_dir)) and is_file").is_ok());
        assert!(parse_query("select name from /test as t1 where not (t1.is_file and is_dir)").is_ok());
        assert!(parse_query("select name from /test where not not (is_file and is_dir)").is_err());
    }

    #[test]
    fn not_clears_is_file_prop() {
        let query = parse_query("select name from /test where not is_file = true").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), None);

        let query = parse_query("select name from /test where not (is_file and size > 0)").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), None);
    }

    #[test]
    fn conflicting_is_file_conditions_are_an_error() {
        let err = parse_query("select name from /test where is_file = true and is_file = false").unwrap_err();
        assert!(err.contains("is_file"), "unexpected error: {}", err);

        let err = parse_query("select name from /test where is_file and size > 0 and is_file != true").unwrap_err();
        assert!(err.contains("is_file"), "unexpected error: {}", err);

        assert!(parse_query("select name from /test where is_file and not is_file").is_ok());

        let query = parse_query("select name from /test where is_file or (is_file = false and size > 1mb)").unwrap();
        assert_eq!(query.expr.unwrap().bool_prop(IS_FILE), None);
    }

    #[test]
    fn unclosed_function_call_is_an_error_not_a_literal() {
        // Previously the failed function parse was discarded, the consumed
        // lexemes were lost, and "length" became a string literal: the WHERE
        // clause silently matched nothing and the SELECT list searched cwd.
        for sql in [
            "select name from /test where length(name > 3",
            "select name from /test where upper(name = 'A.TXT'",
            "select upper(name from /test limit 1",
            "select name, upper(ext from /test",
            "select name from /test order by length(name limit 1",
            "select name, length(name, from /test",
            "select name from /test where length{name > 3",
        ] {
            let err = parse_query(sql).unwrap_err();
            assert!(
                err.contains("function expression") && (err.contains("Length") || err.contains("Upper")),
                "{}: unexpected error: {}", sql, err
            );
        }
    }

    fn has_subquery(expr: &Expr) -> bool {
        expr.subquery.is_some()
            || expr.left.as_deref().is_some_and(has_subquery)
            || expr.right.as_deref().is_some_and(has_subquery)
            || expr.args.iter().flatten().any(has_subquery)
    }

    fn leftmost_field(expr: &Expr) -> Option<Field> {
        match expr.left.as_deref() {
            Some(left) => leftmost_field(left),
            None => expr.field,
        }
    }

    #[test]
    fn logical_chains_sort_every_operand_by_weight() {
        let sorted = parse_query("select name from /test where name like '%.md' and size > 0 and contains('x')").unwrap();
        for query in [
            "select name from /test where size > 0 and contains('x') and name like '%.md'",
            "select name from /test where contains('x') and size > 0 and name like '%.md'",
            "select name from /test where contains('x') and name like '%.md' and size > 0",
        ] {
            assert_eq!(parse_query(query).unwrap().expr, sorted.expr, "{}", query);
        }

        let sorted = parse_query("select name from /test where name like '%.md' or size > 0 or contains('x')").unwrap();
        let query = "select name from /test where size > 0 or contains('x') or name like '%.md'";
        assert_eq!(parse_query(query).unwrap().expr, sorted.expr, "{}", query);
    }

    #[test]
    fn equal_weights_keep_their_textual_order() {
        let a = parse_query("select name from /test where name = 'a' and ext = 'b' and path = 'c'").unwrap().expr.unwrap();
        let b = parse_query("select name from /test where ext = 'b' and name = 'a' and path = 'c'").unwrap().expr.unwrap();
        assert_ne!(a, b);
        assert_eq!(leftmost_field(&a), Some(Field::Name));
        assert_eq!(leftmost_field(&b), Some(Field::Extension));
    }

    #[test]
    fn parenthesized_groups_are_sorted_as_one_operand() {
        let expr = parse_query("select name from /test where (contains('x') or name like '%.md') and size > 0").unwrap().expr.unwrap();
        assert_eq!(expr.logical_op, Some(LogicalOp::And));
        assert_eq!(leftmost_field(&expr), Some(Field::Size), "the stat comparison must come before the content-reading group");
        let group = expr.right.unwrap();
        assert_eq!(group.logical_op, Some(LogicalOp::Or));
        assert_eq!(leftmost_field(&group), Some(Field::Name), "inside the group the name test must come before contains()");
    }

    #[test]
    fn subqueries_sort_after_cheap_conditions() {
        let correlated = "(select count(*) from /inner as t2 where t2.name = t1.name) > 0";
        for query in [
            format!("select name from /outer as t1 where {} and t1.name like '%.md'", correlated),
            format!("select name from /outer as t1 where t1.name like '%.md' and {}", correlated),
        ] {
            let expr = parse_query(&query).unwrap().expr.unwrap();
            assert_eq!(leftmost_field(&expr), Some(Field::Name), "{}", query);
            assert!(!has_subquery(expr.left.as_ref().unwrap()), "{}", query);
            assert!(has_subquery(expr.right.as_ref().unwrap()), "{}", query);
        }

        let c = parse_query(&format!("select name from /outer as t1 where {} and contains('x')", correlated)).unwrap().expr.unwrap();
        assert!(!has_subquery(c.left.as_ref().unwrap()), "a correlated subquery is dearer than reading the file");

        for query in [
            "select name from /test where name in (select name from /inner) and size > 0",
            "select name from /test where size > 0 and name in (select name from /inner)",
        ] {
            let expr = parse_query(query).unwrap().expr.unwrap();
            assert_eq!(leftmost_field(&expr), Some(Field::Size), "{}", query);
            assert!(has_subquery(expr.right.as_ref().unwrap()), "{}", query);
        }
    }

    #[test]
    fn subquery_weight_covers_the_scan_and_correlation() {
        let weight_of = |query: &str| parse_query(query).unwrap().expr.unwrap().right.unwrap().weight;
        let plain = weight_of("select name from /t where name in (select name from /inner)");
        let filtered = weight_of("select name from /t where name in (select name from /inner where size > 0)");
        let correlated = weight_of("select name from /t as t1 where name in (select name from /inner as t2 where t2.size = t1.size)");
        assert_eq!(plain, Expr::SUBQUERY_SCAN_WEIGHT);
        assert_eq!(filtered, Expr::SUBQUERY_SCAN_WEIGHT + 1);
        assert_eq!(correlated, Expr::CORRELATED_SUBQUERY_WEIGHT + Expr::SUBQUERY_SCAN_WEIGHT + 2);
        assert!(Expr::CORRELATED_SUBQUERY_WEIGHT > 100 * Function::Contains.get_weight());
    }

    #[test]
    fn unparsable_date_literals_in_date_functions_are_rejected_at_parse_time() {
        let rejected = [
            "select year('\u{FF12}\u{FF10}\u{FF12}\u{FF16}-\u{FF11}\u{FF10}-\u{FF11}\u{FF15}') from /x",
            "select year('abc') from /x",
            "select name from /x where day('bad') > 0",
            "select date_diff(modified, 'nope') from /x",
            "select extract('year', 'abc') from /x",
            "select date_trunc('day', '2024-13-45') from /x",
            "select name from /x order by date_add('x', 1)",
            "select ext from /x group by ext having max(year('x')) > 0",
        ];
        for query in rejected {
            let err = parse_query(query).expect_err(query);
            assert!(err.contains("Can't parse datetime"), "{}: unexpected error text {:?}", query, err);
        }

        let accepted = [
            "select year('2024-01-01'), year(-1), year(yesterday), year(modified), year(name) from /x",
            "select date_diff(modified, '2024-01-01'), extract('year', modified), date_trunc('day', '2024-01-01 10:00:00') from /x",
            "select date_add('2024-01-01', 3), date_sub(modified, 3), last_day('2024-02-10') from /x",
            "select upper('abc'), length('not a date') from /x",
        ];
        for query in accepted {
            assert!(parse_query(query).is_ok(), "{} should parse", query);
        }
    }

    #[test]
    fn malformed_in_lists_are_rejected() {
        for (query, expected) in [
            ("select name from /test where ext in ()", "IN list cannot be empty"),
            ("select name from /test where ext not in {}", "IN list cannot be empty"),
            ("select name from /test where ext in ('txt' 'md')", "Expected ',' or closing parenthesis in IN list"),
            ("select name from /test where ext in ('txt' 'md', 'rs')", "Expected ',' or closing parenthesis in IN list"),
            ("select name from /test where ext in ('txt',)", "Missing value after ',' in IN list"),
            ("select name from /test where ext in ('txt', , 'md')", "Missing value after ',' in IN list"),
            ("select name from /test where ext in (, 'txt')", "Missing value before ',' in IN list"),
            ("select name from /test where ext in {'txt',}", "Missing value after ',' in IN list"),
        ] {
            let err = parse_err(query);
            assert!(err.contains(expected), "{}
expected {:?}, got {:?}", query, expected, err);
        }
    }

    #[test]
    fn well_formed_in_lists_still_parse() {
        for (query, members) in [
            ("select name from /test where ext in ('txt')", 1),
            ("select name from /test where ext in ('txt', 'md')", 2),
            ("select name from /test where ext in {'txt', 'md', 'rs'}", 3),
            ("select name from /test where ext not in ('txt' , 'md')", 2),
            ("select name from /test where size in (1, -2, 3 * 4)", 3),
            ("select name from /test where ext in (lower(name), upper(ext), 'x')", 3),
            ("select name from /test where ext in ('txt', 'md') and size > 0", 2),
        ] {
            let query = parse_query(query).unwrap_or_else(|e| panic!("{}: {}", query, e));
            let expr = query.expr.as_ref().unwrap();
            let list = if expr.logical_op.is_some() { expr.left.as_ref().unwrap() } else { expr };
            let args = list.right.as_ref().unwrap().args.as_ref().unwrap();
            assert_eq!(args.len(), members, "{:?}", list);
        }

        let query = parse_query("select name from /test where name in (select name from /other)").unwrap();
        assert!(query.expr.unwrap().right.unwrap().subquery.is_some());
    }

    #[test]
    fn well_formed_function_calls_still_parse() {
        let query = parse_query("select curdate() from /test").unwrap();
        let expr = &query.fields[0];
        assert!(expr.function.is_some());
        assert!(expr.left.is_none());
        assert!(expr.args.as_ref().is_none_or(|args| args.is_empty()));

        let query = parse_query("select upper{name}, concat(name, '-', ext) from /test where length(name) > 3").unwrap();
        assert_eq!(query.fields.len(), 2);
        assert_eq!(format!("{}", query.fields[0]), "Upper(Name)");
        assert_eq!(query.fields[1].args.as_ref().map(|a| a.len()), Some(2));
        assert_eq!(format!("{}", query.expr.unwrap()), "Length(Name) Gt 3");

        // A bare zero-argument function name is still a call; the bare name
        // of a function that needs arguments is a string literal.
        let query = parse_query("select name from /test where name = now").unwrap();
        assert!(query.expr.unwrap().right.unwrap().function.is_some());
        let query = parse_query("select name from /test where name = year").unwrap();
        let right = query.expr.unwrap().right.unwrap();
        assert_eq!(right.function, None);
        assert_eq!(right.val.as_deref(), Some("year"));
    }
}

//! Defines the query struct and related types.
//! Query parsing is handled in the `parser` module

use std::collections::HashSet;
use std::rc::Rc;

use crate::expr::Expr;
use crate::field::Field;
use crate::query::TraversalMode::Bfs;

#[derive(Debug, Clone)]
/// Represents a query to be executed on .
///
pub struct Query {
    /// File fields to be selected
    pub fields: Vec<Expr>,
    /// Root directories to search
    pub roots: Vec<Root>,
    /// "where" filter expression
    pub expr: Option<Expr>,
    /// Fields to group by
    pub grouping_fields: Rc<Vec<Expr>>,
    /// Fields to order by
    pub ordering_fields: Rc<Vec<Expr>>,
    /// Ordering direction (true for asc, false for desc)
    pub ordering_asc: Rc<Vec<bool>>,
    /// Max amount of results to return
    pub limit: u32,
    /// Output format
    pub output_format: OutputFormat,
}

impl Query {
    pub fn get_all_fields(&self) -> HashSet<Field> {
        let mut result = HashSet::new();

        for column_expr in &self.fields {
            result.extend(column_expr.get_required_fields());
        }

        result
    }

    pub fn is_ordered(&self) -> bool {
        !self.ordering_fields.is_empty()
    }

    pub fn has_aggregate_column(&self) -> bool {
        self.fields.iter().any(|ref f| f.has_aggregate_function())
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Represents a root directory to start the search from, with traversal options.
pub struct Root {
    pub path: String,
    pub options: RootOptions,
}

#[derive(Debug, Clone, PartialEq)]
/// Represents the traversal options for a root directory.
pub struct RootOptions {
    /// Minimum depth to search
    pub min_depth: u32,
    /// Maximum depth to search
    pub max_depth: u32,
    /// Whether to search archives
    pub archives: bool,
    /// Whether to follow symlinks
    pub symlinks: bool,
    /// Whether to respect .gitignore files
    pub gitignore: Option<bool>,
    /// Whether to respect .hgignore files
    pub hgignore: Option<bool>,
    /// Whether to respect .dockerignore files
    pub dockerignore: Option<bool>,
    /// The traversal mode to use
    pub traversal: TraversalMode,
    /// Treat the path as a regular expression
    pub regexp: bool,
}

impl RootOptions {
    pub fn new() -> RootOptions {
        RootOptions {
            min_depth: 0,
            max_depth: 0,
            archives: false,
            symlinks: false,
            gitignore: None,
            hgignore: None,
            dockerignore: None,
            traversal: Bfs,
            regexp: false,
        }
    }

    #[cfg(test)]
    pub fn from(
        min_depth: u32,
        max_depth: u32,
        archives: bool,
        symlinks: bool,
        gitignore: Option<bool>,
        hgignore: Option<bool>,
        dockerignore: Option<bool>,
        traversal: TraversalMode,
        regexp: bool,
    ) -> RootOptions {
        RootOptions {
            min_depth,
            max_depth,
            archives,
            symlinks,
            gitignore,
            hgignore,
            dockerignore,
            traversal,
            regexp,
        }
    }
}

impl Root {
    pub fn new(path: String, options: RootOptions) -> Root {
        Root { path, options }
    }

    pub fn default(options: Option<RootOptions>) -> Root {
        Root {
            path: String::from("."),
            options: options.unwrap_or_else(RootOptions::new),
        }
    }

    pub fn clone_with_path(new_path: String, source: Root) -> Root {
        Root {
            path: new_path,
            ..source
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TraversalMode {
    Bfs,
    Dfs,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Tabs,
    Lines,
    List,
    Csv,
    Json,
    Html,
}

impl OutputFormat {
    pub fn from(s: &str) -> Option<OutputFormat> {
        let s = s.to_lowercase();

        match s.as_str() {
            "lines" => Some(OutputFormat::Lines),
            "list" => Some(OutputFormat::List),
            "csv" => Some(OutputFormat::Csv),
            "json" => Some(OutputFormat::Json),
            "tabs" => Some(OutputFormat::Tabs),
            "html" => Some(OutputFormat::Html),
            _ => None,
        }
    }
}

use std::collections::HashSet;
use std::rc::Rc;

use crate::expr::Expr;
use crate::field::Field;
use crate::query::TraversalMode::Bfs;

#[derive(Debug, Clone)]
pub struct Query {
    pub fields: Vec<Expr>,
    pub roots: Vec<Root>,
    pub expr: Option<Expr>,
    pub ordering_fields: Rc<Vec<Expr>>,
    pub ordering_asc: Rc<Vec<bool>>,
    pub limit: u32,
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
pub struct Root {
    pub path: String,
    pub min_depth: u32,
    pub max_depth: u32,
    pub archives: bool,
    pub symlinks: bool,
    pub gitignore: Option<bool>,
    pub hgignore: Option<bool>,
    pub dockerignore: Option<bool>,
    pub traversal: TraversalMode,
    pub regexp: bool,
}

impl Root {
    pub fn new(path: String,
               min_depth: u32,
               max_depth: u32,
               archives: bool,
               symlinks: bool,
               gitignore: Option<bool>,
               hgignore: Option<bool>,
               dockerignore: Option<bool>,
               traversal: TraversalMode,
               regexp: bool) -> Root {
        Root {
            path,
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

    pub fn default() -> Root {
        Root {
            path: String::from("."),
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
    pub fn from(s: &String) -> Option<OutputFormat> {
        let s = s.to_lowercase();
        if s == "lines" {
            return Some(OutputFormat::Lines);
        } else if s == "list" {
            return Some(OutputFormat::List);
        } else if s == "csv" {
            return Some(OutputFormat::Csv);
        } else if s == "json" {
            return Some(OutputFormat::Json);
        } else if s == "tabs" {
            return Some(OutputFormat::Tabs);
        } else if s == "html" {
            return Some(OutputFormat::Html);
        } else {
            return None;
        }
    }
}

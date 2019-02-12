use std::collections::HashSet;
use std::rc::Rc;

use crate::expr::Expr;
use crate::field::Field;

#[derive(Debug, Clone)]
pub struct Query {
    pub fields: Vec<Expr>,
    pub roots: Vec<Root>,
    pub expr: Option<Expr>,
    pub ordering_fields: Vec<Expr>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub path: String,
    pub min_depth: u32,
    pub max_depth: u32,
    pub archives: bool,
    pub symlinks: bool,
    pub gitignore: bool,
    pub hgignore: bool,
}

impl Root {
    pub fn new(path: String, min_depth: u32, max_depth: u32, archives: bool, symlinks: bool, gitignore: bool, hgignore: bool) -> Root {
        Root { path, min_depth, max_depth, archives, symlinks, gitignore, hgignore }
    }

    pub fn default() -> Root {
        Root { path: String::from("."), min_depth: 0, max_depth: 0, archives: false, symlinks: false, gitignore: false, hgignore: false }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Tabs, Lines, List, Csv, Json, Html
}

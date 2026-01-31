//! Defines the query struct and related types.
//! Query parsing is handled in the `parser` module

use std::collections::HashSet;

use crate::expr::Expr;
use crate::field::Field;
use crate::query::TraversalMode::Bfs;

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
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
    pub grouping_fields: Vec<Expr>,
    /// Fields to order by
    pub ordering_fields: Vec<Expr>,
    /// Ordering direction (true for asc, false for desc)
    pub ordering_asc: Vec<bool>,
    /// Max amount of results to return
    pub limit: u32,
    /// Output format
    pub output_format: OutputFormat,
    pub raw_query: String,
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

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
/// Represents a root directory to start the search from, with traversal options.
pub struct Root {
    pub path: String,
    pub options: RootOptions,
}

macro_rules! root_options {
    (
        $(#[$struct_attrs:meta])*
        $vis:vis struct $struct_name:ident {
            $(
                $(
                    @text = [$($text:literal),*], description = $description:literal
                )+
                $(#[$field_attrs:meta])*
                $field_vis:vis $field:ident: $field_type:ty
            ),*
            $(,)?
        }
    ) => {
        $(#[$struct_attrs])*
        $vis struct $struct_name {
            $(
                $(#[$field_attrs])*
                $field_vis $field: $field_type,
            )*
        }
        
        impl $struct_name {
            pub fn get_names_and_descriptions() -> Vec<(Vec<&'static str>, &'static str)> {
                vec![
                    $(
                        $(#[$field_attrs])*
                        $(                         
                            (vec![$($text,)*], $description),
                        )+
                    )*
                ]
            }
        }
    };
}

root_options! {
    #[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
    pub struct RootOptions {
        @text = ["mindepth"], description = "Minimum depth to search"
        pub min_depth: u32,
        
        @text = ["maxdepth", "depth"], description = "Maximum depth to search"
        pub max_depth: u32,
        
        @text = ["archives", "arc"], description = "Whether to search archives"
        pub archives: bool,
        
        @text = ["symlinks", "sym"], description = "Whether to follow symlinks"
        pub symlinks: bool,

        @text = ["gitignore", "git"], description = "Search respects .gitignore files found"
        @text = ["nogitignore", "nogit"], description = "Disable .gitignore parsing during the search"
        pub gitignore: Option<bool>,
        
        @text = ["hgignore", "hg"], description = "Search respects .hgignore files found"
        @text = ["nohgignore", "nohg"], description = "Disable .hgignore parsing during the search"
        pub hgignore: Option<bool>,
        
        @text = ["dockerignore", "docker"], description = "Search respects .dockerignore files found"
        @text = ["nodockerignore", "nodocker"], description = "Disable .dockerignore parsing during the search"
        pub dockerignore: Option<bool>,
        
        @text = ["dfs"], description = "Depth-first search mode"
        @text = ["bfs"], description = "Breadth-first search mode (default)"
        pub traversal: TraversalMode,
        
        @text = ["regexp", "rx"], description = "Treat the path as a regular expression"
        pub regexp: bool,

        @text = ["as"], description = "Alias for the root path"
        pub alias: Option<String>,
    }
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
            alias: None,
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
        alias: Option<String>,
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
            alias,
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

#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, Hash, Serialize)]
pub enum TraversalMode {
    Bfs,
    Dfs,
}

macro_rules! output_format {
    (
        $(#[$enum_attrs:meta])*
        $vis:vis enum $enum_name:ident {
            $(
                @text = $text:literal
                @description = $description:literal
                $(#[$variant_attrs:meta])*
                $(@supports_colorization = $colorized:literal)?
                $variant:ident$(,)?
            )*
        }
    ) => {
        $(#[$enum_attrs])*
        $vis enum $enum_name {
            $(
                $(#[$variant_attrs])*
                $variant,
            )*
        }
        
        impl $enum_name {
            pub fn from(s: &str) -> Option<OutputFormat> {
                let s = s.to_lowercase();
                match s.as_str() {
                    $(
                        $text => Some($enum_name::$variant),
                    )*
                    _ => None,
                }
            }
            
            pub fn get_names_and_descriptions() -> Vec<(&'static str, &'static str)> {
                vec![
                    $(
                        ($text, $description),
                    )*
                ]
            }
            
            pub fn supports_colorization(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($supports_colorization) == "true"
                        }
                    )*
                }
            }
        }
    };
}

output_format! {
    #[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Serialize)]
    pub enum OutputFormat {
        @text = "tabs"
        @description = "Tab-separated values (default)"
        @supports_colorization = true
        Tabs,
        
        @text = "lines"
        @description = "One item per line"
        @supports_colorization = true
        Lines,
        
        @text = "list"
        @description = "Entire output onto a single line for xargs"
        List,
        
        @text = "csv"
        @description = "Comma-separated values"
        Csv,
        
        @text = "json"
        @description = "JSON format"
        Json,
        
        @text = "html"
        @description = "HTML format"
        Html,
    }
}
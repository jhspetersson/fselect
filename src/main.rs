//! The entry point of the program
//! Handles the command line arguments parsing

#[macro_use]
extern crate serde_derive;
#[cfg(all(unix, feature = "users"))]
extern crate uzers;
#[cfg(unix)]
extern crate xattr;

use std::env;
use std::io::{stdout, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
#[cfg(feature = "update-notifications")]
use std::time::Duration;

use nu_ansi_term::Color::*;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
#[cfg(feature = "update-notifications")]
use update_informer::{registry, Check};

use crate::config::Config;
use crate::field::Field;
use crate::function::Function;
use crate::parser::Parser;
use crate::query::RootOptions;
use crate::searcher::Searcher;
use crate::util::{error_exit, error_message};
use crate::util::str_to_bool;

mod config;
mod expr;
mod field;
mod fileinfo;
mod function;
mod ignore;
mod lexer;
mod mode;
mod operators;
mod output;
mod parser;
mod query;
mod searcher;
mod util;

fn main() -> ExitCode {
    let default_config = Config::default();

    let mut config = match Config::new() {
        Ok(cnf) => cnf,
        Err(err) => {
            eprintln!("{}", err);
            default_config.clone()
        }
    };

    let env_var_value = std::env::var("NO_COLOR").ok().unwrap_or_default();
    let env_no_color = str_to_bool(&env_var_value).unwrap_or(false);
    let mut no_color = env_no_color || config.no_color.unwrap_or(false);

    #[cfg(windows)]
    {
        if !no_color {
            let res = nu_ansi_term::enable_ansi_support();
            let win_init_ok = match res {
                Ok(()) => true,
                Err(203) => true,
                _ => false,
            };
            no_color = !win_init_ok;
        }
    }

    if env::args().len() == 1 {
        short_usage_info(no_color);
        help_hint();
        return ExitCode::SUCCESS;
    }

    let mut args: Vec<String> = env::args().collect();
    args.remove(0);

    let mut first_arg = args[0].to_ascii_lowercase();

    if first_arg.contains("version") || first_arg.starts_with("-v") {
        short_usage_info(no_color);
        return ExitCode::SUCCESS;
    }

    if first_arg.contains("help")
        || first_arg.starts_with("-h")
        || first_arg.starts_with("/?")
        || first_arg.starts_with("/h")
    {
        usage_info(config, default_config, no_color);
        return ExitCode::SUCCESS;
    }

    if first_arg.starts_with("--fields") {
        complete_fields_info();
        return ExitCode::SUCCESS;
    }

    if first_arg.starts_with("--functions") {
        complete_functions_info();
        return ExitCode::SUCCESS;
    }

    if first_arg.starts_with("--root-options") {
        complete_root_options_info();
        return ExitCode::SUCCESS;
    }

    let mut interactive = false;

    loop {
        if first_arg.contains("nocolor") || first_arg.contains("no-color") {
            no_color = true;
        } else if first_arg.starts_with("-i")
            || first_arg.starts_with("--i")
            || first_arg.starts_with("/i")
        {
            interactive = true;
        } else if first_arg.starts_with("-c")
            || first_arg.starts_with("--config")
            || first_arg.starts_with("/c")
        {
            let config_path = args[1].to_ascii_lowercase();
            config = match Config::from(PathBuf::from(&config_path)) {
                Ok(cnf) => cnf,
                Err(err) => {
                    eprintln!("{}", err);
                    default_config.clone()
                }
            };

            args.remove(0);
        } else {
            break;
        }

        args.remove(0);

        if args.is_empty() {
            if !interactive {
                short_usage_info(no_color);
                help_hint();
                return ExitCode::SUCCESS;
            } else {
                break;
            }
        }

        first_arg = args[0].to_ascii_lowercase();
    }

    let mut exit_value = None::<u8>;

    if interactive {
        match DefaultEditor::new() {
            Ok(mut rl) => loop {
                let readline = rl.readline("query> ");
                match readline {
                    Ok(cmd)
                        if cmd.to_ascii_lowercase().trim() == "quit"
                            || cmd.to_ascii_lowercase().trim() == "exit" =>
                    {
                        break
                    }
                    Ok(query) => {
                        let _ = rl.add_history_entry(query.as_str());
                        exec_search(vec![query], &mut config, &default_config, no_color);
                    }
                    Err(ReadlineError::Interrupted) => {
                        println!("CTRL-C");
                        break;
                    }
                    Err(ReadlineError::Eof) => {
                        println!("CTRL-D");
                        break;
                    }
                    Err(err) => {
                        let err = format!("{:?}", err);
                        error_message("input", &err);
                        break;
                    }
                }
            },
            _ => {
                error_message("editor", "couldn't open line editor");
                exit_value = Some(2);
            }
        }
    } else {
        exit_value = Some(exec_search(args, &mut config, &default_config, no_color));
    }

    config.save();

    #[cfg(feature = "update-notifications")]
    if config.check_for_updates.unwrap_or(false) && stdout().is_terminal() {
        let name = env!("CARGO_PKG_NAME");
        let version = env!("CARGO_PKG_VERSION");
        let informer = update_informer::new(registry::Crates, name, version)
            .interval(Duration::from_secs(60 * 60 * 24));

        if let Some(version) = informer.check_version().ok().flatten() {
            println!("\nNew version is available! : {}", version);
        }
    }

    if let Some(exit_value) = exit_value {
        return ExitCode::from(exit_value);
    }

    ExitCode::SUCCESS
}

fn exec_search(query: Vec<String>, config: &mut Config, default_config: &Config, no_color: bool) -> u8 {
    if config.debug {
        dbg!(&query);
    }

    let mut parser = Parser::new();
    let query = parser.parse(query, config.debug);

    if config.debug {
        dbg!(&query);
    }

    if parser.there_are_remaining_lexemes() {
        error_exit("query", "could not parse tokens at the end of the query");
    }

    match query {
        Ok(query) => {
            let is_terminal = stdout().is_terminal();
            let use_colors = !no_color && is_terminal;

            let mut searcher = Searcher::new(&query, config, default_config, use_colors);
            searcher.list_search_results().unwrap();

            let error_count = searcher.error_count;
            match error_count {
                0 => 0,
                _ => 1,
            }
        }
        Err(err) => {
            error_message("query", &err);
            2
        }
    }
}

fn short_usage_info(no_color: bool) {
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    print!("fselect ");

    if no_color {
        println!("{}", VERSION);
    } else {
        println!("{}", Yellow.paint(VERSION));
    }

    println!("Find files with SQL-like queries.");

    if no_color {
        println!("https://github.com/jhspetersson/fselect");
    } else {
        println!(
            "{}",
            Cyan.underline()
                .paint("https://github.com/jhspetersson/fselect")
        );
    }

    println!();
    println!("Usage: fselect [ARGS] COLUMN[, COLUMN...] [from PATH[, PATH...]] [where EXPR] [group by COLUMN, ...] [order by COLUMN (asc|desc), ...] [limit N] [into FORMAT]");
}

fn help_hint() {
    println!(
        "
For more detailed instructions please refer to the URL above or run fselect --help"
    );
}

fn usage_info(config: Config, default_config: Config, no_color: bool) {
    short_usage_info(no_color);

    let is_archive = config
        .is_archive
        .unwrap_or(default_config.is_archive.unwrap())
        .join(", ");
    let is_audio = config
        .is_audio
        .unwrap_or(default_config.is_audio.unwrap())
        .join(", ");
    let is_book = config
        .is_book
        .unwrap_or(default_config.is_book.unwrap())
        .join(", ");
    let is_doc = config
        .is_doc
        .unwrap_or(default_config.is_doc.unwrap())
        .join(", ");
    let is_font = config
        .is_font
        .unwrap_or(default_config.is_font.unwrap())
        .join(", ");
    let is_image = config
        .is_image
        .unwrap_or(default_config.is_image.unwrap())
        .join(", ");
    let is_source = config
        .is_source
        .unwrap_or(default_config.is_source.unwrap())
        .join(", ");
    let is_video = config
        .is_video
        .unwrap_or(default_config.is_video.unwrap())
        .join(", ");

    println!("
Files Detected as Archives: {is_archive}
Files Detected as Audio: {is_audio}
Files Detected as Book: {is_book}
Files Detected as Document: {is_doc}
Files Detected as Fonts: {is_font}
Files Detected as Image: {is_image}
Files Detected as Source Code: {is_source}
Files Detected as Video: {is_video}

Path Options:
    {}

Regex syntax:
    {}

Column Options:
    {}

Functions:
    {}

Expressions:
    Operators:
        = | == | eq                 Used to check for equality between the column field and value
        === | eeq                   Used to check for strict equality between column field and value irregardless of any special regex characters
        != | <> | ne                Used to check for inequality between column field and value
        !== | ene                   Used to check for inequality between column field and value irregardless of any special regex characters
        < | lt                      Used to check whether the column value is less than the value
        <= | lte | le               Used to check whether the column value is less than or equal to the value
        > | gt                      Used to check whether the column value is greater than the value
        >= | gte | ge               Used to check whether the column value is greater than or equal to the value
        ~= | =~ | regexp | rx       Used to check if the column value matches the regex pattern
        !=~ | !~= | notrx           Used to check if the column value doesn't match the regex pattern
        like                        Used to check if the column value matches the pattern which follows SQL conventions
        notlike                     Used to check if the column value doesn't match the pattern which follows SQL conventions
        between                     Used to check if the column value lies between two values inclusive
        in                          Used to check if the column value is in the list of values
    Logical Operators:
        and                         Used as an AND operator for two conditions made with the above operators
        or                          Used as an OR operator for two conditions made with the above operators

Format:
    tabs (default)                  Outputs each file with its column value(s) on a line with each column value delimited by a tab
    lines                           Outputs each column value on a new line
    list                            Outputs entire output onto a single line for xargs
    csv                             Outputs each file with its column value(s) on a line with each column value delimited by a comma
    json                            Outputs a JSON array with JSON objects holding the column value(s) of each file
    html                            Outputs HTML document with table
    ", format_root_options(), 
        Cyan.underline().paint("https://docs.rs/regex/1.10.2/regex/#syntax"),
        format_field_usage(),
        format_function_usage(),
    );
}

fn format_root_options() -> String {
    RootOptions::get_names_and_descriptions().iter()
        .map(|(names, description)| names.join(" | ").to_string() + " ".repeat(32 - names.join(" | ").to_string().len()).as_str() + description)
        .collect::<Vec<_>>().join("\n    ")
}

fn format_field_usage() -> String {
    Field::get_names_and_descriptions().iter()
        .map(|(names, description)| names.join(" | ").to_string() + " ".repeat(if 32 > names.join(" | ").to_string().len() { 32 - names.join(" | ").to_string().len() } else { 1 }).as_str() + description)
        .collect::<Vec<_>>().join("\n    ")
}

fn format_function_usage() -> String {
    let funcs = Function::get_names_and_descriptions();
    Function::get_groups().iter()
        .filter(|group| funcs.get(*group).is_some())
        .map(|group| {
            let funcs_in_group = funcs.get(*group).unwrap();
            format!(
                "{}:\n        {}",
                group,
                funcs_in_group
                    .iter()
                    .map(|(names, description)| {
                        names.join(" | ").to_string().to_uppercase() + " ".repeat(if 28 > names.join(" | ").to_string().len() { 28 - names.join(" | ").to_string().len() } else { 1 }).as_str() + description
                    })
                    .collect::<Vec<_>>()
                    .join("\n        ")
            )
        })
        .collect::<Vec<_>>().join("\n\n    ")
}

fn complete_fields_info() {
    println!(
        "{}",
        Field::get_names_and_descriptions()
            .iter()
            .map(|(names, _)| names.join(" "))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn complete_functions_info() {
    println!(
        "{}",
        Function::get_names_and_descriptions()
            .iter()
            .flat_map(|entry| entry.1.iter())
            .map(|(names, _)| names.join(" ").to_uppercase())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn complete_root_options_info() {
    println!(
        "{}",
        RootOptions::get_names_and_descriptions()
            .iter()
            .map(|(names, _)| names.join(" "))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
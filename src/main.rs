//! The entry point of the program
//! Handles the command line arguments parsing

#[macro_use]
extern crate serde_derive;
#[cfg(all(unix, feature = "users"))]
extern crate uzers;
#[cfg(unix)]
extern crate xattr;

use std::{env, fs};
use std::io::{stdout, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
#[cfg(feature = "update-notifications")]
use std::time::Duration;

use nu_ansi_term::Color::*;
#[cfg(feature = "interactive")]
use rustyline::error::ReadlineError;
#[cfg(feature = "interactive")]
use rustyline::DefaultEditor;
#[cfg(feature = "update-notifications")]
use update_informer::{registry, Check};

use crate::config::Config;
use crate::field::Field;
use crate::function::Function;
use crate::lexer::Lexer;
use crate::output::OutputFormat;
use crate::parser::Parser;
use crate::query::RootOptions;
use crate::searcher::Searcher;
use crate::util::{set_us_dates, str_to_bool};
use crate::util::error::{error_message, get_no_errors, set_no_errors, set_use_colors};

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
            Config::default()
        }
    };

    let env_no_color = std::env::var("NO_COLOR").is_ok();
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

    if first_arg.starts_with("--output-formats") {
        complete_output_formats_info();
        return ExitCode::SUCCESS;
    }

    #[allow(unused_mut)]
    let mut interactive = false;

    loop {
        if first_arg.contains("nocolor") || first_arg.contains("no-color") {
            no_color = true;
        } else if first_arg.starts_with("-i")
            || first_arg.starts_with("--i")
            || first_arg.starts_with("/i")
        {
            #[cfg(feature = "interactive")]
            { interactive = true; }
        } else if first_arg.starts_with("-c")
            || first_arg.starts_with("--config")
            || first_arg.starts_with("/c")
        {
            if args.len() < 2 {
                eprintln!("Error: --config requires a path argument");
                return ExitCode::from(2);
            }

            let config_path = args[1].clone();
            config = Config::from(PathBuf::from(&config_path)).unwrap_or_else(|err| {
                eprintln!("{}", err);
                Config::default()
            });

            args.remove(0);
        } else if first_arg.starts_with("--no-error") {
            set_no_errors(true);
        } else if first_arg.starts_with("--us-date") { 
            set_us_dates(true);
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
    
    set_use_colors(!no_color);

    if config.us_dates.unwrap_or(default_config.us_dates.unwrap()) {
        set_us_dates(true);
    }

    let mut exit_value = None::<u8>;

    #[cfg(feature = "interactive")]
    if interactive {
        match DefaultEditor::new() {
            Ok(mut rl) => {
                if let Some(project_dir) = crate::util::app_dirs::get_project_dir() {
                    let _ = fs::create_dir_all(&project_dir);
                    let history_file = project_dir.join("history.txt");
                    let _ = rl.load_history(history_file.as_path());
                }

                loop {
                    let readline = rl.readline("query> ");
                    match readline {
                        Ok(cmd) => {
                            let trimmed = cmd.trim().to_ascii_lowercase();
                            if trimmed == "quit" || trimmed == "exit" {
                                break;
                            } else if trimmed == "help" {
                                usage_info(config.clone(), default_config.clone(), no_color);
                            } else if trimmed == "pwd" {
                                match env::current_dir() {
                                    Ok(path) => println!("{}", path.to_string_lossy()),
                                    Err(err) => error_message("pwd", &err.to_string()),
                                }
                            } else if trimmed == "cd" || trimmed.starts_with("cd ") {
                                let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
                                if parts.len() < 2 {
                                    error_message("cd", "no path specified");
                                } else {
                                    let _ = rl.add_history_entry(&cmd);
                                    let new_path: String = parts.iter().skip(1).cloned().collect::<Vec<&str>>().join(" ");
                                    match env::set_current_dir(new_path) {
                                        Ok(()) => {}
                                        Err(err) => error_message("cd", &err.to_string()),
                                    }
                                }
                            } else if trimmed == "errors" || trimmed.starts_with("errors ") {
                                let _ = rl.add_history_entry(&cmd);
                                let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
                                if parts.len() == 2 {
                                    let no_errors = !str_to_bool(&parts[1]).unwrap_or(true);
                                    set_no_errors(no_errors);
                                }
                                println!("Errors are {}", if no_color {
                                    (if get_no_errors() { "OFF" } else { "ON" }).into()
                                } else {
                                    Yellow.paint(if get_no_errors() { "OFF" } else { "ON" })
                                });
                            } else if trimmed == "debug" || trimmed.starts_with("debug ") {
                                let _ = rl.add_history_entry(&cmd);
                                let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
                                if parts.len() == 2 {
                                    config.debug = str_to_bool(&parts[1]).unwrap_or(false);
                                }
                                if no_color {
                                    println!("DEBUG IS {}", (if config.debug { "ON" } else { "OFF" }));
                                } else {
                                    println!("DEBUG IS {}", Yellow.paint(if config.debug { "ON" } else { "OFF" }));
                                }
                            } else {
                                let _ = rl.add_history_entry(&cmd);
                                exec_search(vec![cmd], &mut config, &default_config, no_color);
                            }
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
                }

                if let Some(project_dir) = crate::util::app_dirs::get_project_dir() {
                    let _ = fs::create_dir_all(&project_dir);
                    let history_file = project_dir.join("history.txt");
                    let _ = rl.save_history(history_file.as_path());
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

    #[cfg(not(feature = "interactive"))]
    {
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

    let mut lexer = Lexer::new(query);
    let mut parser = Parser::new(&mut lexer);
    let query = parser.parse(config.debug);

    if config.debug {
        dbg!(&query);
    }

    if parser.there_are_remaining_lexemes() {
        error_message("query", "could not parse tokens at the end of the query");
        return 2;
    }

    match query {
        Ok(query) => {
            let is_terminal = stdout().is_terminal();
            let use_colors = !no_color && is_terminal && query.output_format.supports_colorization();

            let mut searcher = Searcher::new(&query, config, default_config, use_colors);
            if let Err(mut err) = searcher.list_search_results() {
                if err.source.is_empty() {
                    err.source = "result".to_string();
                }
                err.print();
            }

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
    println!("Usage: fselect [ARGS] COLUMN[, COLUMN...] [from PATH[, PATH...]] [where EXPR] [group by COLUMN, ...] [order by COLUMN (asc|desc), ...] [limit N] [offset N] [into FORMAT]");
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
        exists                      Used to check if there is results in the subquery (optionally bound with the main query)
    Logical Operators:
        and                         Used as an AND operator for two conditions made with the above operators
        or                          Used as an OR operator for two conditions made with the above operators

Format:
    {}

Interactive mode:
    help            Get usage help
    pwd             Print the current working directory
    cd PATH         Change the current working directory to PATH
    errors [ON|OFF] Toggle whether errors are shown or not
    exit | quit     Exit fselect
    ", format_root_options(), 
        if no_color { "https://docs.rs/regex/1.10.2/regex/#syntax".into() } else { Cyan.underline().paint("https://docs.rs/regex/1.10.2/regex/#syntax") },
        format_field_usage(),
        format_function_usage(),
        format_output_usage()
    );
}

fn format_root_options() -> String {
    RootOptions::get_names_and_descriptions().iter()
        .map(|(names, description)| {
            let joined = names.join(" | ");
            let pad = if 32 > joined.len() { 32 - joined.len() } else { 1 };
            joined + &" ".repeat(pad) + description
        })
        .collect::<Vec<_>>().join("\n    ")
}

fn format_field_usage() -> String {
    Field::get_names_and_descriptions().iter()
        .map(|(names, description)| {
            let joined = names.join(" | ");
            let pad = if 32 > joined.len() { 32 - joined.len() } else { 1 };
            joined + &" ".repeat(pad) + description
        })
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
                        let joined = names.join(" | ");
                        let pad = if 28 > joined.len() { 28 - joined.len() } else { 1 };
                        joined.to_uppercase() + &" ".repeat(pad) + description
                    })
                    .collect::<Vec<_>>()
                    .join("\n        ")
            )
        })
        .collect::<Vec<_>>().join("\n\n    ")
}

fn format_output_usage() -> String {
    OutputFormat::get_names_and_descriptions().iter()
        .map(|(name, description)| {
            let name = name.to_string();
            let pad = if 32 > name.len() { 32 - name.len() } else { 1 };
            name + &" ".repeat(pad) + description
        })
        .collect::<Vec<_>>().join("\n    ")
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
            .join(" ")
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

fn complete_output_formats_info() {
    println!(
        "{}",
        OutputFormat::get_names_and_descriptions()
            .iter()
            .map(|entry| entry.0.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_repl_command_matching() {
        // Verify that the REPL command matching logic doesn't match partial words
        let trimmed = "cdata";
        assert!(!(trimmed == "cd" || trimmed.starts_with("cd ")));

        let trimmed = "cd /tmp";
        assert!(trimmed == "cd" || trimmed.starts_with("cd "));

        let trimmed = "errors_table";
        assert!(!(trimmed == "errors" || trimmed.starts_with("errors ")));

        let trimmed = "debugger";
        assert!(!(trimmed == "debug" || trimmed.starts_with("debug ")));

        let trimmed = "debug true";
        assert!(trimmed == "debug" || trimmed.starts_with("debug "));
    }
}
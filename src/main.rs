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
use crate::parser::Parser;
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
    mindepth N 	                    Minimum search depth. Default is unlimited. Depth 1 means skip one directory level and search further.
    maxdepth N | depth N 	        Maximum search depth. Default is unlimited. Depth 1 means search the mentioned directory only. Depth 2 means search mentioned directory and its subdirectories.
    symlinks | sym                  If specified, search process will follow symlinks. Default is not to follow.
    hardlinks | hard                If specified, search process will track hardlinks. Default is not to track.
    archives | arc                  Search within archives. Only zip archives are supported. Default is not to include archived content into the search results.
    gitignore | git                 Search respects .gitignore files found.
    hgignore | hg                   Search respects .hgignore files found.
    dockerignore | docker           Search respects .dockerignore files found.
    nogitignore | nogit             Disable .gitignore parsing during the search.
    nohgignore | nohg               Disable .hgignore parsing during the search.
    nodockerignore | nodocker       Disable .dockerignore parsing during the search.
    dfs 	                        Depth-first search mode.
    bfs 	                        Breadth-first search mode. This is the default.
    regexp | rx                     Use regular expressions to search within multiple roots.

Regex syntax:
    {}

Column Options:
    {}

Functions:
    Aggregate:
        AVG                         Returns average of all values
        COUNT                       Returns number of all values
        MAX                         Returns maximum value
        MIN                         Returns minimum value
        SUM                         Returns sum of all values
        STDDEV_POP | STDDEV | STD   Population standard deviation, the square root of variance
        STDDEV_SAMP                 Sample standard deviation, the square root of sample variance
        VAR_POP | VARIANCE          Population variance
        VAR_SAMP                    Sample variance
    Date:
        CURRENT_DATE | CUR_DATE |
        CURDATE                     Returns current date
        DAY                         Returns day of the month
        MONTH                       Returns month of the year
        YEAR                        Returns year of the date
        DOW | DAYOFWEEK             Returns day of the week (1 - Sunday, 2 - Monday, etc.)
    User:
        CURRENT_USER                Returns the current username (unix-only)
        CURRENT_UID                 Returns the current real UID (unix-only)
        CURRENT_GROUP               Returns the current primary groupname (unix-only)
        CURRENT_GID                 Returns the current primary GID (unix-only)
    Xattr:
        HAS_XATTR                   Used to check if xattr exists (unix-only)
        XATTR                       Returns value of xattr (unix-only)
        HAS_CAPABILITIES | HAS_CAPS Check if any Linux capability exists for the file
        HAS_CAPABILITY or HAS_CAP   Check if given Linux capability exists for the file
    String:
        LENGTH | LEN                Returns length of string value
        LOWER | LOWERCASE | LCASE   Returns lowercase value
        UPPER | UPPERCASE | UCASE   Returns uppercase value
        INITCAP                     Returns first letter of each word uppercase, all other letters lowercase
        TO_BASE64 | BASE64          Returns Base64 digest of a value
        FROM_BASE64                 Returns decoded value from a Base64 digest
        LOCATE | POSITION           Returns position of the substring in the string
        SUBSTRING | SUBSTR          Returns part of the string value
        REPLACE                     Returns string with substring replaced with another one
        TRIM                        Returns string with whitespaces at the beginning and the end stripped
        LTRIM                       Returns string with whitespaces at the beginning stripped
        RTRIM                       Returns string with whitespaces at the end stripped
    Japanese string:
        CONTAINS_JAPANESE           Used to check if string value contains Japanese symbols
        CONTAINS_KANA               Used to check if string value contains kana symbols
        CONTAINS_HIRAGANA           Used to check if string value contains hiragana symbols
        CONTAINS_KATAKANA           Used to check if string value contains katakana symbols
        CONTAINS_KANJI              Used to check if string value contains kanji symbols
    Other:
        BIN                         Returns binary representation of an integer value
        HEX                         Returns hexadecimal representation of an integer value
        OCT                         Returns octal representation of an integer value
        ABS                         Returns absolute value of the number
        POWER | POW                 Raise the value to the specified power
        SQRT                        Returns square root of the value
        LOG                         Returns logarithm of the value
        LN                          Returns natural logarithm of the value
        EXP                         Returns e raised to the power of the value
        LEAST                       Returns the smallest value
        GREATEST                    Returns the largest value
        CONTAINS                    Returns true, if file contains string, false if not
        COALESCE                    Returns first nonempty expression value
        CONCAT                      Returns concatenated string of expression values
        CONCAT_WS                   Returns concatenated string of expression values with specified delimiter
        FORMAT_SIZE                 Returns formatted size of a file
        FORMAT_TIME | PRETTY_TIME   Returns human-readable durations of time in seconds
        RANDOM | RAND               Returns random integer (from zero to max int, from zero to arg, or from arg1 to arg2)

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
    ", Cyan.underline().paint("https://docs.rs/regex/1.10.2/regex/#syntax"),
        Field::get_names_and_descriptions().iter()
            .map(|(names, description)| names.join(" | ").to_string() + " ".repeat(if 32 > names.join(" | ").to_string().len() { 32 - names.join(" | ").to_string().len() } else { 1 }).as_str() + description)
            .collect::<Vec<_>>().join("\n    ")
    );
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
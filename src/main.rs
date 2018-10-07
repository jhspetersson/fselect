extern crate chrono;
extern crate chrono_english;
extern crate csv;
extern crate humansize;
extern crate imagesize;
#[macro_use]
extern crate lazy_static;
extern crate mp3_metadata;
extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate term;
extern crate time;
#[cfg(unix)]
extern crate users;
#[cfg(unix)]
extern crate xattr;
extern crate zip;

use std::env;

use term::StdoutTerminal;

mod field;
mod fileinfo;
mod gitignore;
mod lexer;
mod mode;
mod parser;
mod searcher;
mod util;

use parser::Parser;
use searcher::Searcher;
use util::error_message;

fn main() {
    let mut t = term::stdout().unwrap();

    if env::args().len() == 1 {
        usage_info(&mut t);
        return;
    }

    let mut args: Vec<String> = env::args().collect();
    args.remove(0);

    let first_arg = args[0].to_ascii_lowercase();
    if first_arg.contains("help") || first_arg.contains("-h") || first_arg.contains("/?") {
        usage_info(&mut t);
        return;
    }

    let query = args.join(" ");

    let mut p = Parser::new();
    let query = p.parse(&query);

    match query {
        Ok(query) => {
            let mut searcher = Searcher::new(query);
            searcher.list_search_results(&mut t).unwrap()
        },
        Err(err) => error_message("query", &err, &mut t)
    }
}

fn usage_info(t: &mut Box<StdoutTerminal>) {
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    print!("FSelect utility");
    t.fg(term::color::BRIGHT_YELLOW).unwrap();
    println!(" {}", VERSION);
    t.reset().unwrap();

    println!("Find files with SQL-like queries.");

    t.fg(term::color::BRIGHT_CYAN).unwrap();
    println!("https://github.com/jhspetersson/fselect");
    t.reset().unwrap();

    println!("
Usage: fselect COLUMN[, COLUMN...] [from PATH[, PATH...]] [where EXPR] [order by COLUMN (asc|desc), ...] [limit N] [into FORMAT]

Files Detected as Audio: .aac, .aiff, .amr, .flac, .gsm, .m4a, .m4b, .m4p, .mp3, .ogg, .wav, .wma
Files Detected as Archives: .7z, .bzip2, .gz, .gzip, .rar, .tar, .xz, .zip
Files Detected as Book: .azw3, .chm, .epub, .fb2, .mobi, .pdf
Files Detected as Document: .accdb, .doc, .docm, .docx, .dot, .dotm, .dotx, .mdb, .ods, .odt, .pdf, .potm, .potx, .ppt, .pptm, .pptx, .rtf, .xlm, .xls, .xlsm, .xlsx, .xlt, .xltm, .xltx, .xps
Files Detected as Image: .bmp, .gif, .jpeg, .jpg, .png, .webp
Files Detected as Source Code: .asm, .c, .cpp, .cs, .go, .h, .hpp, .java, .js, .jsp, .pas, .php, .pl, .pm, .py, .rb, .rs, .swift
Files Detected as Video: .3gp, .avi, .flv, .m4p, .m4v, .mkv, .mov, .mp4, .mpeg, .mpg, .webm, .wmv

Column Options:
        name                            Returns the name of the file
        path                            Returns the path of the file
        size                            Returns the size of the file in bytes
        fsize                           Returns the size of the file accompanied with the unit
        uid                             Returns the UID of the owner
        gid                             Returns the GID of the owner's group

        accessed                        Returns the time the file was last accessed (YYYY-MM-DD HH:MM:SS)
        created                         Returns the file creation date (YYYY-MM-DD HH:MM:SS)
        modified                        Returns the time the file was last modified (YYYY-MM-DD HH:MM:SS)

        is_dir                          Returns a boolean signifying whether the file path is a directory
        is_file                         Returns a boolean signifying whether the file path is a file
        is_symlink                      Returns a boolean signifying whether the file path is a symlink
        is_pipe | is_fifo               Returns a boolean signifying whether the file path is a FIFO or pipe file
        is_char | is_character          Returns a boolean signifying whether the file path is a character device or character special file
        is_block                        Returns a boolean signifying whether the file path is a block or block special file
        is_socket                       Returns a boolean signifying whether the file path is a socket file
        is_hidden                       Returns a boolean signifying whether the file is a hidden file (files that start with a dot)
        has_xattrs                      Returns a boolean signifying whether the file has extended attributes

        mode                            Returns the permissions of the owner, group, and everybody (similar to the first field in `ls -la`)

        user                            Returns the name of the owner for this file
        user_read                       Returns a boolean signifying whether the file can be read by the owner
        user_write                      Returns a boolean signifying whether the file can be written by the owner
        user_exec                       Returns a boolean signifying whether the file can be executed by the owner

        group                           Returns the name of the owner's group for this file
        group_read                      Returns a boolean signifying whether the file can be read by the owner's group
        group_write                     Returns a boolean signifying whether the file can be written by the owner's group
        group_exec                      Returns a boolean signifying whether the file can be executed by the owner's group

        other_read                      Returns a boolean signifying whether the file can be read by others
        other_write                     Returns a boolean signifying whether the file can be written by others
        other_exec                      Returns a boolean signifying whether the file can be executed by others

        title                           Returns the title of the audio file taken from the file's metadata
        album                           Returns the album name of the audio file taken from the file's metadata
        artist                          Returns the artist of the audio file taken from the file's metadata
        genre                           Returns the genre of the audio file taken from the file's metadata
        year                            Returns the year of the audio file taken from the file's metadata

        freq                            Returns the sampling rate of audio or video file
        bitrate                         Returns the bitrate of the audio file in kbps
        width                           Returns the number of pixels along the width of the photo
        height                          Returns the number of pixels along the height of the photo

        is_shebang                      Returns a boolean signifying whether the file starts with a shebang (#!)
        is_archive                      Returns a boolean signifying whether the file is an archival file
        is_audio                        Returns a boolean signifying whether the file is an audio file
        is_book                         Returns a boolean signifying whether the file is a book
        is_doc                          Returns a boolean signifying whether the file is a document
        is_image                        Returns a boolean signifying whether the file is an image
        is_source                       Returns a boolean signifying whether the file is source code
        is_video                        Returns a boolean signifying whether the file is a video file

Expressions:
    Operators:
        = | == | eq                     Used to check for equality between the column field and value
        ===                             Used to check for strict equality between column field and value irregardless of any special regex characters
        != | <> | ne                    Used to check for inequality between column field and value
        !==                             Used to check for inequality between column field and value irregardless of any special regex characters
        < | lt                          Used to check whether the column value is less than the value
        <= | lte                        Used to check whether the column value is less than or equal to the value
        > | gt                          Used to check whether the column value is greater than the value
        >= | gte                        Used to check whether the column value is greater than or equal to the value
        ~= | =~ | regexp | rx           Used to check if the column value matches the regex pattern
        like                            Used to check if the column value matches the pattern which follows SQL conventions
    Logical Operators:
        and                             Used as an AND operator for two conditions made with the above operators
        or                              Used as an OR operator for two conditions made with the above operators

Format:
        tabs (default)                  Outputs each file with its column value(s) on a line with each column value delimited by a tab
        lines                           Outputs each column value on a new line
        list                            Outputs entire output onto a single line for xargs
        csv                             Outputs each file with its column value(s) on a line with each column value delimited by a comma
        json                            Outputs a JSON array with JSON objects holding the column value(s) of each file
    ");
}

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate text_io;
#[cfg(all(unix, feature = "users"))]
extern crate users;
#[cfg(unix)]
extern crate xattr;

use std::env;
use std::io::Write;

use ansi_term::Colour::*;
use atty::Stream;

mod config;
mod expr;
mod field;
mod fileinfo;
mod function;
mod ignore;
mod lexer;
mod mode;
mod operators;
mod parser;
mod query;
mod searcher;
mod util;

use crate::config::Config;
use crate::parser::Parser;
use crate::searcher::Searcher;
use crate::util::error_message;

fn main() {
    let config = match Config::new() {
        Ok(cnf) => cnf,
        Err(err) => {
            eprintln!("{}", err);
            Config::default()
        }
    };

    let env_no_color = std::env::var("NO_COLOR").ok().eq(&Some("1".to_string()));
    let mut no_color = env_no_color || (config.no_color.is_some() && config.no_color.unwrap());

    #[cfg(windows)]
    {
        if !no_color {
            let res = ansi_term::enable_ansi_support();
            let win_init_ok = match res {
                Ok(()) => true,
                Err(203) => true,
                _ => false
            };
            no_color = !win_init_ok;
        }
    }

    if env::args().len() == 1 {
        short_usage_info(no_color);
        help_hint();
        return;
    }

    let mut args: Vec<String> = env::args().collect();
    args.remove(0);

    let first_arg = args[0].to_ascii_lowercase();

    if first_arg.contains("help") || first_arg.contains("-h") || first_arg.contains("/?") || first_arg.contains("/h") {
        usage_info(no_color);
        return;
    }

    if first_arg.contains("nocolor") || first_arg.contains("no-color") {
        args.remove(0);
        no_color = true;
    }

    let first_arg = args[0].to_ascii_lowercase();
    let query = if first_arg.starts_with("-i") {
        print!("query> ");
        std::io::stdout().flush().unwrap();

        let input: String = read!("{}\n");
        input.trim_end().to_string()
    } else {
        args.join(" ")
    };

    let mut p = Parser::new();
    let query = p.parse(&query);

    match query {
        Ok(query) => {
            let is_terminal = atty::is(Stream::Stdout);
            let use_colors = !no_color && is_terminal;

            let mut searcher = Searcher::new(query, config.clone(), use_colors);
            searcher.list_search_results().unwrap()
        },
        Err(err) => error_message("query", &err)
    }

    config.save();
}

fn short_usage_info(no_color: bool) {
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

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
        println!("{}", Cyan.underline().paint("https://github.com/jhspetersson/fselect"));
    }

    println!();
    println!("Usage: fselect [ARGS] COLUMN[, COLUMN...] [from PATH[, PATH...]] [where EXPR] [order by COLUMN (asc|desc), ...] [limit N] [into FORMAT]");
}

fn help_hint() {
    println!("
For more detailed instructions please refer to the URL above or run fselect --help");
}

fn usage_info(no_color: bool) {
    short_usage_info(no_color);

    println!("

Files Detected as Archives: .7z, .bz2, .bzip2, .gz, .gzip, .lz, .rar, .tar, .xz, .zip
Files Detected as Audio: .aac, .aiff, .amr, .flac, .gsm, .m4a, .m4b, .m4p, .mp3, .ogg, .wav, .wma
Files Detected as Book: .azw3, .chm, .djvu, .epub, .fb2, .mobi, .pdf
Files Detected as Document: .accdb, .doc, .docm, .docx, .dot, .dotm, .dotx, .mdb, .ods, .odt, .pdf, .potm, .potx, .ppt, .pptm, .pptx, .rtf, .xlm, .xls, .xlsm, .xlsx, .xlt, .xltm, .xltx, .xps
Files Detected as Image: .bmp, .gif, .heic, .jpeg, .jpg, .png, .psb, .psd, .tiff, .webp
Files Detected as Source Code: .asm, .bas, .c, .cc, .ceylon, .clj, .coffee, .cpp, .cs, .d, .dart, .elm, .erl, .go, .groovy, .h, .hh, .hpp, .java, .js, .jsp, .kt, .kts, .lua, .nim, .pas, .php, .pl, .pm, .py, .rb, .rs, .scala, .swift, .tcl, .vala, .vb
Files Detected as Video: .3gp, .avi, .flv, .m4p, .m4v, .mkv, .mov, .mp4, .mpeg, .mpg, .webm, .wmv

Column Options:
    name                            Returns the name of the file
    path                            Returns the path of the file
    abspath                         Returns the absolute path of the file
    size                            Returns the size of the file in bytes
    fsize | hsize                   Returns the size of the file accompanied with the unit
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
    is_hidden                       Returns a boolean signifying whether the file is a hidden file (e.g., files that start with a dot on *nix)
    has_xattrs                      Returns a boolean signifying whether the file has extended attributes

    device (Linux only)             Returns the code of device the file is stored on
    inode (Linux only)              Returns the number of inode
    blocks (Linux only)             Returns the number of blocks (256 bytes) the file occupies
    hardlinks (Linux only)          Returns the number of hardlinks of the file

    mode                            Returns the permissions of the owner, group, and everybody (similar to the first field in `ls -la`)

    user                            Returns the name of the owner for this file
    user_read                       Returns a boolean signifying whether the file can be read by the owner
    user_write                      Returns a boolean signifying whether the file can be written by the owner
    user_exec                       Returns a boolean signifying whether the file can be executed by the owner
    user_all                        Returns a boolean signifying whether the file can be fully accecced by the owner

    group                           Returns the name of the owner's group for this file
    group_read                      Returns a boolean signifying whether the file can be read by the owner's group
    group_write                     Returns a boolean signifying whether the file can be written by the owner's group
    group_exec                      Returns a boolean signifying whether the file can be executed by the owner's group
    group_all                       Returns a boolean signifying whether the file can be fully accecced by the group

    other_read                      Returns a boolean signifying whether the file can be read by others
    other_write                     Returns a boolean signifying whether the file can be written by others
    other_exec                      Returns a boolean signifying whether the file can be executed by others
    other_all                       Returns a boolean signifying whether the file can be fully accecced by the others

    suid                            Returns a boolean signifying whether the file permissions have a SUID bit set
    sgid                            Returns a boolean signifying whether the file permissions have a SGID bit set

    width                           Returns the number of pixels along the width of the photo or MP4 file
    height                          Returns the number of pixels along the height of the photo or MP4 file

    mime                            Returns MIME type of the file
    is_binary                       Returns a boolean signifying whether the file has binary contents
    is_text                         Returns a boolean signifying whether the file has text contents
    line_count                      Returns a number of lines in a text file

    exif_datetime                   Returns date and time of taken photo
    exif_altitude | exif_alt        Returns GPS altitude of taken photo
    exif_latitude | exif_lat        Returns GPS latitude of taken photo
    exif_longitude | exif_lng       Returns GPS longitude of taken photo
    exif_make                       Returns name of the camera manufacturer
    exif_model                      Returns camera model
    exif_software                   Returns software name with which the photo was taken
    exif_version                    Returns the version of EXIF metadata

    mp3_title | title               Returns the title of the audio file taken from the file's metadata
    mp3_album | album               Returns the album name of the audio file taken from the file's metadata
    mp3_artist | artist             Returns the artist of the audio file taken from the file's metadata
    mp3_genre | genre               Returns the genre of the audio file taken from the file's metadata
    mp3_year                        Returns the year of the audio file taken from the file's metadata
    mp3_freq | freq                 Returns the sampling rate of audio or video file
    mp3_bitrate | bitrate           Returns the bitrate of the audio file in kbps
    duration                        Returns the duration of audio file in seconds

    is_shebang                      Returns a boolean signifying whether the file starts with a shebang (#!)
    is_archive                      Returns a boolean signifying whether the file is an archival file
    is_audio                        Returns a boolean signifying whether the file is an audio file
    is_book                         Returns a boolean signifying whether the file is a book
    is_doc                          Returns a boolean signifying whether the file is a document
    is_image                        Returns a boolean signifying whether the file is an image
    is_source                       Returns a boolean signifying whether the file is source code
    is_video                        Returns a boolean signifying whether the file is a video file

    sha1                            Returns SHA-1 digest of a file
    sha2_256 | sha256               Returns SHA2-256 digest of a file
    sha2_512 | sha512               Returns SHA2-512 digest of a file
    sha3_512 | sha3                 Returns SHA-3 digest of a file

Functions:
    Aggregate:
        AVG                         Returns average of all values
        COUNT                       Returns number of all values
        MAX                         Returns maximum value
        MIN                         Returns minimum value
        SUM                         Returns sum of all values
    Date:
        DAY                         Returns day of the month
        MONTH                       Returns month of the year
        YEAR                        Returns year of the date
    Xattr:
        HAS_XATTR                   Used to check if xattr exists
        XATTR                       Returns value of xattr
    String:
        LENGTH | LEN                Returns length of string value
        LOWER | LCASE               Returns lowercase value
        UPPER | UCASE               Returns uppercase value
        BASE64                      Returns Base64 digest of a value
        SUBSTRING | SUBSTR          Returns part of the string value
        REPLACE                     Returns string with substring replaced with another one
        TRIM                        Returns string with whitespaces at the beginning and the end stripped
    Japanese string:
        CONTAINS_JAPANESE           Used to check if string value contains Japanese symbols
        CONTAINS_KANA               Used to check if string value contains kana symbols
        CONTAINS_HIRAGANA           Used to check if string value contains hiragana symbols
        CONTAINS_KATAKANA           Used to check if string value contains katakana symbols
        CONTAINS_KANJI              Used to check if string value contains kanji symbols
    Other:
        HEX                         Returns hexadecimal representation of an integer value
        OCT                         Returns octal representation of an integer value
        CONTAINS                    Returns true, if file contains string, false if not
        COALESCE                    Returns first nonempty expression value
        CONCAT                      Returns concatenated string of expression values
        CONCAT_WS                   Returns concatenated string of expression values with specified delimiter
        FORMAT_SIZE                 Returns file size formatted in specified units

Expressions:
    Operators:
        = | == | eq                 Used to check for equality between the column field and value
        ===                         Used to check for strict equality between column field and value irregardless of any special regex characters
        != | <> | ne                Used to check for inequality between column field and value
        !==                         Used to check for inequality between column field and value irregardless of any special regex characters
        < | lt                      Used to check whether the column value is less than the value
        <= | lte | le               Used to check whether the column value is less than or equal to the value
        > | gt                      Used to check whether the column value is greater than the value
        >= | gte | ge               Used to check whether the column value is greater than or equal to the value
        ~= | =~ | regexp | rx       Used to check if the column value matches the regex pattern
        !=~ | !~= | notrx           Used to check if the column value doesn't match the regex pattern
        like                        Used to check if the column value matches the pattern which follows SQL conventions
        notlike                     Used to check if the column value doesn't match the pattern which follows SQL conventions
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
    ");
}

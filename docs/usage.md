# fselect

Find files with SQL-like queries

### Basic usage

    fselect [ARGS] COLUMN[, COLUMN...] [from ROOT[, ROOT...]] [where EXPR] [group by COLUMNS] [order by COLUMNS] [limit N] [into FORMAT]

You write SQL-like query, that's it.

**fselect** command itself is like a first keyword (`select`, i.e., *file select*).
But if you'll put one more `select` behind occasionally, that's not a problem.

Next you put columns you are interested in. It could be file name or path, size, modification date, etc.
See full list of possible columns. You can add columns with arbitrary text (put in quotes if it contains spaces). A few functions (aggregating and formatting) are there for your service. You can use arithmetic expressions when it makes sense.

Where to search? Specify with `from` keyword. You can list one or more directories separated with comma.
If you leave the `from`, then current directory will be processed.

What to search? Use `where` with any number of conditions.

Order results like in real SQL with `order by`. All columns are supported for ordering by, 
as well as `asc`/`desc` parameters and positional numeric shortcuts.

Limiting search results is possible with `limit`. Formatting options are supported with `into` keyword.

If you want to use operators containing `>` or `<`, 
put the whole query into the double quotes. 
This will protect query from the shell and output redirection.
The same applies to queries with parentheses or `*`, `?` and other special shell
metacharacters.

It's ok to use any metacharacters in interactive mode.

### It's not a real SQL

Directories to search at are listed with comma separators.
In a real SQL such syntax would make a cross product. Here it means just search at A, next at B, and so on.

You can use curly braces instead of the regular parentheses! This helps to avoid a few of shell pitfalls a little bit.
Functions with no arguments don't require parentheses at all.

String literals don't really need quotes. 
You will need to put them just in case you query something with spaces inside. 
And yes, you should use quotes for glob-patterns or regular expressions in the query 
on Linux or Mac OS to prevent parameter expansion from the shell. 
If you are on Windows, feel free to omit most of the quotes.

Commas for column separation aren't needed as well. Column aliasing (with or without `as` keyword) is not supported.

`where` section can contain short syntax conditions for boolean columns (like `is_audio` or `other_write`).

`into` keyword specifies output format, not output table.

Joins, unions, and subselects are not supported (yet?).

### Columns and fields

| Column                                       | Meaning                                                                                                    | Comment                                                       |
|----------------------------------------------|------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| `name`                                       | Returns the name (with extension) of the file                                                              |                                                               |
| `extension` or `ext`                         | Returns the extension of the file                                                                          |                                                               |
| `path`                                       | Returns the path of the file                                                                               |                                                               |
| `abspath`                                    | Returns the absolute path of the file                                                                      |                                                               |
| `directory` or `dirname` or `dir`            | Returns the directory of the file                                                                          |                                                               |
| `absdir`                                     | Returns the absolute directory of the file                                                                 |                                                               |
| `size`                                       | Returns the size of the file in bytes                                                                      |                                                               |
| `fsize` or `hsize`                           | Returns the size of the file accompanied with the unit                                                     |                                                               |
| `uid`                                        | Returns the UID of the owner                                                                               |                                                               |
| `gid`                                        | Returns the GID of the owner's group                                                                       |                                                               |
| `accessed`                                   | Returns the time the file was last accessed (YYYY-MM-DD HH:MM:SS)                                          |                                                               |
| `created`                                    | Returns the file creation date (YYYY-MM-DD HH:MM:SS)                                                       |                                                               |
| `modified`                                   | Returns the time the file was last modified (YYYY-MM-DD HH:MM:SS)                                          |                                                               |
| `is_dir`                                     | Returns a boolean signifying whether the file path is a directory                                          |                                                               |
| `is_file`                                    | Returns a boolean signifying whether the file path is a file                                               |                                                               |
| `is_symlink`                                 | Returns a boolean signifying whether the file path is a symlink                                            |                                                               |
| `is_pipe` or `is_fifo`                       | Returns a boolean signifying whether the file path is a FIFO or pipe file                                  |                                                               |
| `is_char` or `is_character`                  | Returns a boolean signifying whether the file path is a character device or character special file         |                                                               |
| `is_block`                                   | Returns a boolean signifying whether the file path is a block or block special file                        |                                                               |
| `is_socket`                                  | Returns a boolean signifying whether the file path is a socket file                                        |                                                               |
| `is_hidden`                                  | Returns a boolean signifying whether the file is a hidden file (e.g., files that start with a dot on *nix) |                                                               |
| `has_xattrs`                                 | Returns a boolean signifying whether the file has extended attributes                                      |                                                               |
| `capabilities` or `caps`                     | Returns a string describing Linux capabilities assigned to a file                                          | Available only on Linux                                       |
| `device`                                     | Returns the code of device the file is stored on                                                           | Available only on Linux                                       |
| `inode`                                      | Returns the number of inode                                                                                | Available only on Linux                                       |
| `blocks`                                     | Returns the number of blocks (256 bytes) the file occupies                                                 | Available only on Linux                                       |
| `hardlinks`                                  | Returns the number of hardlinks of the file                                                                | Available only on Linux                                       |
| `mode`                                       | Returns the permissions of the owner, group, and everybody (similar to the first field in `ls -la`)        |                                                               |
| `user`                                       | Returns the name of the owner for this file                                                                | Available only on *nix platforms with `users` feature enabled |
| `user_read`                                  | Returns a boolean signifying whether the file can be read by the owner                                     |                                                               |
| `user_write`                                 | Returns a boolean signifying whether the file can be written by the owner                                  |                                                               |
| `user_exec`                                  | Returns a boolean signifying whether the file can be executed by the owner                                 |                                                               |
| `user_all`                                   | Returns a boolean signifying whether the file can be fully accessed by the owner                           |                                                               |
| `group`                                      | Returns the name of the owner's group for this file                                                        | Available only on *nix platforms with `users` feature enabled |
| `group_read`                                 | Returns a boolean signifying whether the file can be read by the owner's group                             |                                                               |
| `group_write`                                | Returns a boolean signifying whether the file can be written by the owner's group                          |                                                               |
| `group_exec`                                 | Returns a boolean signifying whether the file can be executed by the owner's group                         |                                                               |
| `group_all`                                  | Returns a boolean signifying whether the file can be fully accessed by the group                           |                                                               |
| `other_read`                                 | Returns a boolean signifying whether the file can be read by others                                        |                                                               |
| `other_write`                                | Returns a boolean signifying whether the file can be written by others                                     |                                                               |
| `other_exec`                                 | Returns a boolean signifying whether the file can be executed by others                                    |                                                               |
| `other_all`                                  | Returns a boolean signifying whether the file can be fully accessed by the others                          |                                                               |
| `suid`                                       | Returns a boolean signifying whether the file permissions have a SUID bit set                              |                                                               |
| `sgid`                                       | Returns a boolean signifying whether the file permissions have a SGID bit set                              |                                                               |
| `width`                                      | Returns the number of pixels along the width of the photo or MP4 file                                      |                                                               |
| `height`                                     | Returns the number of pixels along the height of the photo or MP4 file                                     |                                                               |
| `mime`                                       | Returns MIME type of the file                                                                              |                                                               |
| `is_binary`                                  | Returns a boolean signifying whether the file has binary contents                                          |                                                               |
| `is_text`                                    | Returns a boolean signifying whether the file has text contents                                            |                                                               |
| `line_count`                                 | Returns a number of lines in a text file                                                                   |                                                               |
| `exif_datetime`                              | Returns date and time of taken photo                                                                       |                                                               |
| `exif_altitude` or `exif_alt`                | Returns GPS altitude of taken photo                                                                        |                                                               |
| `exif_latitude` or `exif_lat`                | Returns GPS latitude of taken photo                                                                        |                                                               |
| `exif_longitude` or `exif_lng` or `exif_lon` | Returns GPS longitude of taken photo                                                                       |                                                               |
| `exif_make`                                  | Returns name of the camera manufacturer                                                                    |                                                               |
| `exif_model`                                 | Returns camera model                                                                                       |                                                               |
| `exif_software`                              | Returns software name with which the photo was taken                                                       |                                                               |
| `exif_version`                               | Returns the version of EXIF metadata                                                                       |                                                               |
| `mp3_title` or `title`                       | Returns the title of the audio file taken from the file's metadata                                         |                                                               |
| `mp3_album` or `album`                       | Returns the album name of the audio file taken from the file's metadata                                    |                                                               |
| `mp3_artist` or `artist`                     | Returns the artist of the audio file taken from the file's metadata                                        |                                                               |
| `mp3_genre` or `genre`                       | Returns the genre of the audio file taken from the file's metadata                                         |                                                               |
| `mp3_year`                                   | Returns the year of the audio file taken from the file's metadata                                          |                                                               |
| `mp3_freq` or `freq`                         | Returns the sampling rate of audio or video file                                                           |                                                               |
| `mp3_bitrate` or `bitrate`                   | Returns the bitrate of the audio file in kbps                                                              |                                                               |
| `duration`                                   | Returns the duration of audio file in seconds                                                              |                                                               |
| `is_shebang`                                 | Returns a boolean signifying whether the file starts with a shebang (#!)                                   |                                                               |
| `is_empty`                                   | Returns a boolean signifying whether the file is empty or the directory is empty                           |                                                               |
| `is_archive`                                 | Returns a boolean signifying whether the file is an archival file                                          | [default extensions](#ext_archive)                            |
| `is_audio`                                   | Returns a boolean signifying whether the file is an audio file                                             | [default extensions](#ext_audio)                              |
| `is_book`                                    | Returns a boolean signifying whether the file is a book                                                    | [default extensions](#ext_book)                               |
| `is_doc`                                     | Returns a boolean signifying whether the file is a document                                                | [default extensions](#ext_doc)                                |
| `is_font`                                    | Returns a boolean signifying whether the file is a font                                                    | [default extensions](#ext_font)                               |
| `is_image`                                   | Returns a boolean signifying whether the file is an image                                                  | [default extensions](#ext_image)                              |
| `is_source`                                  | Returns a boolean signifying whether the file is source code                                               | [default extensions](#ext_source)                             |
| `is_video`                                   | Returns a boolean signifying whether the file is a video file                                              | [default extensions](#ext_video)                              |
| `sha1`                                       | Returns SHA-1 digest of a file                                                                             |                                                               |
| `sha2_256` or `sha256`                       | Returns SHA2-256 digest of a file                                                                          |                                                               |
| `sha2_512` or `sha512`                       | Returns SHA2-512 digest of a file                                                                          |                                                               |
| `sha3_512` or `sha3`                         | Returns SHA-3 digest of a file                                                                             |                                                               |

### Functions

#### Aggregate functions

Queries using these functions return only one result row.

| Function                  | Meaning                                                       | Example                                              |
|---------------------------|---------------------------------------------------------------|------------------------------------------------------|
| AVG                       | Average of all values                                         | `select avg(size) from /home/user/Downloads`         |
| COUNT                     | Number of all values                                          | `select count(*) from /home/user/Downloads`          |
| MAX                       | Maximum value                                                 | `select max(size) from /home/user/Downloads`         |
| MIN                       | Minimum value                                                 | `select min(size) from /home/user where size gt 0`   |
| SUM                       | Sum of all values                                             | `select sum(size) from /home/user/Downloads`         |
| STDDEV_POP, STDDEV or STD | Population standard deviation, the square root of variance    | `select stddev_pop(size) from /home/user/Downloads`  |
| STDDEV_SAMP               | Sample standard deviation, the square root of sample variance | `select stddev_samp(size) from /home/user/Downloads` |
| VAR_POP or VARIANCE       | Population variance                                           | `select var_pop(size) from /home/user/Downloads`     |
| VAR_SAMP                  | Sample variance                                               | `select var_samp(size) from /home/user/Downloads`    |

#### Date functions

Used mostly for formatting results.

| Function                            | Meaning                                                | Example                                                                  |
|-------------------------------------|--------------------------------------------------------|--------------------------------------------------------------------------|
| CURRENT_DATE or CUR_DATE or CURDATE | Returns current date                                   | `select modified, path where modified = CURDATE()`                       |
| DAY                                 | Extract day of the month                               | `select day(modified) from /home/user/Downloads`                         |
| MONTH                               | Extract month of the year                              | `select month(name) from /home/user/Downloads`                           |
| YEAR                                | Extract year of the date                               | `select year(name) from /home/user/Downloads`                            |
| DOW or DAYOFWEEK                    | Returns day of the week (1 - Sunday, 2 - Monday, etc.) | `select name, modified, dow(modified) from /home/user/projects/FizzBuzz` |

#### User functions

These are only available on Unix platforms when `users` feature has been enabled during compilation.

| Function       | Meaning                    | Example                  |
|----------------|----------------------------|--------------------------|
| CURRENT_UID    | Current real UID           | `select CURRENT_UID()`   |
| CURRENT_USER   | Current real UID's name    | `select CURRENT_USER()`  |
| CURRENT_GID    | Current primary GID        | `select CURRENT_GID()`   |
| CURRENT_GROUP  | Current primary GID's name | `select CURRENT_GROUP()` |

#### Xattr functions

Used to check if particular xattr exists, or to get its value.
Supported platforms are Linux, MacOS, FreeBSD, and NetBSD. 

| Function                      | Meaning                                             | Example                                               |
|-------------------------------|-----------------------------------------------------|-------------------------------------------------------|
| HAS_XATTR                     | Check if xattr exists                               | `select "name, has_xattr(user.test) from /home/user"` |
| XATTR                         | Get value of xattr                                  | `select "name, xattr(user.test) from /home/user"`     |
| HAS_CAPABILITIES or HAS_CAPS  | Check if any Linux capability exists for the file   | `select "name, has_caps() from /home/user"`           |
| HAS_CAPABILITY or HAS_CAP     | Check if given Linux capability exists for the file | `select "name, has_cap('cap_bpf') from /home/user"`   |

#### String functions

Used mostly for formatting results.

| Function                            | Meaning                                                                                                                                                   | Example                                                                         |
|-------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------|
| LENGTH or LEN                       | Length of string value                                                                                                                                    | `select length(name) from /home/user/Downloads order by 1 desc limit 10`        |
| LOWER or LOWERCASE or LCASE         | Convert value to lowercase                                                                                                                                | `select lower(name) from /home/user/Downloads`                                  |
| UPPER or UPPERCASE or UCASE         | Convert value to uppercase                                                                                                                                | `select upper(name) from /home/user/Downloads`                                  |
| INITCAP                             | Returns first letter of each word uppercase, all other letters lowercase                                                                                  | `select initcap('MICHAEL SMITH')`                                               |
| TO_BASE64 or BASE64                 | Encode value to Base64                                                                                                                                    | `select base64(name) from /home/user/Downloads`                                 |
| FROM_BASE64                         | Decode value from Base64                                                                                                                                  | `select from_base64('ZnNlbGVjdCByb2Nrcw==')`                                    |
| SUBSTRING or SUBSTR (str, pos, len) | Part of `str` value starting from `pos` of (optionally) `len` characters long. Negative `pos` means starting `pos` characters from the end of the string. | `select substr(name, 1, 8) from /home/user/Downloads`                           |
| REPLACE (str, from, to)             | Replace all occurrences of `from` by `to`                                                                                                                 | `select replace(name, metallica, MetaLLicA) from /home/user/Music/Rock`         |
| TRIM                                | Returns string with whitespaces at the beginning and the end stripped                                                                                     | `select trim(title), trim(artist), trim(album) from /home/user/Music into json` |
| LTRIM                               | Returns string with whitespaces at the beginning stripped                                                                                                 | `select ltrim(title) from /home/user/Music into json`                           |
| RTRIM                               | Returns string with whitespaces at the end stripped                                                                                                       | `select rtrim(title) from /home/user/Music into json`                           |

#### Japanese string functions

Used for detecting Japanese symbols in file names and such.

| Function                      | Meaning                                         | Example                                                    |
|-------------------------------|-------------------------------------------------|------------------------------------------------------------|
| CONTAINS_JAPANESE or JAPANESE | Check if string value contains Japanese symbols | `select japanese(name) from /home/user/Downloads`          |
| CONTAINS_KANA or KANA         | Check if string value contains kana symbols     | `select kana(name) from /home/user/Downloads`              |
| CONTAINS_HIRAGANA or HIRAGANA | Check if string value contains hiragana symbols | `select contains_hiragana(name) from /home/user/Downloads` |
| CONTAINS_KATAKANA or KATAKANA | Check if string value contains katakana symbols | `select katakana(name) from /home/user/Downloads`          |
| CONTAINS_KANJI or KANJI       | Check if string value contains kanji symbols    | `select kanji(name) from /home/user/Downloads`             |

#### Other functions

| Function                   | Meaning                                                                                     | Example                                                                                       |
|----------------------------|---------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------|
| BIN                        | Convert integer value to binary representation                                              | `select name, size, bin(size) from /home/user/Downloads`                                      |
| HEX                        | Convert integer value to hexadecimal representation                                         | `select name, size, hex(size), upper(hex(size)) from /home/user/Downloads`                    |
| OCT                        | Convert integer value to octal representation                                               | `select name, size, oct(size) from /home/user/Downloads`                                      |
| ABS                        | Returns absolute value of the expression                                                    | `select abs(-5)`                                                                              |
| POWER or POW               | Raise the value to the specified power                                                      | `select pow(2, 3)`                                                                            |
| SQRT                       | Returns square root of the value                                                            | `select sqrt(25)`                                                                             |
| LOG                        | Returns logarithm of the value                                                              | `select log(1000)`                                                                            |
| LN                         | Returns natural logarithm of the value                                                      | `select ln(10)`                                                                               |
| EXP                        | Returns Euler's number raised to the power of the value                                     | `select exp(2)`                                                                               |
| CONTAINS                   | `true` if file contains string, `false` if not                                              | `select contains(TODO) from /home/user/Projects/foo/src`                                      |
| COALESCE                   | Returns first nonempty expression value                                                     | `select name, size, COALESCE(sha256, '---') from /home/user/Downloads`                        |
| CONCAT                     | Returns concatenated string of expression values                                            | `select CONCAT('Name is ', name, ' size is ', fsize, '!!!') from /home/user/Downloads`        |
| CONCAT_WS                  | Returns concatenated string of expression values with specified delimiter                   | `select name, fsize, CONCAT_WS('x', width, height) from /home/user/Images`                    |
| RANDOM or RAND             | Returns random integer (from zero to max int, from zero to *arg*, or from *arg1* to *arg2*) | `select path from /home/user/Music order by RAND()`                                           |
| FORMAT_TIME or PRETTY_TIME | Returns human-readable durations of time in seconds like *2min 26s*                         | `select format_time(duration) from /home/user/Music`                                          |
| FORMAT_SIZE                | Returns formatted size of a file                                                            | `select name, FORMAT_SIZE(size, '%.0') from /home/user/Downloads order by size desc limit 10` |

Let's try `FORMAT_SIZE` with different format specifiers: 

| Specifier                         | Meaning                                                                        | Output      |
|-----------------------------------|--------------------------------------------------------------------------------|-------------|
| `format_size(1678123)`            | Default output                                                                 | 1.60MiB     |
| `format_size(1678123, ' ')`       | Put a space before units                                                       | 1.60 MiB    |
| `format_size(1678123, '%.0')`     | Round up decimal part                                                          | 2MiB        |
| `format_size(1678123, '%.1')`     | One place for decimal part                                                     | 1.6MiB      |
| `format_size(1678123, '%.2')`     | Two places for decimal part                                                    | 1.60MiB     |
| `format_size(1678123, '%.2 ')`    | Two places for decimal part, and put a space before units                      | 1.60 MiB    |
| `format_size(1678123, '%.2 d')`   | Use decimal divider, e.g. 1000-based units, not 1024-based                     | 1.68 MB     |
| `format_size(1678123, '%.2 c')`   | Use conventional format, e.g. 1024-based divider, but display 1000-based units | 1.60 MB     |
| `format_size(1678123, '%.2 k')`   | Display file size in specified unit, this time in kibibytes                    | 1638.79 KiB |
| `format_size(1678123, '%.2 ck')`  | What is a kibibyte? Gimme conventional unit!                                   | 1638.79 KB  |
| `format_size(1678123, '%.0 ck')`  | And drop this decimal part!                                                    | 1639 KB     |
| `format_size(1678123, '%.0 kb')`  | Use 1000-based kilobyte                                                        | 1678 KB     |
| `format_size(1678123, '%.0kb')`   | Don't put a space                                                              | 1678KB      |
| `format_size(1678123, '%.0s')`    | Use short units                                                                | 2M          |
| `format_size(1678123, '%.0 s')`   | Use short units with a space                                                   | 2 M         |

### File size units

| Specifier    | Meaning  | Bytes                     |
|--------------|----------|---------------------------|
| `t` or `tib` | tebibyte | 1024 * 1024 * 1024 * 1024 |
| `tb`         | terabyte | 1000 * 1000 * 1000 * 1000 |
| `g` or `gib` | gibibyte | 1024 * 1024 * 1024        |
| `gb`         | gigabyte | 1000 * 1000 * 1000        |
| `m` or `mib` | mebibyte | 1024 * 1024               |
| `mb`         | megabyte | 1000 * 1000               |
| `k` or `kib` | kibibyte | 1024                      |
| `kb`         | kilobyte | 1000                      |

    fselect size, path from /home/user/tmp where size gt 2g
    fselect fsize, path from /home/user/tmp where size = 5mib
    fselect hsize, path from /home/user/tmp where size lt 8kb

### Search roots

    path [option N] [option] [option] [option...][, path2 [option...]]
    
When you put a directory to search at, you can specify some options.

| Option         | Meaning                                                                                                                                                                             |
|----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| mindepth N     | Minimum search depth. Default is unlimited. Depth 1 means skip one directory level and search further.                                                                              |
| maxdepth N     | Maximum search depth. Default is unlimited. Depth 1 means search the mentioned directory only. Depth 2 means search mentioned directory and its subdirectories. Synonym is `depth`. |
| symlinks       | If specified, search process will follow symlinks. Default is not to follow. Synonym is `sym`.                                                                                      |
| archives       | Search within archives. Only zip archives are supported. Default is not to include archived content into the search results. Synonym is `arc`.                                      |
| gitignore      | Search respects `.gitignore` files found. Synonym is `git`.                                                                                                                         |
| hgignore       | Search respects `.hgignore` files found. Synonym is `hg`.                                                                                                                           |
| dockerignore   | Search respects `.dockerignore` files found. Synonym is `dock`.                                                                                                                     |
| nogitignore    | Disable `.gitignore` parsing during the search. Synonym is `nogit`.                                                                                                                 |
| nohgignore     | Disable `.hgignore` parsing during the search. Synonym is `nohg`.                                                                                                                   |
| nodockerignore | Disable `.dockerignore` parsing during the search. Synonym is `nodock`.                                                                                                             |
| dfs            | Depth-first search mode.                                                                                                                                                            |
| bfs            | Breadth-first search mode. This is the default.                                                                                                                                     |
| regexp         | Use regular expressions to search within multiple roots. Synonym is `rx`.                                                                                                           | 

### Operators

* `=` or `==` or `eq`
* `!=` or `<>` or `ne`
* `===` or `eeq`
* `!==` or `ene`
* `>` or `gt`
* `>=` or `gte` or `ge`
* `<` or `lt`
* `<=` or `lte` or `le`
* `=~` or `~=` or `regexp` or `rx`
* `!=~` or `!~=` or `notrx`
* `like`
* `notlike`
* `between`

### Arithmetic operators

| Operator | Alias  |
|----------|--------|
| +        | plus   |
| -        | minus  |
| *        | mul    |
| /        | div    |
| %        | mod    |

### Date and time specifiers

When you specify inexact date and time with `=` or `!=` operator, **fselect** understands it as an interval.

    fselect path from /home/user where modified = 2017-05-01
    
`2017-05-01` means all day long from 00:00:00 to 23:59:59.

    fselect path from /home/user where modified = '2017-05-01 15'
    
`2017-05-01 15` means one hour from 15:00:00 to 15:59:59.

    fselect path from /home/user where modified ne '2017-05-01 15:10'
    
`2017-05-01 15:10` is a 1-minute interval from 15:10:00 to 15:10:59.

Other operators assume exact date and time, which could be specified in a more free way:

    fselect "path from /home/user where modified === 'apr 1'"
    fselect "path from /home/user where modified gte 'last fri'"
    fselect path from /home/user where modified gte '01/05'

Or simply use relative offsets as days:

    fselect created, path from /home/user where created gte -2

[More about it](https://github.com/stevedonovan/chrono-english)

**fselect** uses *UK* locale, not American style dates, i.e. `08/02` means *February 8th*.

### Regular expressions ###

[Rust flavor regular expressions](https://docs.rs/regex/latest/regex/index.html#syntax) are used.

### MIME and file types

For MIME guessing use field `mime`. It returns a simple string with deduced MIME type,
which is not always accurate.

    fselect path, mime, is_binary, is_text from /home/user

`is_binary` and `is_text` return `true` or `false` based on MIME type detected. 
Once again, this should not be considered as 100% accurate result, 
or even possible at all to detect correct file type.

Other fields listed below **do NOT** use MIME detection.
Assumptions are being made based on file extension.

The lists below could be edited with the configuration file. 

| Search field                            | Extensions                                                                                                                                                                                                                                             |
|-----------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| <a name="ext_archive"></a> `is_archive` | .7z, .bz2, .bzip2, .gz, .gzip, .lz, .rar, .tar, .xz, .zip                                                                                                                                                                                              |
| <a name="ext_audio"></a> `is_audio`     | .aac, .aiff, .amr, .flac, .gsm, .m4a, .m4b, .m4p, .mp3, .ogg, .wav, .wma                                                                                                                                                                               |
| <a name="ext_book"></a> `is_book`       | .azw3, .chm, .djv, .djvu, .epub, .fb2, .mobi, .pdf                                                                                                                                                                                                     |
| <a name="ext_doc"></a> `is_doc`         | .accdb, .doc, .docm, .docx, .dot, .dotm, .dotx, .mdb, .odp, .ods, .odt, .pdf, .potm, .potx, .ppt, .pptm, .pptx, .rtf, .xlm, .xls, .xlsm, .xlsx, .xlt, .xltm, .xltx, .xps                                                                               |
| <a name="ext_font"></a> `is_font`       | .eot, .fon, .otc, .otf, .ttc, .ttf, .woff, .woff2                                                                                                                                                                                                      |
| <a name="ext_image"></a> `is_image`     | .bmp, .exr, .gif, .heic, .jpeg, .jpg, .jxl, .png, .svg, .tga, .tiff, .webp                                                                                                                                                                             |
| <a name="ext_source"></a> `is_source`   | .asm, .bas, .c, .cc, .ceylon, .clj, .coffee, .cpp, .cs, .d, .dart, .elm, .erl, .go, .groovy, .h, .hh, .hpp, .java, .jl, .js, .jsp, .jsx, .kt, .kts, .lua, .nim, .pas, .php, .pl, .pm, .py, .rb, .rs, .scala, .sol, .swift, .tcl, .ts, .vala, .vb, .zig |
| <a name="ext_video"></a> `is_video`     | .3gp, .avi, .flv, .m4p, .m4v, .mkv, .mov, .mp4, .mpeg, .mpg, .webm, .wmv                                                                                                                                                                               |

    fselect is_archive, path from /home/user
    fselect is_audio, is_video, path from /home/user/multimedia
    fselect path from /home/user where is_doc != 1
    fselect path from /home/user where is_image = false
    fselect path from /home/user where is_video != true

### MP3 support

**fselect** can parse basic MP3 metadata and search by bitrate or sampling frequency of the first frame,
title of the track, artist's name, album, genre, and year.

Duration is measured in seconds.

[List of supported genres](https://docs.rs/mp3-metadata/0.3.0/mp3_metadata/enum.Genre.html)

    fselect duration, bitrate, path from /home/user/music
    fselect mp3_year, album, title from /home/user/music where artist like %Vampire% and bitrate gte 320
    fselect bitrate, freq, path from /home/user/music where genre = Rap or genre = HipHop

### File hashes

| Column                 | Meaning                   |
|------------------------|---------------------------|
| `sha1`                 | SHA-1 digest of a file    |
| `sha2_256` or `sha256` | SHA2-256 digest of a file |
| `sha2_512` or `sha512` | SHA2-512 digest of a file |
| `sha3_512` or `sha3`   | SHA3-512 digest of a file |

    fselect path, sha256, 256 from /home/user/archive limit 5
    fselect path from /home/user/Download where sha1 like cb23ef45% 

### Output formats

    ... into FORMAT

| Format   | Description                                                                     |
|----------|---------------------------------------------------------------------------------|
| `tabs`   | default, columns are separated with tabulation                                  |
| `lines`  | each column goes at a separate line                                             |
| `list`   | columns are separated with NULL symbol, similar to `-print0` argument of `find` |
| `csv`    | comma-separated columns                                                         |
| `json`   | array of resulting objects with requested columns                               |
| `html`   | HTML document with table                                                        | 

    fselect size, path from /home/user limit 5 into json
    fselect size, path from /home/user limit 5 into csv
    fselect size, path from /home/user limit 5 into html
    fselect path from /home/user into list | xargs -0 grep foobar

### Configuration file

**fselect** tries to create a new configuration file if one doesn't exist.

Usual location on Linux:

    /home/user_name/.config/fselect/config.toml
    
On Windows:
    
    C:\Users\user_name\AppData\Roaming\jhspetersson\fselect\config.toml
    
Fresh config is filled with defaults, feel free to update it.

If no config on the standard paths found, **fselect** checks its presence next to the executable. 
You can also specify config location with runtime option, e.g.:

    fselect --config /home/user_name/fselect_custom.toml name, size from /home/user_name/Music where is_audio = 1

#### Check for updates

**fselect** can be built with `update-notifications` feature, that enables automatic check for updates.
This check is disabled by default. To enable it, put

    check_for_updates = true

into the config file.

### Command-line arguments

| Argument                                  | Meaning                      |
|-------------------------------------------|------------------------------|
| `--config` or `-c` or `/config`           | Specify config file location |
| `--nocolor` or `--no-color` or `/nocolor` | Disable colors               |
| `--help` or `-h` or `/?` or `/h`          | Show help and exit           |

### Environment variables

**fselect** respects `NO_COLOR` [environment variable](https://no-color.org).

### Exit values

| Value | Meaning                                                             |
|-------|---------------------------------------------------------------------|
| 0     | everything OK                                                       |
| 1     | I/O error has occurred during any directory listing or file reading |
| 2     | error during parsing of the search query                            |
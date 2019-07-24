# fselect

Find files with SQL-like queries

### Basic usage

    fselect [ARGS] COLUMN[, COLUMN...] [from ROOT[, ROOT...]] [where EXPR] [order by COLUMNS] [limit N] [into FORMAT]

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

### It's not a real SQL

Directories to search at are listed with comma separators.
In a real SQL such syntax would make a cross product. Here it means just search at A, next at B, and so on.

String literals don't really need quotes. 
You will need to put them just in case you query something with spaces inside. 
And yes, you should use quotes for glob-patterns or regular expressions in the query 
on Linux or Mac OS to prevent parameter expansion from the shell. 
If you are on Windows, feel free to omit most of the quotes.

Commas for column separation aren't needed as well.

`into` keyword specifies output format, not output table.

Joins, unions, and subselects are not supported (yet?).

### Columns and fields

* `name`
* `path`
* `abspath`
* `size`
* `hsize` or `fsize`
* `uid`
* `gid`
* `user`
* `group`
* `created`
* `accessed`
* `modified`
* `is_dir`
* `is_file`
* `is_symlink`
* `is_pipe` or `is_fifo`
* `is_character` or `is_char`
* `is_block`
* `is_socket`
* `mode`
* `user_read`
* `user_write`
* `user_exec`
* `user_all`
* `group_read`
* `group_write`
* `group_exec`
* `group_all`
* `other_read`
* `other_write`
* `other_exec`
* `other_all`
* `suid`
* `sgid`
* `is_hidden`
* `has_xattrs`
* `is_shebang`
* `width`
* `height`
* `duration`
* `mp3_bitrate` or `bitrate`
* `mp3_freq` or `freq`
* `mp3_title` or `title`
* `mp3_artist` or `artist`
* `mp3_album` or `album`
* `mp3_genre` or `genre`
* `mp3_year`
* `exif_datetime`
* `exif_altitude` or `exif_alt`
* `exif_latitude` or `exif_lat`
* `exif_longitude` or `exif_lng` or `exif_lon`
* `exif_make`
* `exif_model`
* `exif_software`
* `exif_version`
* `mime`
* `is_binary`
* `is_text`
* `line_count`
* `is_archive`
* `is_audio`
* `is_book`
* `is_doc`
* `is_image`
* `is_source`
* `is_video`
* `sha1`
* `sha2_256` or `sha256`
* `sha2_512` or `sha512`
* `sha3_512` or `sha3`

### Functions

#### Aggregate functions

Queries using these functions return only one result row.

| Function | Meaning | Example |
| --- | --- | --- |
| AVG | Average of all values | `select avg(size) from /home/user/Downloads` |
| COUNT | Number of all values | `select count(*) from /home/user/Downloads` |
| MAX | Maximum value | `select max(size) from /home/user/Downloads` |
| MIN | Minimum value | `select min(size) from /home/user where size gt 0` |
| SUM | Sum of all values | `select sum(size) from /home/user/Downloads` |

#### Date functions

Used mostly for formatting results.

| Function | Meaning | Example |
| --- | --- | --- |
| DAY | Extract day of the month | `select day(modified) from /home/user/Downloads` |
| MONTH | Extract month of the year | `select month(name) from /home/user/Downloads` |
| YEAR | Extract year of the date | `select year(name) from /home/user/Downloads` |

#### Xattr functions

Used to check if particular xattr exists, or to get its value.
Supported platforms are Linux, MacOS, FreeBSD, and NetBSD. 

| Function | Meaning | Example |
| --- | --- | --- |
| HAS_XATTR | Check if xattr exists | `select "name, has_xattr(user.test) from /home/user"` |
| XATTR | Get value of xattr | `select "name, xattr(user.test) from /home/user"` |

#### Other functions

Used mostly for formatting results.

| Function | Meaning | Example |
| --- | --- | --- |
| LENGTH | Length of string value | `select length(name) from /home/user/Downloads order by 1 desc limit 10` |
| LOWER | Convert value to lowercase | `select lower(name) from /home/user/Downloads` |
| UPPER | Convert value to uppercase | `select upper(name) from /home/user/Downloads` |
| BASE64 | Encode value to Base64 | `select base64(name) from /home/user/Downloads` |
| CONTAINS | `true` if file contains string, `false` if not | `select contains(TODO) from /home/user/Projects/foo/src` |

### Search roots

    path [depth N] [symlinks] [archives] [gitignore]
    
When you put a directory to search at, you can specify some options.

| Option | Meaning |
| --- | --- |
| mindepth N | Minimum search depth. Default is unlimited. Depth 1 means skip one directory level and search further. |
| maxdepth N | Maximum search depth. Default is unlimited. Depth 1 means search the mentioned directory only. Depth 2 means search mentioned directory and its subdirectories. Synonym is `depth`. |
| symlinks | If specified, search process will follow symlinks. Default is not to follow. Synonym is `sym`. |
| archives | Search within archives. Only zip archives are supported. Default is not to include archived content into the search results. Synonym is `arc`. |
| gitignore | Search respects `.gitignore` files found. Synonym is `git`. |
| hgignore | Search respects `.hgignore` files found. Synonym is `hg`. |

### Operators

* `=` or `==` or `eq`
* `!=` or `<>` or `ne`
* `===`
* `!==`
* `>` or `gt`
* `>=` or `gte` or `ge`
* `<` or `lt`
* `<=` or `lte` or `le`
* `=~` or `~=` or `regexp` or `rx`
* `!=~`
* `like`
* `notlike`

### File size specifiers

| Specifier | Meaning |
| --- | --- |
| `g` or `gb` or `gib` | gibibytes |
| `m` or `mb` or `mib` | mibibytes |
| `k` or `kb` or `kib` | kibibytes |

    fselect size, path from /home/user/tmp where size gt 2g
    fselect fsize, path from /home/user/tmp where size = 5mib
    fselect hsize, path from /home/user/tmp where size lt 8kb

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

[More about it](https://github.com/stevedonovan/chrono-english)

**fselect** uses *UK* locale, not American style dates, i.e. `08/02` means *February 8th*.

### Regular expressions ###

[Rust flavor regular expressions](https://docs.rs/regex/1.1.0/regex/#syntax) are used.

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

| Search field | Extensions |
| --- | --- |
| `is_archive` | .7z, .bz2, .bzip2, .gz, .gzip, .rar, .tar, .xz, .zip |
| `is_audio` | .aac, .aiff, .amr, .flac, .gsm, .m4a, .m4b, .m4p, .mp3, .ogg, .wav, .wma |
| `is_book` | .azw3, .chm, .epub, .fb2, .mobi, .pdf |
| `is_doc` | .accdb, .doc, .docm, .docx, .dot, .dotm, .dotx, .mdb, .ods, .odt, .pdf, .potm, .potx, .ppt, .pptm, .pptx, .rtf, .xlm, .xls, .xlsm, .xlsx, .xlt, .xltm, .xltx, .xps |
| `is_image` | .bmp, .gif, .jpeg, .jpg, .png, .tiff, .webp |
| `is_source` | .asm, .bas, .c, .cc, .ceylon, .clj, .coffee, .cpp, .cs, .dart, .elm, .erl, .go, .groovy, .h, .hh, .hpp, .java, .js, .jsp, .kt, .kts, .lua, .nim, .pas, .php, .pl, .pm, .py, .rb, .rs, .scala, .swift, .tcl, .vala, .vb |
| `is_video` | .3gp, .avi, .flv, .m4p, .m4v, .mkv, .mov, .mp4, .mpeg, .mpg, .webm, .wmv |

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

| Column | Meaning |
| --- | --- |
| `sha1` | SHA-1 digest of a file|
| `sha2_256` or `sha256` | SHA2-256 digest of a file |
| `sha2_512` or `sha512` | SHA2-512 digest of a file |
| `sha3_512` or `sha3` | SHA3-512 digest of a file |

    fselect path, sha256, 256 from /home/user/archive limit 5
    fselect path from /home/user/Download where sha1 like cb23ef45% 

### Output formats

    ... into FORMAT

| Format | Description |
| --- | --- |
| `tabs` | default, columns are separated with tabulation |
| `lines` | each column goes at a separate line |
| `list` | columns are separated with NULL symbol, similar to `-print0` argument of `find` |
| `csv` | comma-separated columns |
| `json` | array of resulting objects with requested columns |
| `html` | HTML document with table | 

    fselect size, path from /home/user limit 5 into json
    fselect size, path from /home/user limit 5 into csv
    fselect size, path from /home/user limit 5 into html
    fselect path from /home/user into list | xargs -0 grep foobar

### Configuration file

**fselect** tries to create a new configuration file if one doesn't exists.

Usual location on Linux:

    /home/user_name/.config/fselect/config.toml
    
On Windows:
    
    C:\Users\user_name\AppData\Roaming\jhspetersson\fselect\config.toml
    
Fresh config is filled with defaults, feel free to update it.

### Command-line arguments

| Argument | Meaning |
| --- | --- |
| `--nocolor` or `--no-color` or `/nocolor` | Disable colors |
| `--help` or `-h` or `/?` or `/h` | Show help and exit |

### Environment variables

**fselect** respects `NO_COLOR` [environment variable](https://no-color.org).
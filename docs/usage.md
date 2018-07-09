# fselect

Find files with SQL-like queries

### Basic usage

    fselect COLUMN[, COLUMN...] [from ROOT[, ROOT...]] [where EXPR] [order by COLUMNS] [limit N] [into FORMAT]

You write SQL-like query, that's it.

**fselect** command itself is like a first keyword (`select`, i.e., *file select*).
But if you'll put one more `select` behind occasionally, that's not a problem.

Next you put columns you are interested in. It could be file name or path, size, modification date, etc.
See full list of possible columns.

Where to search? Specify with `from` keyword. You can list one or more directories separated with comma.
If you leave the `from`, then current directory will be processed.

What to search? Use `where` with any number of conditions.

Order results like in real SQL with `order by`. All columns are supported for ordering by, 
as well as `asc`/`desc` parameters and positional numeric shortcuts.

Limiting search results is possible with `limit`. Formatting options are supported with `into` keyword.

If you want to use operators containing `>` or `<`, 
put the whole query into the double quotes. 
This will protect query from the shell and output redirection.
The same applies to queries with parentheses or *, ? and other special symbols
 that are supposed to be executed on Linux or Mac OS.

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

Joins, unions, aggregating functions, and subselects are not supported (yet?).

### Columns and fields

* `path`
* `name`
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
* `group_read`
* `group_write`
* `group_exec`
* `other_read`
* `other_write`
* `other_exec`
* `is_hidden`
* `has_xattrs`
* `width`
* `height`
* `bitrate`
* `freq`
* `title`
* `artist`
* `album`
* `year`
* `genre`
* `is_archive`
* `is_audio`
* `is_book`
* `is_doc`
* `is_image`
* `is_source`
* `is_video`

### Search roots

    path [depth N] [symlinks] [archives] [gitignore]
    
When you put a directory to search at, you can specify some options.

| Option | Meaning |
| --- | --- |
| depth N | Maximum search depth. Default is unlimited. Depth 1 means search the mentioned directory only. Depth 2 means search mentioned directory and its subdirectories. |
| symlinks | If specified, search process will follow symlinks. Default is not to follow. |
| archives | Search within archives. Only zip archives are supported. Default is not to include archived content into the search results. |
| gitignore | Search respects `.gitignore` files found |

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
* `like`

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

    fselect path from /home/user where modified === 'apr 1'
    fselect path from /home/user where modified gte 'last fri'
    fselect path from /home/user where modified gte '01/05'

[More about it](https://github.com/stevedonovan/chrono-english)

**fselect** uses *UK* locale, not American style dates.

### File extensions

| Search field | Extensions |
| --- | --- |
| `is_archive` | .7z, .bzip2, .gz, .gzip, .rar, .tar, .xz, .zip |
| `is_audio` | .aac, .aiff, .amr, .flac, .gsm, .m4a, .m4b, .m4p, .mp3, .ogg, .wav, .wma |
| `is_book` | .azw3, .chm, .epub, .fb2, .mobi, .pdf |
| `is_doc` | .accdb, .doc, .docx, .dot, .dotx, .mdb, .ods, .odt, .pdf, .ppt, .pptx, .rtf, .xls, .xlt, .xlsx, .xps |
| `is_image` | .bmp, .gif, .jpeg, .jpg, .png, .tiff, .webp |
| `is_source` | .asm, .c, .cpp, .cs, .java, .js, .jsp, .h, .hpp, .pas, .php, .pl, .pm, .py, .rb, .rs, .swift |
| `is_video` | .3gp, .avi, .flv, .m4p, .m4v, .mkv, .mov, .mp4, .mpeg, .mpg, .webm, .wmv |

    fselect is_archive, path from /home/user
    fselect is_audio, is_video, path from /home/user/multimedia
    fselect path from /home/user where is_doc != 1
    fselect path from /home/user where is_image = false
    fselect path from /home/user where is_video != true

### MP3 support

**fselect** can parse basic MP3 metadata and search by bitrate or sampling frequency of the first frame,
title of the track, artist's name, album, genre, and year.

[List of supported genres](https://docs.rs/mp3-metadata/0.3.0/mp3_metadata/enum.Genre.html)

    fselect bitrate, path from /home/user/music
    fselect year, album, title from /home/user/music where artist like %Vampire% and bitrate gte 320
    fselect bitrate, freq, path from /home/user/music where genre = Rap or genre = HipHop

### Output formats

    ... into FORMAT

| Format | Description |
| --- | --- |
| `tabs` | default, columns are separated with tabulation |
| `lines` | each column goes at a separate line |
| `list` | columns are separated with NULL symbol, similar to `-print0` argument of `find` |
| `csv` | comma-separated columns |
| `json` | array of resulting objects with requested columns | 

    fselect size, path from /home/user limit 5 into json
    fselect size, path from /home/user limit 5 into csv

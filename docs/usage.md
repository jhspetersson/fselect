# fselect

Find files with SQL-like queries

### Basic usage

    fselect COLUMN[, COLUMN...] [from ROOT[, ROOT...]] [where EXPR] [limit N] [into FORMAT]

You write SQL-like query, that's it.

**fselect** command itself is like a first keyword (`select`, i.e., *file select*).
But if you'll put one more `select` behind occasionally, that's not a problem.

Next you put columns you are interested in. It could be file name or path, size, modification date, etc.
See full list of possible columns.

Where to search? Specify with `from` keyword. You can list one or more directories separated with comma.
If you leave the `from`, then current directory will be processed.

What to search? Use `where` with any number of conditions.

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
* `is_doc`
* `is_image`
* `is_source`
* `is_video`

### Search roots

    path [depth N] [symlinks] [archives]
    
When you put a directory to search at, you can specify some options.

| Option | Meaning |
| --- | --- |
| depth N | Maximum search depth. Default is unlimited. Depth 1 means search the mentioned directory only. Depth 2 means search mentioned directory and its subdirectories. |
| symlinks | If specified, search process will follow symlinks. Default is not to follow. |
| archives | Search within archives. Only zip archives are supported. Default is not to include archived content into the search results. |

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
| `g` or `gb` | gibibytes |
| `m` or `mb` | mibibytes |
| `k` or `kb` | kibibytes |

    fselect size, path from /home/user/tmp where size gt 2g
    fselect fsize, path from /home/user/tmp where size = 5m
    fselect hsize, path from /home/user/tmp where size lt 8k

### File extensions

| Search field | Extensions |
| --- | --- |
| `is_archive` | .7z, .bzip2, .gz, .gzip, .rar, .tar, .xz, .zip |
| `is_audio` | .aac, .aiff, .amr, .flac, .gsm, .m4a, .m4b, .m4p, .mp3, .ogg, .wav, .wma |
| `is_doc` | .accdb, .doc, .docx, .dot, .dotx, .mdb, .ods, .odt, .pdf, .ppt, .pptx, .rtf, .xls, .xlt, .xlsx, .xps |
| `is_image` | .bmp, .gif, .jpeg, .jpg, .png, .tiff, .webp |
| `is_source` | .asm, .c, .cpp, .cs, .java, .js, .h, .hpp, .pas, .php, .pl, .pm, .py, .rb, .rs, .swift |
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

# fselect
Find files with SQL-like queries

### Why use fselect?

While it doesn't tend to fully replace traditional `find` and `ls`, **fselect** has these nice features:

* complex queries
* SQL-like (not real SQL, but highly relaxed!) grammar easily understandable by humans
* search within archives
* search by width and height of images
* shortcuts to common file types

More is under way!

### Installation

* Install [Rust with Cargo](https://www.rust-lang.org/en-US/install.html) and its dependencies to build a binary
* Run `cargo install fselect`

...or download a statically precompiled binary from Github if you are on a modern Windows 64bit

### Usage

    fselect COLUMN[, COLUMN...] [from ROOT[, ROOT...]] [where EXPR] [limit N]

### Examples

Find temporary or config files (full path and size):

    fselect size, path from /home/user where name = '*.cfg' or name = '*.tmp'
    
Windows users may omit the quotes:

    fselect size, path from C:\Users\user where name = *.cfg or name = *.tmp

Or put all the arguments into the quotes like this:

    fselect "name from /home/user/tmp where size > 0"

Find files (just names) with any content (size > 0):

    fselect name from /home/user/tmp where size gt 0

Specify file size and add it to the results:

    fselect size, path from /home/user/tmp where size gt 2g
    fselect fsize, path from /home/user/tmp where size = 5m
    fselect hsize, path from /home/user/tmp where size lt 8k
    
More complex query:

    fselect name from /tmp where (name = '*.tmp' and size = 0) or (name = '*.cfg' and size gt 1000000)
    
Use single quotes if you need to address files with spaces:

    fselect path from '/home/user/Misc stuff' where name != 'Some file'
    
Regular expressions supported:

    fselect name from /home/user where path ~= '.*Rust.*'
    
And even simple glob will suffice:

    fselect name from /home/user where path = '*Rust*'
    
Exact match operators to search with regexps disabled:

    fselect path from /home/user where name === 'some_*_weird_*_name'
    
Find files by creation date:

    fselect path from /home/user where created = 2017-05-01
    
Be more specific to match all files created at 3PM:

    fselect path from /home/user where created = '2017-05-01 15'
    
And even more specific:

    fselect path from /home/user where created = '2017-05-01 15:10'
    fselect path from /home/user where created = '2017-05-01 15:10:30'
    
Date and time intervals possible (find everything updated since May 1st):

    fselect path from /home/user where modified gte 2017-05-01
    
Default is current directory:

    fselect path, size where name = '*.jpg'
    
Search within multiple locations:

    fselect path from /home/user/oldstuff, /home/user/newstuff where name = '*.jpg'
    
With maximum depth specified:

    fselect path from /home/user/oldstuff depth 5 where name = '*.jpg'
    fselect path from /home/user/oldstuff depth 5, /home/user/newstuff depth 10 where name = '*.jpg'
    
Search within archives (currently only zip-archives are supported):

    fselect path, size from /home/user archives where name = '*.jpg'
    
Or in combination:

    fselect size, path from /home/user depth 5 archives where name = '*.jpg' limit 100    
    
Search by image dimensions:

    fselect width, height, path from /home/user/photos where width gte 2000 or height gte 2000
    
Shortcuts to common file extensions:

    fselect path from /home/user where is_archive = true
    fselect path from /home/user where is_audio = 1
    fselect path from /home/user where is_doc != 1
    fselect path from /home/user where is_image = false
    fselect path from /home/user where is_video != true
    
Find files with dangerous permissions:
    
    fselect mode, path from /home/user where other_write = true or other_exec = true
    
Simple glob-like expressions or even regular expressions on file mode are possible:
    
    fselect mode, path from /home/user where mode = '*rwx'
    fselect mode, path from /home/user where mode ~= '.*rwx$'
    
Find files by owner's uid or gid:

    fselect uid, gid, path from /home/user where uid != 1000 or gid != 1000
    
Or by owner's or group's name:

    fselect user, group, path from /home/user where user = mike or group = mike
    
Finally limit the results:

    fselect name from /home/user/samples limit 5 

### Columns and expression fields

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
* `is_archive`
* `is_audio`
* `is_doc`
* `is_image`
* `is_source`
* `is_video`

### Operators

* `=` or `==` or `eq`
* `!=` or `<>` or `ne`
* `===`
* `!==`
* `>` or `gt`
* `>=` or `gte` or `ge`
* `<` or `lt`
* `<=` or `lte` or `le`
* `~=` or `regexp` or `rx`

### File size specifiers

* `g` or `gb` for gibibytes
* `m` or `mb` for mibibytes
* `k` or `kb` for kibibytes

### File extensions

* is_archive: `.7z`, `.bzip2`, `.gz`, `.gzip`, `.rar`, `.tar`, `.xz`, `.zip`
* is_audio: `.aac`, `.aiff`, `.amr`, `.flac`, `.gsm`, `.m4a`, `.m4b`, `.m4p`, `.mp3`, `.ogg`, `.wav`, `.wma`
* is_doc: `.accdb`, `.doc`, `.docx`, `.dot`, `.dotx`, `.mdb`, `.ods`, `.odt`, `.pdf`, `.ppt`, `.pptx`, `.rtf`, `.xls`, `.xlt`, `.xlsx`, `.xps`
* is_image: `.bmp`, `.gif`, `.jpeg`, `.jpg`, `.png`, `.tiff`, `.webp`
* is_source: `.asm`, `.c`, `.cpp`, `.cs`, `.java`, `.js`, `.h`, `.hpp`, `.pas`, `.php`, `.pl`, `.pm`, `.py`, `.rb`, `.rs`, `.swift`
* is_video: `.3gp`, `.avi`, `.flv`, `.m4p`, `.m4v`, `.mkv`, `.mov`, `.mp4`, `.mpeg`, `.mpg`, `.webm`, `.wmv`

### License

MIT/Apache-2.0

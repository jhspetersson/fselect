# fselect
Find files with SQL-like queries

### Installation

* Install [Rust with Cargo](https://www.rust-lang.org/en-US/install.html) and its dependencies to build a binary
* Run `cargo install fselect`

...or download a statically precompiled binary from Github if you are on a modern Windows 64bit

### Examples

Find images (full path and size):

    fselect path, size from /home/user where name = *.jpg or name = *.png

Find files (just names) with any content (size > 0):

    fselect name from /home/user/tmp where size gt 0

or put arguments into the quotes:

    fselect "name from /home/user/tmp where size > 0"
    
Specify file size:

    fselect path from /home/user where size gt 2g
    fselect path from /home/user where size = 5m
    fselect path from /home/user where size lt 8k
    
More complex query:

    fselect name from /tmp where (name = *.tmp and size = 0) or (name = *.cfg and size gt 1000000)
    
Use single quotes if you need to address files with spaces:

    fselect path from '/home/user/Misc stuff' where name != 'Some file'
    
Regular expressions supported:

    fselect name from /home/user where path ~= .*Rust.*
    
And even simple glob will suffice:

    fselect name from /home/user where path = *Rust*
    
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

    fselect path, size where name = *.jpg
    
Search within multiple locations:

    fselect path from /home/user/oldstuff, /home/user/newstuff where name = *.jpg
    
With maximum depth specified:

    fselect path from /home/user/oldstuff depth 5 where name = *.jpg
    fselect path from /home/user/oldstuff depth 5, /home/user/newstuff depth 10 where name = *.jpg
    
Shortcuts to popular file extensions:

    fselect path from /home/user where is_archive = true
    fselect path from /home/user where is_audio = 1
    fselect path from /home/user where is_image = false
    fselect path from /home/user where is_video != true

### Columns and expression fields

* `path`
* `name`
* `size`
* `created`
* `accessed`
* `modified`
* `is_dir`
* `is_file`

### Operators

* `=` or `eq`
* `!=` or `ne`
* `>` or `gt`
* `>=` or `gte`
* `<` or `lt`
* `<=` or `lte`
* `~=` or `regexp` or `rx`

### File size specifiers

* `g` or `gb` for gibibytes
* `m` or `mb` for mibibytes
* `k` or `kb` for kibibytes

### File extensions

* is_archive: `.7zip`, `.bzip2`, `.gz`, `.gzip`, `.rar`, `.xz`, `.zip`
* is_audio: `.aac`, `.aiff`, `.amr`, `.flac`, `.gsm`, `.m4a`, `.m4b`, `.m4p`, `.mp3`, `.ogg`, `.wav`, `.wma`
* is_image: `.bmp`, `.gif`, `.jpeg`, `.jpg`, `.png`, `.tiff`
* is_video: `.3gp`, `.avi`, `.flv`, `.m4p`, `.m4v`, `.mkv`, `.mov`, `.mp4`, `.mpeg`, `.mpg`, `.webm`, `.wmv`

### License

MIT/Apache-2.0

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

### Columns and expression fields

* `path`
* `name`
* `size`
* `created`
* `accessed`
* `modified`

### Operators

* `=` or `eq`
* `!=` or `ne`
* `>` or `gt`
* `>=` or `gte`
* `<` or `lt`
* `<=` or `lte`
* `~=` or `regexp` or `rx`

### License

MIT/Apache-2.0

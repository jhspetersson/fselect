# fselect
Find files with SQL-like queries

### Examples

Find images:

    fselect name, size from /home/user where name = *.jpg or name = *.png

Find files with any content:

    fselect name from /home/user/tmp where size gt 0

or

    fselect "name from /home/user/tmp where size > 0"
    
More complex query:

    fselect name from /tmp where (name = *.tmp and size = 0) or (name = *.cfg and size gt 1000000)
    
Use single quotes if you need to address files with spaces:

    fselect name from '/home/user/Misc stuff' where name != 'Some file'

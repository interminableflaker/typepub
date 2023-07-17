# typepub (name hopefully subject to change)

## example usage
- Open book at given path with viewport width maximum 120 characters.  
  `> typepub path "~/books/Alice's Adventures in Wonderland.epub" --width 120`
- Open a book in default book directory with `hobbit` in its name, case insensitive.
  `> typepub search hobbit
  
## help
```
OPTIONS:
    -w, --width <width>
      Width of text view, in characters.
      Defaults to 80.

    -h, --help
      Prints help information.

SUBCOMMANDS:

typepub path

  ARGS:
    <path>
      Path to book.


typepub search

  ARGS:
    <search>
      Book name to search for. Case insensitive.

  OPTIONS:
    -l, --library <library>
      Optional directory to search for books.
      Defaults
          Unix:    `$HOME/books`
          Windows: `%HOMEPATH%\\Documents\\books`
```

# typepub (name hopefully subject to change)

wip

e.g.
Open book at path at third chapter, viewport width maximum 120 characters.
`typepub path "~/books/Alice's Adventures in Wonderland.epub" 3 --width 120`
Open a book in default book directory with `hobbit` in its name, chapter eight,
case insensitive.
`typepub search hobbit 8`

```
OPTIONS:
    -w, --width <width>
      Width of text view.

    -h, --help
      Prints help information.

SUBCOMMANDS:

typepub path

  ARGS:
    <path>
      Path to book.

    <chapter>
      Chapter to open.


typepub search

  ARGS:
    [library]
      Optional directory to search for books.
      Defaults
          Unix:    `$HOME/books`
          Windows: `%HOMEPATH%\\Documents\\books`

    <search>
      Book name to search for. Case insensitive.

    <chapter>
      Chapter to open.
```

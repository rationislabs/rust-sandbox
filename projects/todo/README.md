# todo

A small command-line task list. Tasks have an optional due date and tags, overdue ones come
first, and everything lives in one JSON file. Ids stay stable: removing a task never renumbers
the others, and an id is never handed out twice.

```console
$ todo add "Buy milk" --due 2026-11-02 --tag home
added 1
$ todo add "Write spec"
added 2
$ todo add "Renew passport" --due 2026-10-20
added 3
$ todo list
  3 [ ] overdue 2026-10-20 Renew passport
  1 [ ] 2026-11-02         Buy milk #home
  2 [ ]                    Write spec
$ todo list --due today
  3 [ ] overdue 2026-10-20 Renew passport
$ todo list --tag home
  1 [ ] 2026-11-02         Buy milk #home
$ todo done 3
$ todo rm 2
$ todo list --all
  3 [x] 2026-10-20         Renew passport
  1 [ ] 2026-11-02         Buy milk #home
```

(Run on 2026-10-28.) `--due` takes `today` (due today or overdue), `overdue`, or a date
`YYYY-MM-DD` (due on or before it). `--due` and `--tag` also work with `--all`.

## Where the file lives

`--file <path>`, else `$TODO_FILE`, else `$XDG_DATA_HOME/todo/todo.json`, else
`~/.local/share/todo/todo.json`. The folder is created on the first write.

Saving writes `todo.json.tmp` next to the file and renames it over the old one, so a crash
never leaves half a file. A file that does not parse is reported and never overwritten.

## Run and test

```sh
cargo run -p todo -- add "Try it" --tag demo
cargo test -p todo
```

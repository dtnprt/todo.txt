# todo.txt

A [todo.txt](description/README.md) app in three flavours that share one file format and one set of features:

| Client | What it is | Where |
|---|---|---|
| **Web** | Client-only single page app, vanilla JS, no build step, no dependencies | [`web/`](web) |
| **Desktop** | The web app in a [Tauri v2](https://tauri.app) window with native file access | [`src-tauri/`](src-tauri) |
| **Terminal** | `todo`: interactive TUI *and* a scriptable command line (Rust) | [`cli/`](cli) |

Your tasks stay in a plain `todo.txt` file. Nothing is uploaded anywhere.

> Vibe-coded with [Claude Code](https://claude.com/claude-code). Expect rough edges.

## Features

- **Full todo.txt support**: priorities `(A)`, creation/completion dates, `+projects`, `@contexts`, `key:value` tags. Lenient parser.
- **One input for everything**: add tasks or search (`/`), with live syntax highlighting and autocomplete for priorities, `+projects` and `@contexts`.
- **Smart dates**: behind `due:` you can write `tomorrow`, `morgen`, `next week`, `+3d`, `+2w`, `15.03.`, `24.12.2026` … and it becomes `due:YYYY-MM-DD`. A month calendar pops up while you type.
- **Due colours**: overdue, today and soon-due tasks are highlighted.
- **Notes**: `note:NAME` attaches a longer text to a task, stored in `notes/NAME.txt`. In the GUI just type `note:` and a note editor opens.
- **Folder mode**: use a whole folder of `*.txt` lists and switch between them.
- **Encryption** (optional): lists, notes and the WebDAV cache as AES-256-GCM files; password in the OS keyring, optionally released only after Windows Hello. Works in git folders and on WebDAV (they only see ciphertext). `todo --encrypt on`, or Settings → Security.
- **Git sync** (optional, opt-in per folder): pull on start, commit and push after every change, manual sync.
- **Sorting, filtering, drag & drop re-prioritising, undo/redo, archive finished tasks.**
- **Themes** (GUI): 30+ built-in (Nord, Dracula, Catppuccin, Solarized, Rosé Pine, Tokyo Night, … plus a few playful ones) and your own CSS files.
- Files changed by another program (editor, sync client) are reloaded automatically.

## Web

No build step. Serve the folder over HTTP (ES modules need it) and open it:

```sh
cd web
python -m http.server 8000     # then open http://localhost:8000
```

Works best in Chrome/Edge, where the File System Access API can open and save files in place. Other browsers fall back to upload/download. Drag & drop of a file onto the page works too.

Parser tests (Node 18+): `cd web && npm test`.

## Desktop

Needs the [Rust toolchain](https://rustup.rs) and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```sh
cargo install tauri-cli --version "^2"
cargo tauri dev        # run
cargo tauri build      # installer / executable
```

Run them inside `src-tauri/`. The desktop app adds native open/save dialogs, auto save, several remembered files, folder mode, git integration, frameless window, always on top, minimal mode (`F11`) and user themes in `~/.todo.txt/themes/`.

## Terminal

```sh
cd cli
cargo build --release          # binary: target/release/todo
cargo install --path .         # or install it
```

```sh
todo                                   # interactive UI
todo buy milk @shop due:tomorrow       # add a task and exit
todo -l                                # list (add -A for done tasks, -v for details)
todo -l +Home @phone                   # filtered list
todo -x 3                              # toggle done on line 3
todo -p 3 A                            # set priority
todo -d 3                              # delete
todo -u 3 call the bank                # add a sub-entry under line 3 ("- text"; also with -a)
todo --move "+Work" a.txt b.txt        # move matching tasks (+ sub-entries) to another file (--dry-run to preview)
todo -D 3                              # edit the note of line 3 in $EDITOR
todo -f ~/lists                        # use (and remember) a folder
todo -v                                # show folder, file, git state, version, options
```

`todo --help` lists everything. In the TUI press **F1** for the key bindings (`F2` sort, `F3` show done, `F4` sidebar, `F5` archive, `F7` note, `F8` file selector, `F9` git sync, `F10` git settings, `Ctrl+P` parent for new sub-entries, `Ctrl+O` move the filtered tasks to another file).

### Configuration

`~/.config/todo.cfg` (`%USERPROFILE%\.config\todo.cfg` on Windows), plain `key=value` lines, shared by the terminal and desktop apps: `file=`, `folder=`, `sort=`, `showdone=`, `sidebar=`, `calendar=`, `statusbar=`.

### Encryption

```sh
todo --encrypt on        # asks for a password, encrypts the open file/folder (lists + notes), encrypts every later save
todo --crypt-login       # keep the password in the OS keyring
todo --crypt-hello on    # Windows only: ask Windows Hello before the keyring password is used
todo --encrypt off       # decrypt everything again
```

Format: AES-256-GCM, key from PBKDF2-HMAC-SHA256 (600000 iterations); see `cli/src/crypt.rs`. Without the password the files cannot be recovered. Images and `todo.cfg` are not encrypted.

### Git

A todo folder that is itself a git repository can be synced automatically. Opt in per machine:

```sh
todo -f ~/lists --git-init     # creates .todo-git.cfg (excluded from the repo)
todo --git-auto on
git -C ~/lists remote add origin <url>
```

Without `.todo-git.cfg` nothing automatic ever happens. Settings: `enabled`, `auto`, `pull`, `commit`, `push`.

## Repository layout

```
cli/          terminal app (ratatui + crossterm), also the shared git/folder logic
src-tauri/    Tauri v2 desktop wrapper around web/
web/          web app (todotxt.js = pure format logic, app.js = UI, composer.js = input bar)
description/  the todo.txt format specification (GPL-3.0, from the todo.txt project)
tools/        build helper (version + commit hash)
VERSION       MAJOR.MINOR.BUILD
```

All three clients are meant to stay feature-identical; `cli/src/todotxt.rs` is a port of `web/todotxt.js`.

## License

The code is released into the public domain, see [LICENSE](LICENSE). The format description in `description/` is from the [todo.txt project](https://github.com/todotxt/todo.txt) and keeps its own license (GPL-3.0). Icons are [Font Awesome Free](https://fontawesome.com/license/free) (vendored in `web/vendor/fa/`).

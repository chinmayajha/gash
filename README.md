<img src="assets/gash-icon.svg" width="112" height="112" alt="gash icon">

# gash for macOS

A fast, native, from-scratch **bash-compatible shell for macOS**. It is written in Rust and has
zero dependencies. This is a port of [hex2f/gash](https://github.com/hex2f/gash), which
targets Windows and Git Bash. The Windows and MSYS code is replaced with POSIX code.

gash parses and runs bash scripts itself. It does not wrap bash. Subshells, `$(...)`
and pipelines of builtins run in the same process (cloned shell state and threads), so a
script with many of them does not pay a `fork()` for each one. External programs run
with `posix_spawn` or `fork` and `exec` through Rust's `std::process`.

## Status

The test scripts in `tests/scripts` have expected output from bash 5.2. All 70 scripts pass on
macOS (about 2,000 labelled cases). Two scripts, `062_append_redirect` and `064_dev_stdin`,
have expected output made for the macOS tools (BSD `ls` messages, BSD `head` read-ahead).
Unit tests cover the parser, expansion, arithmetic, globbing, patterns, the line editor,
completion and the system call layouts.

## Building

You need Rust (any recent stable version; developed with 1.99).

```bash
cargo build --release
```

The binary is `target/release/gash`. It needs only the system C library and the usual
`/bin` and `/usr/bin` tools.

## Installing

```bash
install -m 755 target/release/gash ~/bin/gash
```

You can also copy it to `/usr/local/bin`. `~/bin` is not on the default macOS `PATH`.
A login shell puts `~/bin` first in `PATH`. To use gash as `bash` or `sh`, copy it (do not
make a symlink) under those names in a directory before `/bin` on `PATH`:

```bash
install -m 755 target/release/gash ~/bin/bash
install -m 755 target/release/gash ~/bin/sh
```

gash reports the name it was started with. Do not replace `/bin/bash`: System Integrity
Protection prevents it, and system scripts need the real bash.

## Using it

```bash
gash                      # interactive shell
gash script.sh args       # run a script
gash -c 'echo hi'         # run a command string
gash -lc '...'            # login shell (reads ~/.bash_profile)
```

Login shells read the first of `~/.bash_profile`, `~/.bash_login` and `~/.profile`.
They also read `/etc/profile` when `GASH_ETC_PROFILE` is set. Interactive shells that are
not login shells read `~/.bashrc` (or the `--rcfile` file).

A login shell builds `PATH` from `~/bin`, the output of `/usr/libexec/path_helper -s`, and
the `PATH` it inherited.

Options: `-c`, `-s`, `-i`, `-l`/`--login`, `--norc`, `--noprofile`, `--rcfile FILE`,
`--posix`, `-o opt`, `-O shopt`, any `set` flag (`-e -u -x ...`), `--version`.

Interactive mode has emacs-style line editing, history (`~/.bash_history`, `HISTCONTROL`,
`!!`, `!$`, `^old^new`, Ctrl+R), and Tab completion of commands, files, `$variables` and
`~` paths. The terminal uses raw mode through termios. The shell restores the earlier mode on
exit, on Ctrl+C and on panic.

## Supported bash features

- Full grammar: pipelines, `&&`/`||`, `&`, subshells, groups, `if`/`case` (with `;&` and
  `;;&`), `while`/`until`/`for`/C-style `for`/`select`, functions, `[[ ]]`, `(( ))`, `time`,
  `!`, here-docs (`<<`, `<<-`, quoted delimiters), here-strings, process substitution,
  aliases.
- All parameter expansions: defaults and alternates, `#` and `%` trimming, `/` `//` `/#` `/%`
  substitution (with `patsub_replacement` `&`), substrings, case modification, `${!ref}`,
  `${!prefix*}`, `${!arr[@]}`, `@Q @E @P @A @a @U @u @L @K`, namerefs.
- Indexed and associative arrays. `declare`, `local`, `readonly` and `export` with all
  attributes. Integer, upper-case and lower-case variables.
- Arithmetic with all bash operators, bases (`0x`, `0`, `N#`) and assignment operators.
- Globbing with `nullglob`, `failglob`, `dotglob`, `nocaseglob`, `globstar`, `extglob` and
  `GLOBIGNORE`. Brace expansion, including sequences.
- `set -euxo pipefail`, `set -C`, `shopt` (all bash option names), traps (`EXIT`, `ERR`,
  `RETURN` and all signals), `getopts`, `read` (all flags), `mapfile`, `printf`
  (with `%q %b %(fmt)T` and `-v`), `echo`, `test` (with the POSIX file tests, checked with
  `access(2)`), `type`, `command`, `hash`, `eval`, `source`, `exec`, `pushd`/`popd`/`dirs`,
  `jobs`/`wait`/`kill`/`fg`/`disown`, `umask`, `ulimit`, `alias`, `history`, `compgen`, and
  more.
- Bash's error-recovery rules. Arithmetic and substitution errors discard the current command
  line. `${x:?}` and `set -u` exit the script.
- Signals. `kill` sends any signal by name or number. A `trap` on `HUP`, `INT`, `TERM` and the
  other signals runs when the signal comes from outside. An untrapped `HUP` or `TERM` ends
  the shell through the normal exit path (the `EXIT` trap runs and the history is saved).
- `$OSTYPE`, `$MACHTYPE`, `$HOSTTYPE` and `$BASH_VERSINFO` give macOS values.

## Differences from bash

- **No `fork()` for shell constructs.** Subshells, `$(...)` and pipeline stages run in the
  same process, on threads that have their own variables, working directory and fd table.
  `$BASHPID` in a subshell is a synthetic number. `kill` on the `$BASHPID` of a subshell only
  works for signals sent to itself.
- **Process substitution** `<(cmd)` and `>(cmd)` uses FIFOs. `cmd` runs on a thread and
  streams, as in bash.
- **Job control.** Background jobs, `jobs`, `wait`, `disown`, `fg` (waits) and `kill` work.
  Ctrl+Z does not stop a job: an interactive shell ignores `TSTP`, `TTIN` and `TTOU`.
  A background job reads from `/dev/null` unless you redirect its input.
- **`exec`** replaces the process only in the top-level shell. In a subshell it runs the
  command and exits.
- **Here-doc input** that a child process does not read stays for the next reader. Small input
  passes through a pipe, and input larger than 64 KB passes through an unlinked temporary file.
- **File descriptors above 2** work inside the shell (`exec 3>file`, `>&3`). External programs
  do not receive them.
- **`umask`** is per shell. A subshell has its own copy.
- Strings are UTF-8 inside gash. Bytes of 0x80 and above in variables (`$'\xff'`) are
  approximate. `printf` and `echo -e` write raw bytes correctly.

## Testing

```bash
cargo test                                        # unit tests and compat suite
GASH_COMPAT_FILTER=015 cargo test --test compat   # one script
GASH_COMPAT_JOBS=8 cargo test --test compat       # parallel
```

Each script in `tests/scripts` has an `.out` file (expected standard output) and a `.code`
file (expected exit status). The `.out` files come from bash 5.2. The suite runs the scripts
with a small `wc` wrapper first on `PATH`, because the BSD `wc` pads its numbers. If a script
fails, the test aborts and does not print the report. Run the script by hand and compare it
with its `.out` file.

## Contributing

**Every commit must be attributed to the model that produced it, if any.** End the commit
message with a trailer for each model used, naming the model and its version:

```
Assisted-By: GPT-5.1
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

`Assisted-By: <model and version>` works for any model. `Co-Authored-By: <model and version>
<noreply@vendor>` is the form that some tools, such as Claude Code, write themselves. If no
model was involved, end the message with `Assisted-By: none`. A PR check fails on any commit
that has none of these lines, and comments on the PR with what to fix. Reword the commits with
`git rebase -i` and force-push.

## Layout

| File | Purpose |
|---|---|
| `src/parser.rs` | Recursive-descent parser (heredocs, aliases, `[[ ]]`, arithmetic, all word forms) |
| `src/expand.rs` | Brace, tilde, parameter, command and arithmetic expansion, IFS splitting, globbing, process substitution |
| `src/exec.rs` | Pipelines, subshells, functions, redirections, process launching, signal dispatch |
| `src/builtins.rs`, `src/builtins_io.rs` | Builtins |
| `src/shell.rs`, `src/vars.rs` | Shell state, copy-on-write variable scopes, options, jobs |
| `src/io.rs` | Per-shell fd table over files, pipes, memory buffers and the console |
| `src/path.rs` | Path and `PATH`-list helpers |
| `src/pattern.rs`, `src/regex.rs` | Glob and extglob matcher, POSIX ERE engine for `=~` |
| `src/arith.rs` | Arithmetic evaluator |
| `src/lineedit.rs`, `src/interactive.rs`, `src/prompt.rs` | Line editor, REPL, completion, prompts |
| `src/sys.rs` | POSIX FFI for macOS (termios, signals, `waitpid`, `stat`, `rlimit`, FIFOs; no `libc` crate) |
| `assets/gash-icon.svg` | The gash icon |
| `.github/workflows/attribution*.yml`, `.github/scripts/check-attribution.sh` | Check that every PR commit names the models that produced it, and comment on the PR when one does not |

## License and attribution

gash is MIT licensed. This port keeps the original [`LICENSE`](LICENSE) and is based on
[hex2f/gash](https://github.com/hex2f/gash) by the gash authors. This fork does not
maintain Windows support.

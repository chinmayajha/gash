//! gash — a fast, native, bash-compatible shell for macOS.
#![allow(dead_code)]

mod arith;
mod ast;
mod builtins;
mod builtins_io;
mod exec;
mod expand;
mod interactive;
mod io;
mod lineedit;
mod parser;
mod path;
mod pattern;
mod prompt;
mod regex;
mod shell;
mod sys;
mod vars;

use shell::*;
use std::path::PathBuf;
use std::sync::OnceLock;
use vars::*;

pub const BASH_VERSION: &str = "5.2.37(1)-release";
pub const BASH_VERSINFO: &[&str] = &["5", "2", "37", "1", "release"];

pub struct Global {
    pub exe: PathBuf,
    pub exe_posix: String,
}

static GLOBAL: OnceLock<Global> = OnceLock::new();

pub fn global() -> &'static Global {
    GLOBAL.get_or_init(|| {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gash"));
        let exe_posix = path::path_str(&exe);
        Global { exe, exe_posix }
    })
}

struct Cli {
    command: Option<String>,
    script: Option<String>,
    read_stdin: bool,
    interactive: Option<bool>,
    login: bool,
    norc: bool,
    noprofile: bool,
    rcfile: Option<String>,
    set_opts: Vec<(String, bool)>,
    set_letters: Vec<(char, bool)>,
    shopts: Vec<(String, bool)>,
    arg0: Option<String>,
    args: Vec<String>,
    posix: bool,
}

fn usage(name: &str) {
    eprintln!(
        "Usage:\t{name} [option] ...\n\t{name} [option] script-file ...\n\
         Options:\n\t-c command\tread commands from the string\n\t-i\t\tinteractive\n\t-l, --login\tlogin shell\n\
         \t-s\t\tread commands from standard input\n\t--norc, --noprofile, --rcfile FILE, --posix, --version\n\
         \t-o option, -O shopt_option, and any `set` flag (-e -u -x ...)"
    );
}

/// `report`: print errors, `--help` and `--version` (off when checking a script's `#!` line).
fn parse_cli(argv: &[String], name: &str, report: bool) -> Result<Cli, i32> {
    let mut c = Cli {
        command: None,
        script: None,
        read_stdin: false,
        interactive: None,
        login: false,
        norc: false,
        noprofile: false,
        rcfile: None,
        set_opts: vec![],
        set_letters: vec![],
        shopts: vec![],
        arg0: None,
        args: vec![],
        posix: false,
    };
    let mut want_c = false;
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if a == "--" || a == "-" {
            i += 1;
            break;
        }
        if let Some(long) = a.strip_prefix("--") {
            match long {
                "login" => c.login = true,
                "norc" => c.norc = true,
                "noprofile" => c.noprofile = true,
                "rcfile" | "init-file" => {
                    i += 1;
                    c.rcfile = argv.get(i).cloned();
                }
                "posix" => c.posix = true,
                "verbose" => c.set_letters.push(('v', true)),
                "noediting" | "restricted" | "debugger" | "dump-strings" | "dump-po-strings" | "wordexp" | "pretty-print" => {}
                "version" => {
                    if report {
                        println!("gash {} (bash-compatible shell for macOS; BASH_VERSION {})", env!("CARGO_PKG_VERSION"), BASH_VERSION);
                        println!("A from-scratch native bash-compatible shell. License: MIT.");
                    }
                    return Err(0);
                }
                "help" => {
                    if report {
                        usage(name);
                    }
                    return Err(0);
                }
                _ => {
                    if report {
                        eprintln!("{}: --{}: invalid option", name, long);
                        usage(name);
                    }
                    return Err(2);
                }
            }
            i += 1;
            continue;
        }
        let on = a.starts_with('-');
        if !(on || a.starts_with('+')) || a.len() < 2 {
            break;
        }
        let flags: Vec<char> = a[1..].chars().collect();
        for f in flags {
            match f {
                'c' if on => want_c = true,
                's' if on => c.read_stdin = true,
                'i' => c.interactive = Some(on),
                'l' => c.login = on,
                'r' | 'D' => {}
                'o' => {
                    i += 1;
                    match argv.get(i) {
                        Some(n) => c.set_opts.push((n.clone(), on)),
                        None => {}
                    }
                }
                'O' => {
                    i += 1;
                    if let Some(n) = argv.get(i) {
                        c.shopts.push((n.clone(), on));
                    }
                }
                other => {
                    if set_opt_by_letter(other).is_some() {
                        c.set_letters.push((other, on));
                    } else {
                        if report {
                            eprintln!("{}: -{}: invalid option", name, other);
                            usage(name);
                        }
                        return Err(2);
                    }
                }
            }
        }
        i += 1;
    }
    let rest: Vec<String> = argv[i.min(argv.len())..].to_vec();
    if want_c {
        let mut it = rest.into_iter();
        match it.next() {
            Some(cmd) => c.command = Some(cmd),
            None => {
                if report {
                    eprintln!("{}: -c: option requires an argument", name);
                }
                return Err(2);
            }
        }
        c.arg0 = it.next();
        c.args = it.collect();
    } else if c.read_stdin || rest.is_empty() {
        c.args = rest;
    } else {
        let mut it = rest.into_iter();
        c.script = it.next();
        c.args = it.collect();
    }
    Ok(c)
}

/// Applies the `set` and `shopt` options of a command line (or of a script's `#!` line).
/// Err holds the message for an unknown option name.
fn apply_options(sh: &mut Shell, cli: &Cli) -> Result<(), String> {
    if cli.posix {
        sh.o[O_POSIX] = true;
    }
    for (c, on) in &cli.set_letters {
        if let Some(i) = set_opt_by_letter(*c) {
            sh.o[i] = *on;
        }
    }
    for (n, on) in &cli.set_opts {
        match set_opt_index(n) {
            Some(i) => sh.o[i] = *on,
            None => return Err(format!("{}: invalid option name", n)),
        }
    }
    for (n, on) in &cli.shopts {
        match shopt_index(n) {
            Some(i) => sh.sh[i] = *on,
            None => return Err(format!("{}: invalid shell option name", n)),
        }
    }
    Ok(())
}

fn is_login_argv0(a0: &str) -> bool {
    a0.starts_with('-')
}

fn import_environment(sh: &mut Shell) {
    let mut extra = Vec::new();
    for (k, v) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        let v = v.to_string_lossy().into_owned();
        if k.is_empty() || k == "_" {
            continue;
        }
        if !parser::is_valid_name(&k) {
            extra.push((k, v));
            continue;
        }
        let var = sh.vars.global_mut(&k);
        var.value = Some(Value::Str(v));
        var.attrs |= EXPORT;
    }
    sh.env_extra = std::sync::Arc::new(extra);
}

fn setv(sh: &mut Shell, k: &str, v: &str, export: bool) {
    let var = sh.vars.global_mut(k);
    var.value = Some(Value::Str(v.to_string()));
    if export {
        var.attrs |= EXPORT;
    }
}

fn setup_environment(sh: &mut Shell, login: bool) {
    // HOME
    let home = match sh.get_str("HOME") {
        Some(h) if !h.is_empty() => h,
        _ => path::path_str(&expand::home_dir()),
    };
    setv(sh, "HOME", &home, true);
    let mut front: Vec<String> = Vec::new();
    if login {
        let home_bin = format!("{}/bin", home);
        if std::path::Path::new(&home_bin).is_dir() {
            front.push(home_bin);
        }
        let mut system = path_helper_dirs();
        if system.is_empty() {
            system = ["/usr/local/bin", "/usr/bin", "/bin"].iter().map(|s| s.to_string()).collect();
        }
        front.extend(system);
    }
    let inherited = sh.get_str("PATH").unwrap_or_default();
    for p in path::split_posix_list(&inherited) {
        if !front.contains(&p) {
            front.push(p);
        }
    }
    if let Some(own) = own_shell_dir() {
        front.retain(|p| p != &own);
        front.insert(0, own);
    }
    setv(sh, "PATH", &front.join(":"), true);
    // misc
    let exe_posix = global().exe_posix.clone();
    if sh.get_str("SHELL").map(|s| s.is_empty()).unwrap_or(true) {
        setv(sh, "SHELL", &exe_posix, true);
    }
    if sh.get_str("TERM").map(|s| s.is_empty()).unwrap_or(true) {
        setv(sh, "TERM", "xterm-256color", true);
    }
    if sh.get_str("USER").is_none() {
        let u = sys::user_name();
        setv(sh, "USER", &u, true);
    }
    if sh.get_str("LOGNAME").is_none() {
        let u = sh.get_str("USER").unwrap_or_default();
        setv(sh, "LOGNAME", &u, false);
    }
    if login && sh.get_str("LANG").is_none() && sh.get_str("LC_ALL").is_none() && sh.get_str("LC_CTYPE").is_none() {
        setv(sh, "LANG", "en_US.UTF-8", true);
    }
    if sh.get_str("HOSTNAME").is_none() {
        let h = sys::host_name();
        setv(sh, "HOSTNAME", &h, false);
    }
    let lvl = sh.get_str("SHLVL").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    setv(sh, "SHLVL", &(lvl + 1).to_string(), true);
    let pwd = path::path_str(&sh.cwd);
    setv(sh, "PWD", &pwd, true);
    if sh.vars.get("OLDPWD").is_none() {
        let v = sh.vars.global_mut("OLDPWD");
        v.attrs |= EXPORT;
    }
    if sh.get_str("HISTFILE").map(|s| s.is_empty()).unwrap_or(true) {
        setv(sh, "HISTFILE", &format!("{}/.bash_history", home), false);
    }
    if sh.get_str("HISTSIZE").is_none() {
        setv(sh, "HISTSIZE", "500", false);
    }
    if sh.get_str("HISTFILESIZE").is_none() {
        setv(sh, "HISTFILESIZE", "500", false);
    }
}

fn path_helper_dirs() -> Vec<String> {
    let Ok(out) = std::process::Command::new("/usr/libexec/path_helper").arg("-s").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let Some(rest) = text.strip_prefix("PATH=\"") else {
        return Vec::new();
    };
    let Some(end) = rest.find('"') else {
        return Vec::new();
    };
    path::split_posix_list(&rest[..end]).into_iter().filter(|p| !p.is_empty()).collect()
}

/// Folder of our executable, if it also provides `bash` or `sh`.
fn own_shell_dir() -> Option<String> {
    let exe = &global().exe;
    let dir = exe.parent()?;
    let stem = exe.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let provides = stem == "bash" || stem == "sh" || dir.join("bash").is_file() || dir.join("sh").is_file();
    if !provides {
        return None;
    }
    Some(path::path_str(dir))
}

fn source_if_exists(sh: &mut Shell, posix: &str) -> Option<R> {
    let p = path::resolve(posix, &sh.cwd);
    if !p.is_file() {
        return None;
    }
    let src = exec::read_script_file(&p).ok()?;
    let saved = std::mem::replace(&mut sh.source_name, posix.to_string());
    sh.sourcing += 1;
    let r = sh.run_source(&src, false);
    sh.sourcing -= 1;
    sh.source_name = saved;
    Some(match r {
        Err(Flow::Return(s)) => Ok(s),
        r => r,
    })
}

fn real_main(argv: Vec<String>) -> i32 {
    sys::startup_umask();
    let argv0 = argv.first().cloned().unwrap_or_else(|| "gash".into());
    let base = argv0.rsplit('/').next().unwrap_or("gash").trim_start_matches('-');
    let name = if base.is_empty() { "gash".to_string() } else { base.to_string() };
    let cli = match parse_cli(&argv, &name, true) {
        Ok(c) => c,
        Err(code) => return code,
    };
    let _ = global();
    let stdin_tty = sys::is_tty(sys::STDIN_FD);
    let interactive = cli.interactive.unwrap_or(cli.command.is_none() && cli.script.is_none() && stdin_tty);
    sys::install_signal_handlers(interactive);

    let mut sh = Shell::new(&name);
    import_environment(&mut sh);
    let login = cli.login || is_login_argv0(&argv0);
    setup_environment(&mut sh, login);
    sh.init_shell_vars();
    sh.interactive = interactive;
    if login {
        sh.sh[sho::login_shell()] = true;
    }
    if name == "sh" {
        sh.o[O_POSIX] = true;
    }
    if let Err(msg) = apply_options(&mut sh, &cli) {
        eprintln!("{}: {}", name, msg);
        return 2;
    }
    if interactive {
        sh.sh[sho::expand_aliases()] = true;
        sh.o[O_HISTEXPAND] = true;
        sh.o[O_HISTORY] = true;
        sh.o[O_MONITOR] = true;
        sh.o[O_EMACS] = true;
        if sh.get_str("PS1").is_none() {
            setv(
                &mut sh,
                "PS1",
                "\\[\\033]0;\\u@\\h:\\w\\007\\]\\n\\[\\033[32m\\]\\u@\\h \\[\\033[33m\\]\\w\\[\\033[36m\\]`__git_ps1`\\[\\033[0m\\]\\n$ ",
                false,
            );
        }
        let a = std::sync::Arc::make_mut(&mut sh.aliases);
        a.insert("ls".into(), "ls -FG".into());
        a.insert("ll".into(), "ls -l".into());
    }

    // positional parameters / $0
    match (&cli.command, &cli.script) {
        (Some(_), _) => {
            sh.arg0 = cli.arg0.clone().unwrap_or_else(|| argv0.clone());
            sh.source_name = name.clone();
        }
        (None, Some(s)) => {
            sh.arg0 = s.clone();
            sh.source_name = s.clone();
        }
        _ => {
            sh.arg0 = argv0.clone();
            sh.source_name = name.clone();
        }
    }
    sh.positional = cli.args.clone();

    // startup files
    let home = sh.get_str("HOME").unwrap_or_default();
    let run_startup = |sh: &mut Shell, r: Option<R>| -> Option<i32> {
        if let Some(Err(Flow::Exit(s))) = r {
            return Some(sh.finish_main(Err(Flow::Exit(s))));
        }
        None
    };
    if login && !cli.noprofile {
        if std::env::var_os("GASH_ETC_PROFILE").is_some() {
            let r = source_if_exists(&mut sh, "/etc/profile");
            if let Some(s) = run_startup(&mut sh, r) {
                return s;
            }
        }
        for f in [".bash_profile", ".bash_login", ".profile"] {
            let p = format!("{}/{}", home, f);
            if let Some(r) = source_if_exists(&mut sh, &p) {
                if let Some(s) = run_startup(&mut sh, Some(r)) {
                    return s;
                }
                break;
            }
        }
    } else if interactive && !cli.norc {
        let rc = cli.rcfile.clone().unwrap_or_else(|| format!("{}/.bashrc", home));
        let r = source_if_exists(&mut sh, &rc);
        if let Some(s) = run_startup(&mut sh, r) {
            return s;
        }
    } else if !interactive {
        if let Some(be) = sh.get_str("BASH_ENV") {
            if !be.is_empty() {
                let _ = source_if_exists(&mut sh, &be);
            }
        }
    }

    if let Some(cmd) = &cli.command {
        return sh.run_command_string(cmd.strip_prefix('\u{feff}').unwrap_or(cmd));
    }
    if let Some(script) = &cli.script {
        let wp = path::resolve(script, &sh.cwd);
        let wp = if wp.is_file() {
            wp
        } else {
            // bash searches PATH for the script if not found
            match sh.find_command(script) {
                Some(r) if r.path.is_file() => r.path,
                _ => {
                    eprintln!("{}: {}: No such file or directory", name, script);
                    return 127;
                }
            }
        };
        let src = match exec::read_script_file(&wp) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: {}: {}", name, script, exec::io_err_msg(&e));
                return 126;
            }
        };
        sh.script_mode = true;
        return sh.run_main(&src);
    }
    if interactive && stdin_tty {
        return interactive::repl(&mut sh);
    }
    // commands from stdin
    let data = io::Handle::Stdin.read_to_end().unwrap_or_default();
    let mut src = String::from_utf8_lossy(&data).into_owned();
    if src.starts_with('\u{feff}') {
        src.remove(0);
    }
    if src.contains("\r\n") {
        src = src.replace("\r\n", "\n");
    }
    if interactive {
        // -i with piped input: execute but keep going after errors
        let r = sh.run_source(&src, false);
        return sh.finish_main(r);
    }
    sh.run_main(&src)
}

fn main() {
    let argv: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    let h = std::thread::Builder::new().stack_size(STACK_SIZE).spawn(move || real_main(argv)).expect("failed to start interpreter thread");
    sys::block_catchable_signals_here();
    let code = h.join().unwrap_or(1);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}

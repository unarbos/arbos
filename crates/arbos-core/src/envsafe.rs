//! What of a process's environment may pass to the kernel and to a job.
//!
//! The desktop, launched from a shell, inherits every `export` in the
//! user's rc file; before this it handed all of that to the kernel and the
//! kernel handed it to every bash job. A stray `OPENAI_API_KEY` in `.zshrc`
//! rode into every command. Now both spawns start from an empty
//! environment plus this allowlist plus what the secrets door grants.

/// Names passed through as they are: what a shell and its tools need to
/// find programs, files, the display, the agent socket, and a locale.
pub const PASS_THROUGH: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "TMP",
    "TEMP",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "SSH_AUTH_SOCK",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "RUST_BACKTRACE",
    // Where a Rust process was started; the desktop needs it for nothing,
    // but cargo-run children do.
    "CARGO_HOME",
    "RUSTUP_HOME",
    // A git identity the user exported is theirs to pass on; a job with
    // none gets the kernel's default (`jobs::git_identity_env`).
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
];

/// Prefixes passed through: locale, XDG folders, our own knobs.
pub const PASS_THROUGH_PREFIXES: &[&str] = &["LC_", "XDG_", "ARBOS_"];

/// Whether `name` is on the allowlist.
pub fn allowed(name: &str) -> bool {
    PASS_THROUGH.contains(&name) || PASS_THROUGH_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Whether a variable's name reads like a credential. A heuristic for
/// `check` and the job scrub: a false positive costs a job one variable
/// it did not need, a false negative is the leak this exists for.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    if n == "SSH_AUTH_SOCK" || n.starts_with("ARBOS_") || n.ends_with("_KEYBOARD") {
        return false;
    }
    let words = [
        "API_KEY",
        "APIKEY",
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "AUTH_KEY",
        "CLIENT_SECRET",
        "_KEY",
    ];
    words.iter().any(|w| n.contains(w))
}

/// The current process's environment reduced to the allowlist plus
/// `extra` names (the model key the config names, the vault token, the
/// variables a place's `secrets.toml` reads with `env:`).
pub fn filtered(extra: &[String]) -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(k, v)| {
            let k = k.into_string().ok()?;
            let v = v.into_string().ok()?;
            (allowed(&k) || extra.iter().any(|e| e == &k)).then_some((k, v))
        })
        .collect()
}

/// Names in the current environment that read like secrets and are not in
/// `managed` (the secrets door's sources and the model key): what `check`
/// warns about.
pub fn stray_secrets(managed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| looks_secret(k) && !managed.iter().any(|m| m == k))
        .collect();
    out.sort();
    out
}

/// Environment variables a place's `.arbos/secrets.toml` reads with
/// `env:VAR`: the desktop lets those through to the kernel so the secrets
/// door can resolve them. The file is `[secrets] NAME = "env:VAR"`.
pub fn place_secret_env_names(place: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(place.join(".arbos").join("secrets.toml")) else {
        return Vec::new();
    };
    let Ok(v) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    v.get("secrets")
        .and_then(|s| s.as_table())
        .map(|t| {
            t.values()
                .filter_map(|src| src.as_str())
                .filter_map(|src| src.strip_prefix("env:"))
                .map(|name| name.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// What a window hands a kernel it starts for `place`: the allowlist, the
/// model key the config names (`key_env`), the vault token, and the
/// variables the place's secrets file reads. Nothing else of the shell the
/// window was launched from.
pub fn kernel_env(place: &std::path::Path, key_env: &str) -> Vec<(String, String)> {
    let mut extra = vec![
        key_env.to_string(),
        "OP_SERVICE_ACCOUNT_TOKEN".to_string(),
        "OP_SESSION".to_string(),
    ];
    extra.extend(place_secret_env_names(place));
    // `op` keeps per-account sessions as OP_SESSION_<id>.
    let mut out = filtered(&extra);
    for (k, v) in std::env::vars() {
        if k.starts_with("OP_SESSION_") && !out.iter().any(|(x, _)| x == &k) {
            out.push((k, v));
        }
    }
    out
}

/// A line for the start of a job's shell script: after the login shell
/// has read the user's profile, drop every variable that reads like a
/// secret unless it is one the secrets door granted (`ARBOS_GRANTED`, a
/// space-separated list of names). `bash -l` sources the rc files, so
/// what the spawn left out can come back; this puts it out again.
pub fn scrub_prologue() -> &'static str {
    r#"for __v in $(env 2>/dev/null | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p'); do case "$__v" in ARBOS_*|SSH_AUTH_SOCK) ;; *API_KEY*|*APIKEY*|*SECRET*|*TOKEN*|*PASSWORD*|*PASSWD*|*CREDENTIAL*|*PRIVATE_KEY*|*ACCESS_KEY*|*AUTH_KEY*|*_KEY) case " ${ARBOS_GRANTED:-} " in *" $__v "*) ;; *) unset "$__v" 2>/dev/null ;; esac ;; esac; done; unset __v;"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_keeps_the_shell_working_and_nothing_secret() {
        for ok in [
            "PATH",
            "HOME",
            "LC_ALL",
            "XDG_CONFIG_HOME",
            "ARBOS_DRIVER_SOCKET",
            "SSH_AUTH_SOCK",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for no in [
            "OPENAI_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "EDITOR",
            "NPM_TOKEN",
        ] {
            assert!(!allowed(no), "{no}");
        }
    }

    #[test]
    fn secret_looking_names() {
        for yes in [
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "DB_PASSWORD",
            "AWS_SECRET_ACCESS_KEY",
            "OP_SERVICE_ACCOUNT_TOKEN",
            "STRIPE_KEY",
        ] {
            assert!(looks_secret(yes), "{yes}");
        }
        for no in [
            "PATH",
            "SSH_AUTH_SOCK",
            "ARBOS_JOB_LOG_CAP",
            "HOME",
            "TERM",
            "GNOME_KEYBOARD",
        ] {
            assert!(!looks_secret(no), "{no}");
        }
    }
}
